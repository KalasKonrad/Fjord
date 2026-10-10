// ── fjord-app · playback/inhibit.rs ───────────────────────────────────────
//   PlaybackCookies         ScreenSaver cookie + KDE PowerManagement cookie + systemd child
//   inhibit_screensaver     ScreenSaver.Inhibit + KDE PowerManagement.Inhibit + systemd-inhibit child
//   uninhibit_screensaver   release all three (KDE/systemd no-op when unavailable)
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// ── screensaver + display inhibitor ──────────────────────────────────────────

// Holds cookies from both the freedesktop ScreenSaver inhibitor and the KDE
// PowerManagement inhibitor.  Either may be None if the call is unavailable
// (e.g. not running under KDE, or busctl absent).
#[derive(Default)]
pub(crate) struct PlaybackCookies {
    freedesktop: Option<u32>,
    kde_power: Option<u32>,
    // systemd-logind inhibitor (idle + sleep) held open as a child process.
    // Covers sleep/suspend on GNOME, XFCE, and any systemd-based DE that is
    // not KDE (KDE sleep is already covered by kde_power above).
    systemd_child: Option<std::process::Child>,
}

fn busctl_inhibit(service: &str, path: &str, interface: &str, label: &str) -> Option<u32> {
    let out = std::process::Command::new("busctl")
        .args([
            "call",
            "--session",
            service,
            path,
            interface,
            "Inhibit",
            "ss",
            "Fjord",
            "Video playback",
        ])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let cookie = stdout
        .trim()
        .strip_prefix("u ")
        .and_then(|s| s.parse().ok());
    if let Some(c) = cookie {
        info!("{} inhibited (cookie={})", label, c);
    } else {
        debug!("{} inhibit unavailable", label);
    }
    cookie
}

fn busctl_uninhibit(service: &str, path: &str, interface: &str, cookie: u32, label: &str) {
    let _ = std::process::Command::new("busctl")
        .args([
            "call",
            "--session",
            service,
            path,
            interface,
            "UnInhibit",
            "u",
            &cookie.to_string(),
        ])
        .status();
    info!("{} uninhibited (cookie={})", label, cookie);
}

fn inhibit_systemd_sleep() -> Option<std::process::Child> {
    // systemd-logind inhibitor: holds an fd open via a long-lived child process.
    // Blocks idle + sleep on any systemd-based DE (GNOME, XFCE, Cinnamon, …).
    // KDE sleep is already covered by the KDE PowerManagement inhibitor above.
    match std::process::Command::new("systemd-inhibit")
        .args([
            "--what=idle:sleep",
            "--who=Fjord",
            "--why=Video playback",
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
    PlaybackCookies {
        freedesktop: busctl_inhibit(
            "org.freedesktop.ScreenSaver",
            "/org/freedesktop/ScreenSaver",
            "org.freedesktop.ScreenSaver",
            "ScreenSaver",
        ),
        kde_power: busctl_inhibit(
            "org.kde.PowerManagement",
            "/org/kde/PowerManagement/Inhibit",
            "org.kde.PowerManagement.Inhibition",
            "KDE PowerManagement",
        ),
        systemd_child: inhibit_systemd_sleep(),
    }
}

pub(crate) fn uninhibit_screensaver(mut cookies: PlaybackCookies) {
    if let Some(c) = cookies.freedesktop {
        busctl_uninhibit(
            "org.freedesktop.ScreenSaver",
            "/org/freedesktop/ScreenSaver",
            "org.freedesktop.ScreenSaver",
            c,
            "ScreenSaver",
        );
    }
    if let Some(c) = cookies.kde_power {
        busctl_uninhibit(
            "org.kde.PowerManagement",
            "/org/kde/PowerManagement/Inhibit",
            "org.kde.PowerManagement.Inhibition",
            c,
            "KDE PowerManagement",
        );
    }
    if let Some(mut child) = cookies.systemd_child.take() {
        child.kill().ok();
        child.wait().ok();
        info!("systemd sleep inhibitor released");
    }
}
