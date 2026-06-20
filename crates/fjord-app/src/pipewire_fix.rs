// ── fjord-app · pipewire_fix.rs ──────────────────────────────────────────────
//   is_pipewire_device          true for "" (auto) or pipewire / pipewire/* names
//   apply_alsa_irq_scheduling   write/delete WirePlumber config + restart wireplumber
//                               (persistent: survives Fjord exit until toggled off)
// ─────────────────────────────────────────────────────────────────────────────

const CONF_FILENAME: &str = "fjord-alsa-irq.conf";

const CONF_CONTENT: &str = "\
# Written by Fjord — Settings → Audio → PipeWire IRQ scheduling.
# WARNING: This file is managed by Fjord. Do not edit it manually.
# Toggling the setting off in Fjord will DELETE this file entirely,
# discarding any changes you have made here.
monitor.alsa.rules = [
  {
    matches = [{ node.name = \"~alsa_output.*\" }]
    actions = {
      update-props = {
        api.alsa.disable-tsched = true
      }
    }
  }
]
";

fn conf_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
        });
    base.join("wireplumber").join("wireplumber.conf.d").join(CONF_FILENAME)
}

/// Returns true when the selected mpv audio device goes through PipeWire.
/// Empty string = "auto", which on modern Linux defaults to PipeWire.
pub(crate) fn is_pipewire_device(device: &str) -> bool {
    device.is_empty() || device == "pipewire" || device.starts_with("pipewire/")
}

/// Enable or disable hardware IRQ scheduling for ALSA output nodes.
///
/// When `enable` is true, writes a WirePlumber rule that sets
/// `api.alsa.disable-tsched = true` for all `alsa_output.*` nodes, then
/// restarts WirePlumber so the rule takes effect.  When false, deletes the
/// rule file and restarts WirePlumber to restore the default (software timer).
///
/// The config persists after Fjord exits.  It is only removed when the toggle
/// is explicitly switched off.  A ~1 s audio gap occurs during WirePlumber
/// restart.
pub(crate) fn apply_alsa_irq_scheduling(enable: bool) {
    let path = conf_path();

    if enable {
        if let Some(dir) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                tracing::warn!("could not create wireplumber config dir: {e}");
                return;
            }
        }
        if path.exists() {
            // Config already in place — nothing to change, skip the restart.
            tracing::info!("alsa IRQ scheduling: config already present, skipping restart");
            return;
        }
        if let Err(e) = std::fs::write(&path, CONF_CONTENT) {
            tracing::warn!("could not write wireplumber config: {e}");
            return;
        }
        tracing::info!("alsa IRQ scheduling: wrote {}", path.display());
    } else {
        if !path.exists() {
            tracing::info!("alsa IRQ scheduling: config not present, nothing to remove");
            return;
        }
        if let Err(e) = std::fs::remove_file(&path) {
            tracing::warn!("could not remove wireplumber config: {e}");
            return;
        }
        tracing::info!("alsa IRQ scheduling: removed {}", path.display());
    }

    restart_wireplumber();
}

fn restart_wireplumber() {
    let out = std::process::Command::new("systemctl")
        .args(["--user", "restart", "wireplumber"])
        .output();
    match out {
        Ok(o) if o.status.success() => tracing::info!("wireplumber restarted"),
        Ok(o) => tracing::warn!(
            "wireplumber restart failed: {}",
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) => tracing::warn!("could not run systemctl: {e}"),
    }
}
