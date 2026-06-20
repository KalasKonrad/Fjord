// ── fjord-app · pipewire_fix.rs ──────────────────────────────────────────────
//   is_pipewire_device          true for "" (auto) or pipewire / pipewire/* names
//   apply_alsa_irq_scheduling   set api.alsa.disable-tsched on all PipeWire nodes
//                               (creation-time param; takes effect on next device open)
// ─────────────────────────────────────────────────────────────────────────────

/// Returns true when the selected mpv audio device goes through PipeWire.
/// Empty string = "auto", which on modern Linux defaults to PipeWire.
pub(crate) fn is_pipewire_device(device: &str) -> bool {
    device.is_empty() || device == "pipewire" || device.starts_with("pipewire/")
}

/// Set `api.alsa.disable-tsched` on every PipeWire Node.
///
/// When `enable` is true, PipeWire switches its ALSA backend from software
/// timer scheduling to hardware IRQ wakeups.  This fixes dropout at 192 kHz
/// IEC61937 rates (EAC3/DTS-HD/TrueHD passthrough) under GPU load.
///
/// The property is a creation-time parameter: the change takes effect the
/// next time PipeWire opens the ALSA device (i.e. after the next idle
/// suspend, typically ~5 s after playback stops).
pub(crate) fn apply_alsa_irq_scheduling(enable: bool) {
    let Ok(ls_out) = std::process::Command::new("pw-cli")
        .args(["ls", "Node"])
        .output()
    else {
        tracing::warn!("pw-cli not found — cannot apply ALSA IRQ scheduling");
        return;
    };

    let text = String::from_utf8_lossy(&ls_out.stdout);
    let val   = if enable { "true" } else { "false" };
    let props = format!("{{\"api.alsa.disable-tsched\":{val}}}");
    let mut count = 0u32;

    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with("id ") { continue; }
        let Some(id_str) = line.split(',').next().and_then(|s| s.strip_prefix("id ")) else { continue };
        let id_str = id_str.trim();
        if id_str.is_empty() { continue; }
        let _ = std::process::Command::new("pw-cli")
            .args(["set-param", id_str, "Props", &props])
            .output();
        count += 1;
    }

    tracing::info!("alsa IRQ scheduling: disable-tsched={enable} applied to {count} PipeWire nodes");
}
