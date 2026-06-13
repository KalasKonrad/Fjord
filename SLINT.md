# Slint — Best Practices for Fjord

Reference for structuring and writing Slint UI code. Covers file organisation,
globals, property visibility, and patterns that have caused real bugs in this
codebase. Runtime gotchas are also in `CLAUDE.md § Known Slint gotchas`.

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
