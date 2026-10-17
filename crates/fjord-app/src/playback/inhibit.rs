// ── fjord-app · playback/inhibit.rs ───────────────────────────────────────
//   PlaybackCookies         the held session-bus connection + ScreenSaver / PowerManagement cookies +
//                           the systemd-inhibit child (zbus since 2026-10-17: busctl never worked)
//   inhibit_screensaver     org.freedesktop.ScreenSaver.Inhibit (screen blanking/DPMS) +
//                           org.freedesktop.PowerManagement.Inhibit (sleep) on one held connection +
//                           systemd-inhibit idle:sleep child
//   uninhibit_screensaver   UnInhibit both, drop the connection, end the child (no-op when unavailable)
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// ── screensaver + display inhibitor ──────────────────────────────────────────

// What keeps the screen on and the machine awake while something plays (also while Fjord's own
// screensaver shows black — the TV must not lose its HDMI signal). The desktop's ScreenSaver and
// PowerManagement inhibitions are tied to the D-Bus connection that asked for them (KWin/
// PowerDevil and GNOME release them when it closes), so `bus` is held for the whole playback;
// dropping it releases both. systemd-logind's idle+sleep inhibitor is a child process.
#[derive(Default)]
pub(crate) struct PlaybackCookies {
    bus: Option<zbus::blocking::Connection>,
    screensaver: Option<u32>,
    power: Option<u32>,
    systemd_child: Option<std::process::Child>,
}

const SCREENSAVER: (&str, &str, &str) = (
    "org.freedesktop.ScreenSaver",
    "/org/freedesktop/ScreenSaver",
    "org.freedesktop.ScreenSaver",
);
const POWER: (&str, &str, &str) = (
    "org.freedesktop.PowerManagement",
    "/org/freedesktop/PowerManagement/Inhibit",
    "org.freedesktop.PowerManagement.Inhibit",
);

fn dbus_inhibit(
    bus: &zbus::blocking::Connection,
    (service, path, iface): (&str, &str, &str),
) -> Option<u32> {
    match bus
        .call_method(
            Some(service),
            path,
            Some(iface),
            "Inhibit",
            &("Fjord", "Playback"),
        )
        .and_then(|reply| reply.body().deserialize::<u32>())
    {
        Ok(cookie) => {
            info!("{iface} inhibited (cookie={cookie})");
            Some(cookie)
        }
        Err(e) => {
            debug!("{iface} inhibit unavailable: {e}");
            None
        }
    }
}

fn dbus_uninhibit(
    bus: &zbus::blocking::Connection,
    (service, path, iface): (&str, &str, &str),
    cookie: u32,
) {
    match bus.call_method(Some(service), path, Some(iface), "UnInhibit", &(cookie,)) {
        Ok(_) => info!("{iface} uninhibited (cookie={cookie})"),
        Err(e) => debug!("{iface} uninhibit failed (released with the connection): {e}"),
    }
}

fn inhibit_systemd_sleep() -> Option<std::process::Child> {
    // systemd-logind inhibitor: holds an fd open via a long-lived child process.
    // Blocks idle + sleep on any systemd-based desktop.
    match std::process::Command::new("systemd-inhibit")
        .args([
            "--what=idle:sleep",
            "--who=Fjord",
            "--why=Playback",
            "--mode=block",
            "sleep",
            "infinity",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => {
            info!("systemd sleep inhibited (pid={})", child.id());
            Some(child)
        }
        Err(e) => {
            debug!("systemd-inhibit unavailable: {}", e);
            None
        }
    }
}

pub(crate) fn inhibit_screensaver() -> PlaybackCookies {
    let bus = match zbus::blocking::Connection::session() {
        Ok(bus) => Some(bus),
        Err(e) => {
            debug!("session bus unavailable, no screen-saver inhibit: {e}");
            None
        }
    };
    let screensaver = bus.as_ref().and_then(|b| dbus_inhibit(b, SCREENSAVER));
    let power = bus.as_ref().and_then(|b| dbus_inhibit(b, POWER));
    PlaybackCookies {
        bus,
        screensaver,
        power,
        systemd_child: inhibit_systemd_sleep(),
    }
}

pub(crate) fn uninhibit_screensaver(mut cookies: PlaybackCookies) {
    if let Some(bus) = cookies.bus.take() {
        if let Some(c) = cookies.screensaver {
            dbus_uninhibit(&bus, SCREENSAVER, c);
        }
        if let Some(c) = cookies.power {
            dbus_uninhibit(&bus, POWER, c);
        }
    }
    if let Some(mut child) = cookies.systemd_child.take() {
        child.kill().ok();
        child.wait().ok();
        info!("systemd sleep inhibitor released");
    }
}
