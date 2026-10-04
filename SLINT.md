# Slint — Best Practices for Fjord

Reference for structuring and writing Slint UI code. Covers file organisation,
globals, property visibility, and patterns that have caused real bugs in this
codebase. The full write-ups of runtime gotchas that caused real bugs are in
§ Known Slint gotchas at the end of this file (a condensed checklist is in `CLAUDE.md`).

---

## File organisation

Slint is designed for multi-file projects. Use relative imports — no build.rs
changes needed:

```slint
import { Theme } from "theme.slint";
import { PlayerScreen } from "player.slint";
```

The entry point passed to `slint_build::compile()` is the root; everything else
is imported from it transitively.

### When to split into a separate file

- Self-contained components with a clean property interface (e.g. `MediaCard`,
  `FjordButton`, `SectionRow`)
- Full-screen overlays that own a coherent block of state and layout
- Anything over ~300 lines that has a single clear responsibility

### When to keep things together

- Components that are tightly coupled to each other's internal layout
- The keyboard handler — it branches on all screen modes simultaneously; keep it
  in `main.slint` where it can see all state

---

## Globals for shared state

`global` singletons are accessible from any `.slint` file without threading
properties down through the component tree. Use them for state that multiple
unrelated components need to read or write.

```slint
// app_state.slint
export global AppState {
    in-out property <bool>  is-playing:      false;
    in-out property <bool>  show-series:     false;
    in-out property <int>   focused-section: -1;
    // ...
}
```

```slint
// player.slint
import { AppState } from "app_state.slint";

export component PlayerScreen {
    visible: AppState.is-playing;
    // ...
}
```

```slint
// main.slint
import { AppState } from "app_state.slint";
import { PlayerScreen } from "player.slint";

export component MainWindow inherits Window {
    // keyboard handler writes to AppState directly:
    fs := FocusScope {
        key-pressed(event) => {
            if event.text == Key.Escape && AppState.is-playing {
                AppState.is-playing = false;
                return accept;
            }
            reject
        }
    }
    PlayerScreen { }
}
```

Fjord already uses this pattern for `Theme` in `theme.slint`. The same approach
applies to screen-mode flags and navigation state.

---

## Property visibility

| Modifier | Readable from outside | Writable from outside | Use for |
|---|---|---|---|
| `property` (plain) | No | No | Internal state |
| `in property` | No | Yes | Configuration passed in by parent |
| `out property` | Yes | No | Values the component publishes |
| `in-out property` | Yes | Yes | Bidirectional / two-way binding |

Prefer the most restrictive modifier that works. Internal state that only the
component itself reads and writes should be plain `property`, not `in-out`.

---

## Callbacks vs properties for actions

Use **callbacks** for one-shot actions (play, close, navigate). Use **properties**
for state (is-playing, focused-card). Callbacks cannot be two-way bound and do
not have change-detection overhead.

```slint
// Good
callback play-item(string);   // fires once, carries the item id
in-out property <bool> is-playing;  // state, can be observed

// Avoid
in-out property <string> item-to-play;  // polling anti-pattern
```

---

## Keyboard handler structure

All keyboard input goes through a single zero-size `FocusScope` at the top of
`MainWindow`. The handler is a chain of exclusive `if` blocks — each screen mode
is checked first and returns `accept` on a match so lower blocks never fire for
the wrong screen. The contract:

```slint
key-pressed(event) => {
    // Most specific / highest-priority screen first:
    if AppState.is-playing { /* player keys */ return accept/reject; }
    if AppState.show-series { /* series keys */ return accept/reject; }
    if AppState.show-detail { /* detail keys */ return accept/reject; }
    // ... etc.
    // Global shortcuts last (always active):
    if event.text == "q" { root.quit(); return accept; }
    reject
}
```

`return accept` — event handled, stop propagation.
`return reject` — unhandled, let Slint propagate to focusable children.

---

## Scrollable containers

**`Flickable` is the only reliably keyboard-scrollable container.** Bind
`viewport-y` to an externally-tracked `length` property and clamp it:

```slint
property <length> scroll: 0px;

Flickable {
    viewport-height: content.preferred-height;
    viewport-y: clamp(-scroll, min(0px, self.height - self.viewport-height), 0px);

    content := VerticalLayout { /* ... */ }
}
```

Drive `scroll` from the keyboard handler. Reset it to `0px` whenever the overlay
closes.

**Do not use `ScrollView`** for keyboard-driven scroll — it manages `viewport-y`
internally and silently ignores any binding you set on it.

**Do not use `ListView`** when you need to drive scroll from outside — it also
writes to `viewport-y` from its own scroll handler, overwriting your binding.

---

## Visibility vs opacity

| | Hit-testable | Cursor shape | `TouchArea` fires |
|---|---|---|---|
| `visible: false` | No | No | No |
| `opacity: 0` | **Yes** | **Yes** | **Yes** |

`opacity: 0` makes an element invisible but fully interactive. Use `visible:
false` to remove something from event handling. When fading controls in/out with
`opacity`, add a full-size `TouchArea` (declared last, highest z-order) with
`enabled: !controls-visible` and `mouse-cursor: MouseCursor.none` to suppress
hit-testing while the controls are hidden.

---

## Mouse movement without a button held

`TouchArea.moved` fires only during a drag (button held). To react to plain
cursor movement use property-change callbacks:

```slint
TouchArea {
    changed mouse-x => { root.show-controls(); }
    changed mouse-y => { root.show-controls(); }
}
```

---

## Layout gotchas

- **`Rectangle` children are horizontally centred by default.** Set `x: 0`
  explicitly on fill bars, overlays, or anything that must be left-anchored.
- **`preferred-height` on a named `VerticalLayout`** gives the intrinsic height
  of its content — use this as `Flickable.viewport-height` for keyboard-scrollable
  lists.
- **`HorizontalLayout` / `VerticalLayout` with `alignment: start`** prevents
  children from stretching to fill the container when you don't want that.

---

## `invoke_from_event_loop` (Rust ↔ Slint)

Closures passed to `invoke_from_event_loop` must be `'static + Send`. Capture
owned values, not references:

```rust
// Good
let title = item.title.clone();   // owned String
let _ = slint::invoke_from_event_loop(move || {
    window.set_title(title.into());
});

// Bad — does not compile
let _ = slint::invoke_from_event_loop(move || {
    window.set_title(item.title.as_str().into());  // &str is not 'static
});
```

Use `Arc<T>` when you need shared ownership inside the closure.

---

## Known Slint gotchas

These have each caused real bugs in this codebase:

**`Flickable` is the only reliable keyboard-scrollable container.** `ScrollView` ignores declarative `viewport-y` bindings (it manages its own scroll internally). `ListView` also writes to `viewport-y` from its own scroll handler, silently overwriting any binding you set. The correct pattern for any keyboard-driven scrollable list is `Flickable { viewport-height: ...; VerticalLayout { for ... } }` with `viewport-y` bound to a `clamp(...)` expression that tracks the focused index.

**Do not self-reference a `Flickable`'s own layout properties in its `viewport-y` binding.** Writing `viewport-y: clamp(... flk.height ... flk.viewport-height ...)` creates a binding whose dependencies Slint may not reliably track — `flk.height` and `flk.viewport-height` are layout-managed and may not trigger binding re-evaluation when `player-panel-cursor` changes. Instead, reference `parent.height` (the outer Rectangle's height) and the content layout's `preferred-height` directly: `clamp(-(cursor * 34px) + parent.height / 2 - 17px, min(0px, parent.height - list.preferred-height), 0px)`. This is what fixed the track panel scroll bug (#22).

**A `viewport-y` binding on a `Flickable` blocks native mouse-wheel scrolling.** When `viewport-y` is bound to an expression, the Flickable's internal scroll handler can't write to it (the binding overrides any assignment on the next frame), so mouse-wheel does nothing. When both keyboard nav and mouse-wheel scroll are needed: remove the `viewport-y` binding; on the outer Rectangle declare `property <length> kb-y: clamp(...)` and `changed kb-y => { fl.viewport-y = kb-y; }` for keyboard nav; the Flickable then handles mouse-wheel natively. Also fix any `fl.height` / `self.viewport-height` self-references in the old binding expression — use the outer container's `self.height` and the content layout's `preferred-height` instead. This is the pattern applied to all scrollable Flickables in the codebase (player panels, detail, series, home/movies/TV dashboards, library grid, settings right pane, browse list). Note: the browse list previously used `interactive: false` which blocks mouse-wheel regardless of bindings; changing it to `interactive: true` re-enables native scroll while child `TouchArea` clicks still fire normally (Slint distinguishes drag from click).

**Plain `Rectangle` children are horizontally centred by default.** If you need a fill bar or overlay anchored to the left edge, you must set `x: 0` explicitly. Omitting it centres the element and produces the "progress bar starts from the middle" bug.

**`KeyEvent.repeat` is unreliable — never use it to guard state transitions.** In practice `repeat` can be `false` for auto-repeated key events (confirmed on desktop Wayland, not just wireless keyboards). A guard like `if !event.repeat { close_screen() }` will fire on every spurious non-repeat event during a hold, chaining through screens unexpectedly. The correct pattern is to let the state machine be the guard: once the transition fires (e.g. `show-browse = false`), the outer `if AppState.show-browse` condition stops subsequent events from re-firing it. For search fields specifically: Backspace should only delete characters; use Escape as the dedicated "exit search" key. Never use `!event.repeat` to gate a backspace-exits-search path — a held Backspace will empty the query and then bleed into the close-screen handler.

**Slint ternary short-circuits dependency tracking.** If a property binding uses `condition ? A : B` and `B` contains a reactive property (e.g. `has-hover`), Slint only tracks `B`'s dependencies when the else-branch is actually evaluated. If the condition is initially true, `has-hover` is never read and hover changes never trigger a repaint. Fix: read the property unconditionally first using a block expression — `background: { let hov = ta.has-hover; cond ? Theme.accent : (hov ? Theme.surface : transparent) };`. This was the root cause of settings left-pane hover not working.

**`invoke_from_event_loop` closures must be `'static + Send`.** Capture owned values (`String`, `Arc<…>`) not references. This is the correct pattern for communicating from Tokio tasks back to Slint UI state.

**`TouchArea.moved` fires only during drag (button held), not plain cursor movement.** To react to mouse movement without a button press, use `changed mouse-x => { ... }` and `changed mouse-y => { ... }` callbacks. This is how the player controls overlay auto-show is implemented.

**`opacity: 0` elements remain fully hit-testable.** Setting `opacity: 0` makes an element invisible but it still participates in hit-testing and determines the mouse cursor shape — only `visible: false` removes it from event handling. The player controls bar fades via `opacity`, so its child `TouchArea`s were silently overriding `mouse-cursor: none` on the element beneath them. The fix is a full-size cursor-hider `TouchArea` declared last (highest z-order) with `enabled: !root.controls-visible` and `mouse-cursor: MouseCursor.none`. When `enabled: false`, a `TouchArea` passes events through to elements below it.

**`self.width` / `self.height` inside a conditional element reads from the parent layout cache — do not use it in ChangeTracker properties.** When a `changed prop => { ... }` ChangeTracker initialises it immediately reads the tracked property. If that property reads `self.width` or `self.height`, and `self` is an element inside a VerticalLayout/HorizontalLayout that is a conditional (`if cond: Element { ... }`), then `self.width`/`self.height` is derived from the parent layout's cache. If that layout cache is currently being evaluated — e.g. because a `kb-y` anchor position triggered it — and the layout cache called `ensure_updated()` on this very conditional (which then ran `user_init` → ChangeTracker → reads `self.width` → reads the layout cache) — Slint detects recursion and panics. **Fix:** replace `self.width`/`self.height` with `root.width`/`root.height` (the component root's size, set by the outer parent, never derived from the internal layout cache). Both values are always equal when the conditional element fills the full component width/height, which is the normal case. This was the root cause of the series screen "Recursion detected" crash — season tabs `kb-x` read `self.width`, which came from the content VerticalLayout's layout cache that was in the middle of calling `ensure_updated()` on the season tabs conditional.

**A plain child reading its immediate parent `HorizontalLayout`/`VerticalLayout`'s own `.width`/`.height` to compute its own size is a stricter, related trap — caught by the compiler outright, not a runtime panic.** Different from the gotcha above in two ways: no conditional (`if cond:`) is required, and the failure is a compile error, not a "Recursion detected" runtime panic. Mechanism: when a Layout element (`HorizontalLayout`/`VerticalLayout`) has no externally-imposed width, it computes its own width from its children's `layoutinfo` (preferred/min/max sizes, cached as `layout-cache`) — so if one of those children reads `parent.width` to compute *its own* `width:` binding, that child's width is simultaneously an *input to* and *dependent on* the same `layout-cache` resolution, which Slint's compiler rejects: `The binding for the property 'width' is part of a binding loop (layoutinfo-h -> layout-cache -> width -> width -> layoutinfo-h)`. Real example, live-hit 2026-07-18 (see the Seerr integration section's button-position saga for the full story): a plain `Rectangle` set to `width: parent.width - 288px` where `parent` was the enclosing `HorizontalLayout` — this looks identical in shape to the many already-safe `parent.width`/`parent.height` reads elsewhere in this codebase (e.g. `PosterBlock`'s own children), but those are all safe specifically because their `parent` is a plain `Rectangle` with an already-fixed/explicit size, not a Layout element currently resolving its own width from that very child. **Fix:** read `root.width`/`root.height` instead (the component's own top-level size, resolved by a separate, already-completed layout pass one level further out — safe for the identical reason the gotcha above recommends it) — every screen with this pattern in this codebase wraps its body in a `Flickable { width: parent.width; viewport-width: self.width; }` near the component root, so `root.width` threads down unchanged to wherever it's needed, several levels deep, with no circularity. **Rule of thumb:** `parent.width`/`parent.height` is safe when `parent` is a plain `Rectangle`/element with a fixed or already-otherwise-determined size; it is NOT safe when `parent` is a Layout element whose own size is still being derived from its children — reach for `root.width`/`root.height` (or another confirmed-already-resolved ancestor) instead.

**A sibling `Rectangle` with `width`/`height` bound to `root.width + Npx` / `root.height + Npx` (an *outset*, not flush) can also trigger "Recursion detected" during mouse-hover hit-testing.** `MediaCard`'s focus-ring overlay (see `widgets.slint`) was briefly changed from flush-with-root (`x:0; y:0; width:root.width; height:root.height;`) to a 3px outset (`x:-3px; y:-3px; width:root.width+6px; height:root.height+6px;`) so the ring wouldn't overlap the poster image/title text. This crashed live during hover — `properties.rs:583: Recursion detected`, inside `item_geometry` evaluation triggered by `send_mouse_event_to_item` — even though `root.width`/`root.height` is exactly the pattern the *previous* gotcha recommends as the safe fix, and this Rectangle isn't inside a conditional at all. Root cause not fully isolated (the backtrace only pointed at an internal, auto-numbered `Opacity_...` wrapper, not directly at this binding), but reverting the outset back to flush-with-root immediately and reproducibly fixed it. **Takeaway:** don't assume `root.width ± constant` is automatically as safe as bare `root.width` — if a card/row's focus ring needs to visually clear its own content, inset the *content* inward within the element's existing, already-stable bounds instead of expanding the ring outward past them; this also sidesteps a second, independent problem outsetting has (a `Flickable`-scrolled container clips content that intentionally overflows an item's own declared bounds, so an outset ring gets visibly cut off for edge-of-row items regardless of the crash). **Part 2 — the "inset the content" fix has its own trap:** doing that inset by giving the content `VerticalLayout` explicit geometry bindings (`x:3px; y:3px; width: root.width - 6px; height: root.height - 6px;`) crashed on the HTPC with the identical `properties.rs:583: Recursion detected` panic (`fjord.log` r716, triggered ~9s into playback when a WS delta refresh rebuilt the Continue Watching row's model). The dev machine never reproduced it — the difference is the *instantiation site*: `LibraryGrid` creates `MediaCard`s with `width:` only, no explicit `height:`, so `root.height` there is assigned by the enclosing row `HorizontalLayout`'s layout cache; an explicit-geometry binding on a child **layout element** that reads `root.height` feeds back into the same constraint computation that produces `root.height` → cycle. `SectionRow` passes `height:` explicitly (a plain property, no cache), which is why dashboards were immune and dev-machine testing passed. **Fix:** inset via `padding: 3px` on the layout (a native layout property, resolved entirely inside the layout's own cache — no external geometry binding), and have sized children read `root.height - constant` (plain-Rectangle children reading root has been stable for months) rather than `parent.height` (the padded layout, whose height participates in layout-info). General rule: never put explicit `x/y/width/height` bindings derived from `root` geometry on a **layout element** inside a reusable component — some instantiation site will eventually size the component through a layout cache and close the cycle.

**`parent` inside a reusable child component refers to that component's own immediate parent, not the grandparent two levels up.** `widgets.slint`'s `FadeInTrigger` (dropped into a screen as `fade := FadeInTrigger {}` to fade it in via `opacity: fade.fade-opacity`) originally wrote `parent.opacity = 1` directly inside its own `Timer.triggered`. Since the `Timer` is nested one level inside `FadeInTrigger`, and `FadeInTrigger` is nested one level inside the actual host screen, `parent` from the Timer's perspective resolved to `FadeInTrigger` itself (which has no meaningful `opacity` effect on anything), not the host screen two levels up — the write silently no-oped and every screen using it stayed stuck at `opacity: 0` forever (confirmed via a real screenshot showing a fully black content area, not a Slint compile error — `parent.opacity = 1` type-checks fine against `FadeInTrigger`'s own built-in `opacity` property). **Fix:** never rely on `parent` chaining out of a reusable child component to reach an ancestor two-plus levels up. Instead expose an `out property` on the child (`out property <float> fade-opacity: 0;`) and have the ancestor bind to it by name (`opacity: fade.fade-opacity;`) — this works regardless of nesting depth since it's a direct property reference, not a parent walk. **Also note:** `if`-gated element ids (`fade := FadeInTrigger {}` written identically inside N different `if cond: Element {...}` blocks) are NOT scoped per-conditional — Slint requires every named id to be unique across the whole enclosing component, so reusing the same id text in more than one `if` block is a compile error ("duplicated element id"), even though each block only ever instantiates one of them at a time. Every such usage needs a distinct id (e.g. `fade1`, `fade2`, ...).

**Returning a brand-new `ModelRc` from a "refresh this row" helper makes Slint destroy and recreate every delegate element, even when every row's data is identical.** A `Repeater`/`for` loop tracks the *model instance* it's bound to; swapping in a different `ModelRc` (e.g. `ModelRc::new(VecModel::from(rows))`) — rather than mutating the existing one — is indistinguishable from "everything changed" to Slint, so every child element (here, each card's poster `Image`) gets torn down and rebuilt from scratch. This silently defeated the whole point of `refresh_row_preserving_posters`/`upsert_cards_in_model` (Phases 91/93/94): they correctly carried the *poster data* forward into the new model, but the new model instance itself still forced every `Image` to be recreated, re-running `FadeInTrigger`'s fade-in (Phase 92) for cards whose poster never actually changed — a real flash, just one level down from where the fix was aimed. **Fix:** when the refreshed data has the same ids in the same order as what's already there (check first), call `existing_model.set_row_data(i, new_card)` for each index and return the *same* `ModelRc` — this fires a per-row change notification that Slint applies to the existing delegate in place, so unrelated conditional children (like the poster `Image`) are never destroyed. Only fall back to building a new model when the row *membership or order* genuinely changed (a real content difference, where recreating elements is correct and expected, not a bug). **Phase 96 note:** this same same-shape/mutate-in-place bug turned out to be duplicated across five different "build a `Vec<CardItem>`, push it to a model" call sites (`poster.rs`'s two poster-decode functions, `movies.rs`'s library-decode function, plus the two already-fixed here) — fixing it once wasn't enough because the underlying mistake (a fresh `ModelRc::new(VecModel::from(...))` with no same-shape check) had been independently reinvented in each file. Now centralized in `main.rs::apply_cards_preserving_identity`, next to `item_to_card_item`/`items_to_model` — any new "refresh this row" code should call it rather than writing the check again. **Phase 97 note:** even after fixing all five, the library grid *still* flashed — `browse::refresh_library_display` builds `library-display` (the model the grid actually renders) as its own independent derived view over `all_movies`/etc, via sort+filter+search, and had never been touched — so `all_movies` itself was correctly preserved while the thing on screen was rebuilt from scratch anyway. **Lesson generalized:** when a value flows through more than one derived/rendered model (a backing list *and* a separately-sorted/filtered display list), every hop needs the same-shape check independently; fixing the source model doesn't help if a later stage still does a blind rebuild. **Phase 98 note:** `apply_cards_preserving_identity`'s same-shape check only stops Slint from destroying/recreating card *elements* — it says nothing about whether the `Image` *value* newly bound to `source:` is the same object as before. `movies.rs::push_library_cards` and `poster.rs`'s two poster-decode functions always built a fresh `slint::Image::from_rgba8(...)` from newly-decoded bytes regardless of whether the row already had one, so even a `same_shape=true` apply still swapped every visible card's texture in one synchronous batch — visibly disruptive on a large grid with zero element recreation involved. Fix: look up the existing `CardItem` by id first and reuse its `poster` handle directly whenever `has_poster` is already true; only decode a new `Image` for a row that didn't have one. Applies whenever a function *produces* a decoded image (not just merges one that's already decoded, which `refresh_row_preserving_posters`/`refresh_favorites` already did correctly from the start). **Phase 99 note:** even with identity and Image-handle reuse both fixed, Movies/Collections/Music still flashed while TV never did — traced (via `#[track_caller]` added to `refresh_library_display` and `update_library_filter`, logging which of ~15 call sites fired) to `movies.rs::push_library_cards` setting `library-display` **directly** to its just-applied model — which is in raw network/decode order — instead of routing through `browse::refresh_library_display`'s sort/filter/query pipeline like `poster.rs::push_decoded_series` (TV's equivalent) always correctly did. The grid briefly showed Jellyfin's own raw order, indistinguishable from sorted for most items, then visibly reshuffled the next time anything called `refresh_library_display` for real (a nav double-click's `library-search-clear`, a sort-bar apply) — because Jellyfin's ordering and Rust's `.to_lowercase()` title comparator disagree on at least one tied pair (articles/symbols/accents), and stable-sort tie-breaking over a different starting order shifts more than just that one item. **Lesson generalized further:** identity-preservation (Phase 96) and value-reuse (Phase 98) both assume the *content* being applied is already correct — neither one catches a shortcut that skips the actual sort/filter computation and substitutes a merely-similar-looking order instead. When two code paths are supposed to produce "the same" derived view, one of them taking a shortcut that happens to usually agree with the other is worse than both doing the full computation, because the disagreement only surfaces as an intermittent, hard-to-repro flash rather than a compile error or an always-wrong result. **Phase 100 note:** immediately after Phase 99, a new report — Collections' unplayed-count badge flashed in then vanished. `main.rs::item_to_card_item` correctly copies `unplayed_count` from `MediaItem.user_data.unplayed_item_count` (Jellyfin populates this for any folder-like item, BoxSets included, not just Series), so `refresh_row_preserving_posters`' metadata-merge pass showed the badge correctly. But `movies.rs`'s `meta`/`decoded` tuples (the *other* landing point, firing moments later once posters finish decoding) never carried an unplayed-count field at all — `CardItem::default()` leaves it 0 — so that pass's same-shape mutate-in-place `set_row_data` overwrote the just-correct value back to 0, even though nothing about row identity had changed. `poster.rs::push_decoded_series` (TV) never had this gap since its tuple already threaded the field through. **Same lesson as Phase 99, one field lower:** it's not enough for two code paths to agree on *whether* to rebuild vs. mutate-in-place (Phase 96) or *which* Image/order to use (Phases 98-99) — every field on the shared struct has to actually be threaded through both paths, or the one that drops a field will periodically stomp the one that doesn't, and it'll look like a display bug when it's actually a data-plumbing gap.

**`vertical-alignment: bottom` combined with `overflow: elide` + `wrap: word-wrap` mis-measures text height in Slint 1.16.1, as soon as the text actually wraps to 2+ lines — regardless of whether truncation is needed.** `MediaCard`'s title `Text` (widgets.slint) originally used a fixed 34px (2-line) box with default (top) alignment; a short, 1-line title left blank space at the bottom of that box before the subtitle/year started. Bottom-aligning the title (`vertical-alignment: bottom`) fixed that specific case. But once a *long* title wrapped to 2 real lines — with or without a 3rd line being elided away — the same gap reappeared, and only for some titles, which made it look content-dependent rather than systemic. Root-caused with a disposable standalone Slint crate outside the repo (pinned to the exact `slint = "=1.16.1"` from this project's `Cargo.lock`, since the bug does *not* reproduce on 1.17.1 — the two versions clearly handle this differently) reproducing the two real offending titles pulled straight from `~/.cache/fjord/*.json` (`Chillin' in Another World With Level 2 Super Cheat Powers`, `Your Friendly Neighborhood Spider-Man`) at the real card width, with `spectacle -b -n -a -o <path>` (KDE's screenshot CLI) to inspect the rendered output directly — confirmed that `vertical-alignment: top` closes the gap correctly (its baseline is hardcoded to `Font::Length::zero()` in `i-slint-core`'s `textlayout.rs`, no text-height computation involved at all) while `bottom` does not, even though both are given an identical 2-line-filling box. **Fix (and a design change in the same pass, per user preference discovered along the way — don't cap the title at all, always show it in full):** dropped the fixed height / `overflow: elide` / bottom-alignment approach entirely. Both the title and subtitle `Text` elements are now plain `wrap: word-wrap` with no explicit height, so they grow to however many lines they actually need (no more silent truncation of long titles or long episode names) and default to top alignment, which is immune to this bug by construction. `poster-rect` reads the sibling text block's `preferred-height` (named `text-col`) and reserves exactly that much space (`max(0px, root.height - 12px - text-col.preferred-height)`), so the poster shrinks — down to fully hidden in a pathological case — rather than the text ever overflowing the card's own bounds. This still reads `root.height` only from a plain `Rectangle` (the established-safe pattern) and reads a *sibling's* `preferred-height`, not an ancestor's layout cache, so it doesn't reintroduce the `LibraryGrid` "Recursion detected" crash from the gotcha above — re-verified with the same disposable-crate technique, instantiating the equivalent structure both with an explicit height (`SectionRow`-style) and inside a `HorizontalLayout` row with no explicit height (`LibraryGrid`-style, the historically vulnerable case) plus a hover-triggered focus ring (the exact mechanism of the earlier real crash): no panic either way. **General lesson:** when a `vertical-alignment` fix works for the simple case (single line) but a report comes back claiming the *same symptom* on a different, more complex input, don't assume it's the same bug with a missed edge case — reproduce the exact input in isolation before re-patching, since here the "same-looking" gap on wrapped titles had a completely different (and version-specific) root cause from the one-line case, and no amount of staring at the single-line fix would have surfaced it.

**A stray trailing newline/whitespace in Jellyfin's scraped `Overview` metadata inflates `StorylineSection`'s expanded-box height beyond what the visible text needs — this is correct Slint behavior on unclean input data, not a rendering bug.** User report: one specific series' expanded overview left a visible gap between the last line of text and the season tabs below it; every other item's overview expanded correctly with no gap, which pointed at that item's data rather than the shared `StorylineSection` component (confirmed by a standalone repro of the component in isolation with the exact on-screen synopsis text — no gap, box tightly wraps the text). `wrap: word-wrap` `Text` respects embedded `\n` as a real line break, so a trailing `\n` or `\n\n` left over from whatever scraped `Overview` from TMDb/TVDB (or a similar provider) creates one or more genuinely blank lines at the end of the string — invisible (nothing to render), but still counted in `txt.preferred-height`, which is exactly what `StorylineSection`'s expanded height binds to (`max(txt.preferred-height, 66px)`). No code path anywhere trims this field before it reaches `AppState` — `.overview.clone().unwrap_or_default()` was used verbatim at every call site. **Fix:** added `.trim()` at all 8 sites across `detail.rs`, `series.rs` (×2), `season.rs`, `artist.rs`, `album.rs`, `collection.rs`, `person.rs` (bio) — the general fix (sanitize the data once, at the boundary where it enters the app) rather than a per-item or per-screen special case, consistent with how every other `StorylineSection` consumer shares the identical component and should share the identical data-cleanliness guarantee.
