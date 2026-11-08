// ── fjord-app · appearance.rs ────────────────────────────────────────────────
//   spawn_watch      the desktop's light/dark preference for Settings → Interface → Theme = "Like
//                    the desktop": reads the freedesktop portal's org.freedesktop.appearance
//                    color-scheme (ReadOne, or the older Read) on its own thread and follows its
//                    SettingChanged signal → AppState.system-prefers-light (Theme.system-light).
//                    No portal = stays dark; logged at debug
//   prefers_light    color-scheme value → light? (2 = prefer light; 1 = dark, 0 = no preference;
//                    the older Read wraps it in a second variant) — unit-tested
// ─────────────────────────────────────────────────────────────────────────────
use slint::Global;
use tracing::debug;
use zbus::zvariant::{OwnedValue, Value};

use crate::{AppState, MainWindow};

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";

pub(crate) fn spawn_watch(window: slint::Weak<MainWindow>) {
    let spawned = std::thread::Builder::new()
        .name("appearance".into())
        .spawn(move || {
            if let Err(e) = watch(&window) {
                debug!("appearance: the desktop's color scheme isn't available ({e}) — stays dark");
            }
        });
    if let Err(e) = spawned {
        debug!("appearance: couldn't start its thread: {e}");
    }
}

/// The portal's color-scheme: 2 = prefer light; 1 = prefer dark, 0 = no preference (→ dark).
fn prefers_light(value: &Value<'_>) -> bool {
    match value {
        Value::U32(n) => *n == 2,
        Value::Value(inner) => prefers_light(inner),
        _ => false,
    }
}

fn apply(window: &slint::Weak<MainWindow>, light: bool) {
    debug!(
        "appearance: the desktop prefers {}",
        if light { "light" } else { "dark" }
    );
    let window = window.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = window.upgrade() {
            AppState::get(&w).set_system_prefers_light(light);
        }
    });
}

fn watch(window: &slint::Weak<MainWindow>) -> zbus::Result<()> {
    let bus = zbus::blocking::Connection::session()?;
    let read = |method: &str| -> zbus::Result<OwnedValue> {
        let reply = bus.call_method(
            Some(PORTAL),
            PORTAL_PATH,
            Some(SETTINGS),
            method,
            &(NAMESPACE, KEY),
        )?;
        reply.body().deserialize::<OwnedValue>()
    };
    // ReadOne is portal version 2; Read (deprecated) wraps the value in one more variant.
    let value = read("ReadOne").or_else(|_| read("Read"))?;
    apply(window, prefers_light(&value));

    let proxy = zbus::blocking::Proxy::new(&bus, PORTAL, PORTAL_PATH, SETTINGS)?;
    for message in proxy.receive_signal("SettingChanged")? {
        let Ok((namespace, key, value)) =
            message.body().deserialize::<(String, String, OwnedValue)>()
        else {
            continue;
        };
        if namespace == NAMESPACE && key == KEY {
            apply(window, prefers_light(&value));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::prefers_light;
    use zbus::zvariant::Value;

    #[test]
    fn only_prefer_light_is_light() {
        assert!(prefers_light(&Value::U32(2)));
        assert!(!prefers_light(&Value::U32(1)));
        assert!(!prefers_light(&Value::U32(0)));
        assert!(!prefers_light(&Value::Str("light".into())));
        // The older Read call: the value inside one more variant.
        assert!(prefers_light(&Value::Value(Box::new(Value::U32(2)))));
    }
}
