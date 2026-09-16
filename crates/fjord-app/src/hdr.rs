// ── fjord-app · hdr.rs ────────────────────────────────────────────────────
//   spawn_worker      entry point, called once from activity::
//                     FjordApplicationHandler the instant real wl_display/
//                     wl_surface handles first become available — spawns a
//                     dedicated OS thread and returns immediately, never
//                     touching the Slint/UI thread. Stage 1+2 name was
//                     `spawn_capability_diagnostic`; renamed since the thread
//                     now does real negotiation, not just diagnostics.
//   run_worker        the actual unsafe FFI + persistent command loop.
//                     Setup (Stage 1+2, unchanged in spirit): attaches a
//                     SECOND, independent wayland-client event queue to the
//                     same Wayland socket winit already owns
//                     (Backend::from_foreign_display), binds
//                     wp_color_manager_v1 if advertised, logs every
//                     capability it reports, wraps Fjord's own existing
//                     wl_surface as a typed proxy. Stage 3 addition: instead
//                     of exiting after setup, the thread then blocks on
//                     `for cmd in rx { ... }`, negotiating a real HDR image
//                     description per HdrCommand::SetHdr and applying/
//                     clearing it on the surface — see HdrCommand's own doc
//                     comment for the full per-command flow. Every fallible
//                     step logs and either skips the command or (for a
//                     genuine Wayland protocol/dispatch error, which is
//                     fatal to the WHOLE connection) exits the thread
//                     entirely — this thread must be structurally incapable
//                     of destabilizing the app regardless of what it
//                     discovers about the compositor.
//   HdrCommand        SetHdr(HdrParams) | Unset — sent via send_command()
//   HdrParams         optional real per-file mastering-luminance/CLL/FALL
//                     metadata for an eligible (PQ + BT.2020) video; TF/
//                     primaries are hardcoded for v1, not fields (see
//                     build_hdr_params's own doc comment)
//   HdrStatus         Idle | Disabled | NotApplicable | Unavailable |
//                     Negotiating | Active | Failed — the stats overlay's
//                     "HDR" row reads this via status_text()
//   send_command      pub(crate), best-effort — a no-op if the worker was
//                     never spawned (X11) or has since exited
//   maybe_negotiate   pub(crate) — called once per playback (see playback.rs's
//                     wire_mpv_timer hook) with the source's real HDR
//                     metadata; runs eligibility, sets status, sends SetHdr
//   set_status_disabled  pub(crate) — called instead of maybe_negotiate when
//                     the "HDR passthrough" Settings toggle is off; sets
//                     status only, sends no Wayland command at all
//   build_hdr_params  pure eligibility function — Some only for gamma=="pq"
//                     && primaries=="bt.2020" (this exact compositor doesn't
//                     advertise Hlg — see the live capability log this was
//                     designed against)
//   WorkerState       the worker thread's own Dispatch target AND general
//                     mutable state for its whole lifetime — not consumed
//                     anywhere else
//
//   Cross-thread state note: HDR_STATUS/HDR_CHANNEL below are the one place
//   in this codebase using a module-level `static` instead of the usual
//   Arc::clone()-from-main() convention — see HDR_CHANNEL's own doc comment
//   for exactly why (a genuine bootstrap-ordering constraint, not a
//   shortcut): confirmed from main.rs that FjordApplicationHandler —  the
//   only code that ever learns the real wl_display/wl_surface handles — is
//   constructed and registered before MainWindow::new(), which is itself
//   before FjordState/VideoState exist at all, so there is no point before
//   FjordApplicationHandler's own construction where an Arc<Mutex<...>>
//   clone of either could be threaded in.
// ───────────────────────────────────────────────────────────────────────────

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::mpsc;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{registry_queue_init, BindError, GlobalListContents};
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::color_management::v1::client::wp_color_management_surface_v1::WpColorManagementSurfaceV1;
use wayland_protocols::wp::color_management::v1::client::wp_color_manager_v1::{
    self, Feature, Primaries, RenderIntent, TransferFunction, WpColorManagerV1,
};
use wayland_protocols::wp::color_management::v1::client::wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1;
use wayland_protocols::wp::color_management::v1::client::wp_image_description_v1::{
    self, WpImageDescriptionV1,
};

// ── on-screen/status surface ────────────────────────────────────────────────

/// The stats overlay's "HDR" row (Stage 3) reads this on the same ~500ms
/// cadence it reads every other stats field on. Written from two different
/// threads for two different reasons: the app-level trigger in
/// `playback.rs` sets `Disabled`/`NotApplicable`/`Negotiating` *before* any
/// Wayland command is even sent; the worker thread sets `Active`/`Failed`/
/// `Unavailable` once it actually knows the outcome. `Unset` unconditionally
/// resets it to `Idle` regardless of whether anything was actually active —
/// see `run_worker`'s own `HdrCommand::Unset` arm for why this must be
/// unconditional (a real gap an independent review pass caught: without it,
/// an HDR video's terminal status visibly lingered through a following
/// audio-only track, since the stats overlay's VIDEO section isn't hidden
/// during audio playback and the whole trigger is skipped for audio items).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum HdrStatus {
    #[default]
    Idle,
    Disabled,
    NotApplicable,
    Unavailable,
    Negotiating,
    Active,
    Failed,
}

impl HdrStatus {
    fn as_str(self) -> &'static str {
        match self {
            HdrStatus::Idle          => "Idle",
            HdrStatus::Disabled      => "Disabled",
            HdrStatus::NotApplicable => "Not applicable",
            HdrStatus::Unavailable   => "Unavailable",
            HdrStatus::Negotiating   => "Negotiating…",
            HdrStatus::Active        => "Active (HDR10)",
            HdrStatus::Failed        => "Failed",
        }
    }
}

static HDR_STATUS: Mutex<HdrStatus> = Mutex::new(HdrStatus::Idle);

fn set_status(s: HdrStatus) {
    *HDR_STATUS.lock().unwrap() = s;
}

/// Read by the stats overlay (`stats.rs`) every refresh. Never blocks for
/// long — a plain uncontended `Mutex<enum>` read.
pub(crate) fn status_text() -> &'static str {
    HDR_STATUS.lock().unwrap().as_str()
}

/// hdr branch, Stage 4 (2026-09-16) — read from `wire_mpv_timer`'s own poll
/// to detect the Idle/Negotiating→Active transition and apply real mpv HDR
/// output settings exactly once per item. Deliberately doesn't leak
/// `HdrStatus` itself outside this module (which stays private) — a plain
/// bool is all any caller actually needs.
pub(crate) fn is_active() -> bool {
    *HDR_STATUS.lock().unwrap() == HdrStatus::Active
}

// ── commands ─────────────────────────────────────────────────────────────

/// Real, per-file mastering-luminance/CLL/FALL metadata for an already-
/// eligible video (see `build_hdr_params`). Transfer function and primaries
/// are hardcoded to St2084Pq/Bt2020 by the worker itself for v1 — eligibility
/// already guarantees the source matches those, so they aren't fields here.
pub(crate) struct HdrParams {
    pub(crate) min_lum:  Option<f64>,
    pub(crate) max_lum:  Option<f64>,
    pub(crate) max_cll:  Option<f64>,
    pub(crate) max_fall: Option<f64>,
}

pub(crate) enum HdrCommand {
    /// Negotiate and apply a real HDR image description for the given
    /// source metadata. A prior active description (from an earlier item)
    /// is simply overwritten — `tear_down_player` already sends `Unset`
    /// before any new item's own playback ever starts, so by the time this
    /// arrives nothing should still be active, but the worker doesn't
    /// depend on that ordering for correctness.
    SetHdr(HdrParams),
    /// Clear whatever's currently applied (if anything) and reset the
    /// on-screen status to `Idle`. Sent unconditionally from
    /// `tear_down_player` on every playback teardown — cheap when nothing
    /// was ever set (no Wayland call at all in that case).
    Unset,
}

/// See this file's own header for why this is a `static LazyLock` rather
/// than something threaded from `main()`. Constructed lazily on first touch
/// — by either `send_command` (a UI-thread/timer-thread caller) or the
/// worker-spawn logic (the winit-callback thread) — whichever runs first;
/// `LazyLock` handles that race safely with no explicit init call needed
/// from `main()` at all.
type HdrChannel = (mpsc::Sender<HdrCommand>, Mutex<Option<mpsc::Receiver<HdrCommand>>>);
static HDR_CHANNEL: LazyLock<HdrChannel> = LazyLock::new(|| {
    let (tx, rx) = mpsc::channel();
    (tx, Mutex::new(Some(rx)))
});

/// Best-effort, matching this module's established non-fatal ethos: a no-op
/// if the worker thread was never spawned (non-Wayland launch) or has since
/// exited (compositor doesn't advertise the global, or a fatal protocol
/// error killed the connection).
///
/// `Unset` synchronously resets `HDR_STATUS` to `Idle` on the *calling*
/// thread, before the command is even queued — real bug, found during Stage
/// 4 planning (2026-09-16): the worker only resets the status once it
/// actually gets around to processing a queued `Unset`, on its own isolated
/// thread, with no guarantee this has happened by the time `tear_down_player`
/// (the only caller of `Unset`) returns and the next item's own
/// `reset_video_state_for_playback`/`wire_mpv_timer` ticks start running.
/// Harmless before Stage 4 (the only reader was a passive stats-overlay
/// string), but Stage 4's own poll for "did negotiation just succeed" would
/// otherwise see a stale `Active` left over from the *previous* item and
/// wrongly apply real-HDR-output mpv properties to the *new* item's Player
/// before its own negotiation has even started. `set_status`/`HDR_STATUS`
/// are a plain `Mutex`, freely callable from any thread, so this is safe;
/// the worker's own eventual `Unset` handling still runs (to actually tear
/// down the real Wayland surface state) and still sets `Idle` itself —
/// redundant but idempotent by then.
pub(crate) fn send_command(cmd: HdrCommand) {
    if matches!(cmd, HdrCommand::Unset) {
        set_status(HdrStatus::Idle);
    }
    let _ = HDR_CHANNEL.0.send(cmd);
}

/// Called once per playback, from `wire_mpv_timer`'s one-shot hook, with the
/// just-reconfigured player's real source metadata — only when the "HDR
/// passthrough" Settings toggle is on (see `set_status_disabled` for the
/// off case, which never reaches here at all).
pub(crate) fn maybe_negotiate(meta: fjord_player::SourceHdrMetadata) {
    match build_hdr_params(&meta) {
        Some(params) => {
            set_status(HdrStatus::Negotiating);
            send_command(HdrCommand::SetHdr(params));
        }
        None => set_status(HdrStatus::NotApplicable),
    }
}

/// Called instead of `maybe_negotiate` when the Settings toggle is off —
/// pure status update, no Wayland command is ever sent for this case.
pub(crate) fn set_status_disabled() {
    set_status(HdrStatus::Disabled);
}

/// Eligibility — deliberately narrow for v1: only genuine HDR10 (PQ +
/// BT.2020) content, the one combination this exact compositor is confirmed
/// (live, Stage 2) to advertise support for. `gamma`/`primaries` values
/// already empirically confirmed live for real HDR10 content on this exact
/// server ("bt.2020 · pq", from the earlier CLR IN/OUT freeze-bug
/// investigation). HLG content and anything with non-named/custom primaries
/// deliberately stay at today's tone-mapped behavior — see CLAUDE.md's HDR
/// section for the full reasoning; revisit if a future KWin version
/// advertises `Hlg`.
fn build_hdr_params(meta: &fjord_player::SourceHdrMetadata) -> Option<HdrParams> {
    if meta.gamma != "pq" || meta.primaries != "bt.2020" {
        return None;
    }
    Some(HdrParams {
        min_lum:  meta.min_luma,
        max_lum:  meta.max_luma,
        max_cll:  meta.max_cll,
        max_fall: meta.max_fall,
    })
}

// ── spawn / setup ────────────────────────────────────────────────────────

/// Called once, from `activity::FjordApplicationHandler::window_event`, the
/// first time a real winit `Window` (and therefore real wl_display/
/// wl_surface handles) exists. Spawns a dedicated thread and returns
/// immediately — never blocks the caller, never touches Slint state.
/// Best-effort: a spawn failure is logged and otherwise harmless, matching
/// this codebase's established `slint::set_xdg_app_id`/`BackendSelector::select()`
/// non-fatal pattern.
pub(crate) fn spawn_worker(display: NonNull<c_void>, surface: NonNull<c_void>) {
    // Raw pointers aren't `Send`, so they can't cross the thread::spawn
    // boundary directly — carry them as plain addresses and cast back to
    // pointers on the other side. Sound for the same reason spelled out in
    // the safety comment below: both addresses stay valid for the whole
    // life of the process (the one window Fjord ever creates never closes
    // before the process itself does).
    let display_addr = display.as_ptr() as usize;
    let surface_addr = surface.as_ptr() as usize;
    if let Err(e) = std::thread::Builder::new()
        .name("fjord-hdr-worker".into())
        .spawn(move || {
            // Safety: `display_addr`/`surface_addr` come from a
            // `RawDisplayHandle::Wayland`/`RawWindowHandle::Wayland` pair
            // obtained from winit's own `Window::display_handle()`/
            // `window_handle()` the instant this thread was spawned — both
            // are guaranteed valid, non-null, live Wayland objects for at
            // least the lifetime of the window (per raw-window-handle's own
            // safety contract), and Fjord's single window lives for the
            // whole process.
            unsafe { run_worker(display_addr as *mut c_void, surface_addr as *mut c_void) };
        })
    {
        tracing::warn!("couldn't spawn HDR worker thread: {e}");
    }
}

/// The actual unsafe FFI plus the persistent command loop. Every fallible
/// setup step logs and returns early (thread exits, `HDR_STATUS` left at
/// `Unavailable`) rather than panicking. Once the command loop starts, a
/// genuine Wayland protocol/dispatch error (fatal to the whole shared
/// connection — the same one winit's own window uses) also ends the thread
/// rather than looping forever uselessly; `send_command` already tolerates
/// "the worker is gone" as a silent no-op, so nothing else needs to know.
unsafe fn run_worker(display_ptr: *mut c_void, surface_ptr: *mut c_void) {
    // Safety: see spawn_worker's own safety comment.
    let backend = unsafe { Backend::from_foreign_display(display_ptr.cast()) };
    let connection = Connection::from_backend(backend);

    let (globals, mut event_queue) = match registry_queue_init::<WorkerState>(&connection) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("hdr worker: registry_queue_init failed: {e}");
            set_status(HdrStatus::Unavailable);
            return;
        }
    };
    let qh = event_queue.handle();

    let mut state = WorkerState::default();

    let manager = match globals.bind::<WpColorManagerV1, WorkerState, ()>(
        &qh,
        1..=WpColorManagerV1::interface().version,
        (),
    ) {
        Ok(manager) => manager,
        Err(BindError::NotPresent) => {
            tracing::info!(
                "hdr worker: compositor does not advertise wp_color_manager_v1 \
                 (staging color-management-v1) — real HDR passthrough unavailable this session"
            );
            set_status(HdrStatus::Unavailable);
            return;
        }
        Err(e) => {
            tracing::warn!("hdr worker: binding wp_color_manager_v1 failed: {e}");
            set_status(HdrStatus::Unavailable);
            return;
        }
    };

    // Same capability roundtrip Stage 2 already did — one roundtrip is
    // enough to receive the whole burst for a freshly-bound global, per the
    // protocol's own "done" semantics.
    if let Err(e) = event_queue.roundtrip(&mut state) {
        tracing::warn!("hdr worker: roundtrip after binding wp_color_manager_v1 failed: {e}");
        set_status(HdrStatus::Unavailable);
        return;
    }
    if state.done {
        tracing::info!(
            "hdr worker: compositor advertises wp_color_manager_v1 (color-management-v1) \
             — intents={:?} features={:?} tf_named={:?} primaries_named={:?}",
            state.supported_intents,
            state.supported_features,
            state.supported_tf_named,
            state.supported_primaries_named,
        );
    } else {
        tracing::warn!(
            "hdr worker: bound wp_color_manager_v1 but never received its 'done' event \
             — capability list may be incomplete"
        );
    }

    // Wrap Fjord's own existing wl_surface as a typed proxy on THIS (new,
    // second) connection — needed for get_surface() below and for the
    // explicit .commit() calls the SetHdr/Unset arms issue. `ObjectId::
    // from_ptr` cross-checks the real interface name against the pointer's
    // own `wl_proxy_get_class` before ever succeeding, so this can't
    // silently wrap the wrong kind of object.
    // Safety: `surface_ptr` is the same already-validated Wayland pointer
    // passed into this function; `WlSurface::interface()` is the correct,
    // matching interface descriptor for it.
    let wrapped_surface: Option<WlSurface> =
        match unsafe { ObjectId::from_ptr(WlSurface::interface(), surface_ptr.cast()) } {
            Ok(object_id) => match Proxy::from_id(&connection, object_id) {
                Ok(surface) => {
                    let surface: WlSurface = surface;
                    tracing::info!(
                        "hdr worker: wrapped Fjord's own wl_surface as a typed WlSurface proxy \
                         on the new connection ({:?})",
                        surface.id()
                    );
                    Some(surface)
                }
                Err(e) => {
                    tracing::warn!("hdr worker: Proxy::from_id for the existing wl_surface failed: {e}");
                    None
                }
            },
            Err(e) => {
                tracing::warn!("hdr worker: ObjectId::from_ptr for the existing wl_surface failed: {e}");
                None
            }
        };
    let Some(wrapped_surface) = wrapped_surface else {
        // No surface to negotiate onto at all — nothing further this
        // worker can usefully do. Exiting (rather than looping and no-op'ing
        // every command forever) matches every other fatal-setup path above.
        set_status(HdrStatus::Unavailable);
        return;
    };

    let Some(rx) = HDR_CHANNEL.1.lock().unwrap().take() else {
        // Structurally shouldn't happen (this is the only place that ever
        // takes it, guarded by activity.rs's own one-time capture flag) —
        // defensive, not expected.
        tracing::warn!("hdr worker: command receiver already taken — exiting");
        return;
    };

    // Persistent negotiation state, for the life of this thread.
    let mut cms: Option<WpColorManagementSurfaceV1> = None; // lazily get_surface()'d, at most once ever
    let mut has_active = false;

    for cmd in rx {
        match cmd {
            HdrCommand::SetHdr(params) => {
                if let Err(fatal) = handle_set_hdr(
                    &manager, &qh, &mut event_queue, &mut state,
                    &wrapped_surface, &mut cms, &mut has_active, params,
                ) {
                    tracing::warn!("hdr worker: {fatal} — connection likely dead, exiting");
                    set_status(HdrStatus::Unavailable);
                    return;
                }
            }
            HdrCommand::Unset => {
                if has_active {
                    if let Some(surface) = cms.as_ref() {
                        surface.unset_image_description();
                        wrapped_surface.commit();
                    }
                    has_active = false;
                }
                // Unconditional — see HDR_STATUS's own doc comment for why
                // this must always reset to Idle, even when nothing was
                // actually active.
                set_status(HdrStatus::Idle);
            }
        }
    }
}

/// One `SetHdr` command's worth of negotiation. Returns `Err(reason)` only
/// for a genuine Wayland dispatch/roundtrip error (fatal to the whole
/// connection) — every other failure (compositor capability gone,
/// `failed`/timeout from the compositor) is handled internally via
/// `HdrStatus::Failed` and a normal `Ok(())` return, since those don't
/// affect the connection's own health.
#[allow(clippy::too_many_arguments)]
fn handle_set_hdr(
    manager: &WpColorManagerV1,
    qh: &QueueHandle<WorkerState>,
    event_queue: &mut wayland_client::EventQueue<WorkerState>,
    state: &mut WorkerState,
    wrapped_surface: &WlSurface,
    cms: &mut Option<WpColorManagementSurfaceV1>,
    has_active: &mut bool,
    params: HdrParams,
) -> Result<(), String> {
    set_status(HdrStatus::Negotiating);

    // Defensive re-check against this compositor's own captured
    // capabilities — eligibility upstream already implies these, but a
    // genuinely different/future compositor shouldn't be trusted blind.
    let has_tf = has_value(&state.supported_tf_named, TransferFunction::St2084Pq);
    let has_prim = has_value(&state.supported_primaries_named, Primaries::Bt2020);
    let has_feature = |f: Feature| has_value(&state.supported_features, f);
    if !has_tf || !has_prim || !has_feature(Feature::Parametric) {
        tracing::warn!("hdr worker: compositor no longer advertises what HDR10 negotiation needs — skipping");
        set_status(HdrStatus::Failed);
        return Ok(());
    }

    let creator: WpImageDescriptionCreatorParamsV1 = manager.create_parametric_creator(qh, ());
    creator.set_tf_named(TransferFunction::St2084Pq);
    creator.set_primaries_named(Primaries::Bt2020);

    // Optional mastering luminance — gated on the one feature flag the
    // protocol ties both set_mastering_display_primaries and
    // set_mastering_luminance to. set_mastering_display_primaries itself is
    // deliberately never called: per the protocol's own text, omitting it
    // makes the target volume default to matching the primary color volume
    // (BT.2020) — exactly what's wanted, one fewer call, one fewer thing to
    // get chromaticity-coordinate scaling wrong on.
    if has_feature(Feature::SetMasteringDisplayPrimaries) {
        if let (Some(min_l), Some(max_l)) = (params.min_lum, params.max_lum) {
            if max_l > min_l {
                let min_scaled = (min_l * 10_000.0).round() as u32;
                let max_scaled = max_l.round() as u32;
                creator.set_mastering_luminance(min_scaled, max_scaled);

                // max_cll/max_fall are ONLY ever attempted alongside a
                // validated mastering-luminance range, checked against that
                // real range in cd/m² — the protocol's own version-1-only
                // bound ("max_cll/max_fall must be > min L and <= max L of
                // the mastering range") applied unconditionally regardless
                // of which interface version actually negotiated, since
                // satisfying the stricter check is always also valid under
                // the more permissive v2+ rule. A value sent outside this
                // bound would raise a FATAL invalid_luminance protocol
                // error on the shared connection — real gap an independent
                // review pass caught, fixed here rather than left open.
                let cll_ok = params.max_cll.is_some_and(|c| c > min_l && c <= max_l);
                if let Some(cll) = params.max_cll.filter(|_| cll_ok) {
                    creator.set_max_cll(cll.round() as u32);
                    match params.max_fall {
                        Some(fall) if fall > min_l && fall <= max_l && fall <= cll => {
                            creator.set_max_fall(fall.round() as u32);
                        }
                        // A real, live-observed case, not hypothetical: a
                        // file's own max_fall (or max_cll) can legitimately
                        // fall outside its own mastering range — a real
                        // metadata inconsistency some HDR10 masters carry.
                        // Send max_cll alone rather than risk this one
                        // extra property taking the whole negotiation down
                        // with a fatal protocol error.
                        Some(fall) => tracing::debug!(
                            "hdr worker: skipping max_fall={fall} — out of mastering range \
                             ({min_l}..={max_l}) or exceeds max_cll={cll}"
                        ),
                        None => {}
                    }
                } else if let Some(cll) = params.max_cll {
                    tracing::debug!(
                        "hdr worker: skipping max_cll={cll} (and any max_fall) — out of \
                         mastering range ({min_l}..={max_l})"
                    );
                }
            }
        }
    }

    state.pending_result = None;
    let img: WpImageDescriptionV1 = creator.create(qh, ());

    let start = Instant::now();
    while state.pending_result.is_none() && start.elapsed() < Duration::from_secs(2) {
        event_queue
            .roundtrip(state)
            .map_err(|e| format!("roundtrip while waiting for ready2/failed failed: {e}"))?;
    }

    match state.pending_result.take() {
        Some(Ok(())) => {
            if cms.is_none() {
                *cms = Some(manager.get_surface(wrapped_surface, qh, ()));
            }
            if let Some(surface) = cms.as_ref() {
                surface.set_image_description(&img, RenderIntent::Perceptual);
                img.destroy();
                wrapped_surface.commit();
                *has_active = true;
                set_status(HdrStatus::Active);
                tracing::info!(
                    "hdr worker: negotiated HDR10 image description — \
                     min_lum={:?} max_lum={:?} max_cll={:?} max_fall={:?}",
                    params.min_lum, params.max_lum, params.max_cll, params.max_fall,
                );
            } else {
                tracing::warn!("hdr worker: no wp_color_management_surface_v1 to apply the image description to");
                set_status(HdrStatus::Failed);
            }
        }
        Some(Err((cause, msg))) => {
            tracing::warn!("hdr worker: image description creation failed: cause={cause:?} msg={msg}");
            set_status(HdrStatus::Failed);
        }
        None => {
            tracing::warn!("hdr worker: timed out waiting for ready2/failed from the compositor");
            set_status(HdrStatus::Failed);
        }
    }
    Ok(())
}

fn has_value<T: PartialEq>(list: &[WEnum<T>], target: T) -> bool {
    list.iter().any(|w| matches!(w, WEnum::Value(v) if *v == target))
}

/// The worker thread's own `Dispatch` target for its whole lifetime — not
/// just the one-shot capability roundtrip Stage 2 originally used this
/// shape for. `supported_*`/`done` are populated once, at startup, then
/// read-only for the rest of the thread's life. `pending_result` is set by
/// `Dispatch<WpImageDescriptionV1, ()>`'s own event handler and polled by
/// `handle_set_hdr`'s bounded roundtrip-wait loop; cleared before every new
/// `create()` attempt — safe because negotiation is fully serial (the
/// command loop only ever has one `SetHdr` in flight at a time).
#[derive(Default)]
struct WorkerState {
    // `WEnum<T>` (not bare `T`) is what wayland-scanner actually generates
    // for an enum-typed event argument — `Value(T)` for a recognized wire
    // value, `Unknown(u32)` otherwise. Kept as-is (not unwrapped) rather
    // than filtered/converted: for a diagnostic log, an "Unknown(N)" entry
    // is itself useful information (a compositor advertising a value this
    // crate's own vendored protocol version doesn't yet know the name of),
    // not a case to discard.
    supported_intents: Vec<WEnum<RenderIntent>>,
    supported_features: Vec<WEnum<Feature>>,
    supported_tf_named: Vec<WEnum<TransferFunction>>,
    supported_primaries_named: Vec<WEnum<Primaries>>,
    done: bool,
    pending_result: Option<Result<(), (WEnum<wp_image_description_v1::Cause>, String)>>,
}

// Required by `registry_queue_init` — this worker only ever needs the
// registry's INITIAL global list (a single well-known, always-present
// global, not a dynamic multi-instance one like wl_output/wl_seat), so
// dynamic add/remove events are deliberately ignored.
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for WorkerState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpColorManagerV1, ()> for WorkerState {
    fn event(
        state: &mut Self,
        _proxy: &WpColorManagerV1,
        event: wp_color_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        use wp_color_manager_v1::Event;
        match event {
            Event::SupportedIntent { render_intent } => state.supported_intents.push(render_intent),
            Event::SupportedFeature { feature } => state.supported_features.push(feature),
            Event::SupportedTfNamed { tf } => state.supported_tf_named.push(tf),
            Event::SupportedPrimariesNamed { primaries } => {
                state.supported_primaries_named.push(primaries)
            }
            Event::Done => state.done = true,
            // `Event` is #[non_exhaustive] — a future protocol version could
            // add more events; nothing here needs them.
            _ => {}
        }
    }
}

impl Dispatch<WpImageDescriptionV1, ()> for WorkerState {
    fn event(
        state: &mut Self,
        _proxy: &WpImageDescriptionV1,
        event: wp_image_description_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        use wp_image_description_v1::Event;
        match event {
            // ready2 (v2+) and the deprecated 32-bit ready (v1) both mean
            // the same thing for our purposes — treated identically so this
            // is correct regardless of which interface version negotiates.
            Event::Ready2 { .. } | Event::Ready { .. } => state.pending_result = Some(Ok(())),
            Event::Failed { cause, msg } => state.pending_result = Some(Err((cause, msg))),
            _ => {}
        }
    }
}

// wp_image_description_creator_params_v1 and wp_color_management_surface_v1
// both have zero <event> elements in the protocol (requests only) — a
// Dispatch impl is still required to construct a proxy of either type, but
// there is nothing to ever handle.
impl Dispatch<WpImageDescriptionCreatorParamsV1, ()> for WorkerState {
    fn event(
        _state: &mut Self,
        _proxy: &WpImageDescriptionCreatorParamsV1,
        _event: <WpImageDescriptionCreatorParamsV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpColorManagementSurfaceV1, ()> for WorkerState {
    fn event(
        _state: &mut Self,
        _proxy: &WpColorManagementSurfaceV1,
        _event: <WpColorManagementSurfaceV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}
