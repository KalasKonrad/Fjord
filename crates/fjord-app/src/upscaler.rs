// ── fjord-app · upscaler.rs ──────────────────────────────────────────────────
//   Settings → Display → Upscaler: how mpv scales the video up when Fjord draws it larger than
//   it is ("default" = mpv's lanczos, "sharp" = ewa_lanczossharp, "fsr" = AMD FidelityFX Super
//   Resolution 1). With display sync matching the TV to the video's resolution nothing is scaled
//   here — the TV upscales.
//   FSR_GLSL         assets/shaders/FSR.glsl — FSR v1.0.2 for mpv (MIT, AMD; ported by agyild)
//   install_shaders  writes it twice into the cache (as is, and with FSR_PQ 1 for HDR/PQ video),
//                    once per run, off the UI thread → FSR_PATHS
//   pq_copy          the shader with FSR_PQ 1 — unit-tested
//   scale_for        the setting → mpv's `scale` ("" = mpv's default) — unit-tested
//   fsr_for          the setting → the shader paths for PlayerConfig.fsr_shaders (None before
//                    they're written or for another setting)
// ─────────────────────────────────────────────────────────────────────────────
use std::path::PathBuf;
use std::sync::OnceLock;

use tracing::{debug, warn};

const FSR_GLSL: &str = include_str!("../../../assets/shaders/FSR.glsl");

static FSR_PATHS: OnceLock<(String, String)> = OnceLock::new();

/// The shader for PQ (HDR) video: FSR_PQ is defined once per pass.
fn pq_copy(shader: &str) -> String {
    shader.replace("#define FSR_PQ 0", "#define FSR_PQ 1")
}

pub(crate) fn scale_for(upscaler: &str) -> &'static str {
    match upscaler {
        "sharp" => "ewa_lanczossharp",
        _ => "",
    }
}

pub(crate) fn fsr_for(upscaler: &str) -> Option<(String, String)> {
    if upscaler != "fsr" {
        return None;
    }
    let paths = FSR_PATHS.get().cloned();
    if paths.is_none() {
        debug!("upscaler: FSR chosen but its shader files aren't written yet — default scaler");
    }
    paths
}

/// Write the two shader files into `<cache>/shaders/` (on its own thread at startup).
pub(crate) fn install_shaders(cache_dir: PathBuf) {
    let spawned = std::thread::Builder::new()
        .name("shaders".into())
        .spawn(move || {
            let dir = cache_dir.join("shaders");
            let sdr = dir.join("fsr.glsl");
            let pq = dir.join("fsr-pq.glsl");
            let written = std::fs::create_dir_all(&dir)
                .and_then(|_| std::fs::write(&sdr, FSR_GLSL))
                .and_then(|_| std::fs::write(&pq, pq_copy(FSR_GLSL)));
            match written {
                Ok(()) => {
                    let _ = FSR_PATHS.set((
                        sdr.to_string_lossy().into_owned(),
                        pq.to_string_lossy().into_owned(),
                    ));
                }
                Err(e) => warn!(
                    "upscaler: couldn't write the FSR shader to {}: {e}",
                    dir.display()
                ),
            }
        });
    if let Err(e) = spawned {
        warn!("upscaler: couldn't start the shader thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pq_copy_sets_both_passes_to_pq() {
        assert_eq!(FSR_GLSL.matches("#define FSR_PQ 0").count(), 2);
        let pq = pq_copy(FSR_GLSL);
        assert_eq!(pq.matches("#define FSR_PQ 1").count(), 2);
        assert!(!pq.contains("#define FSR_PQ 0"));
    }

    #[test]
    fn each_choice_maps_to_mpv() {
        assert_eq!(scale_for("default"), "");
        assert_eq!(scale_for("sharp"), "ewa_lanczossharp");
        assert_eq!(
            scale_for("fsr"),
            "",
            "FSR keeps mpv's scaler for chroma and beyond 4x"
        );
        assert_eq!(fsr_for("sharp"), None);
    }
}
