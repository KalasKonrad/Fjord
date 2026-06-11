use anyhow::{Context, Result};
use libmpv2::{events::Event, FileState, Format, Mpv};
use std::{sync::mpsc, thread};
use tracing::{debug, error, info};

// libmpv2::Error contains Rc<Error> in the Loadfiles variant, so it is !Send + !Sync
// and cannot be used with anyhow::Context directly. Use this helper instead.
fn mpv_err(e: libmpv2::Error, msg: &str) -> anyhow::Error {
    anyhow::anyhow!("{}: {}", msg, e)
}

/// Wraps a libmpv instance and runs its event loop on a background thread.
///
/// Phase 2 uses the "separate window" strategy: mpv opens its own fullscreen
/// window and owns the vsync loop entirely. Default key bindings apply:
/// Space = pause, left/right = seek ±5 s, q/Esc = quit.
pub struct Player {
    done_rx: mpsc::Receiver<Result<()>>,
    _thread: thread::JoinHandle<()>,
}

impl Player {
    /// Create an mpv instance, configure it, and start playing `url`.
    ///
    /// Returns immediately; playback runs on a background thread.
    pub fn play(url: &str) -> Result<Self> {
        let mut mpv = Mpv::with_initializer(|init| {
            // Try hardware decoding (vaapi / nvdec) without risking glitches.
            init.set_option("hwdec", "auto-safe")?;
            // Pass AC3/DTS/TrueHD through to the receiver undecoded.
            init.set_option("audio-spdif", "ac3,dts,truehd")?;
            // Always open fullscreen so mpv covers the app window while playing.
            init.set_option("fs", true)?;
            Ok(())
        })
        .map_err(|e| mpv_err(e, "mpv init failed"))?;

        mpv.event_context_mut()
            .observe_property("vsync-ratio", Format::Double, 1)
            .map_err(|e| mpv_err(e, "observe vsync-ratio"))?;

        mpv.playlist_load_files(&[(url, FileState::Replace, None)])
            .map_err(|e| mpv_err(e, "loadfile failed"))?;

        let (done_tx, done_rx) = mpsc::sync_channel(1);

        let handle = thread::spawn(move || {
            info!("mpv event loop started");
            let result = event_loop(mpv);
            if let Err(ref e) = result {
                error!("mpv exited with error: {:#}", e);
            }
            let _ = done_tx.send(result);
        });

        Ok(Self {
            done_rx,
            _thread: handle,
        })
    }

    /// Block until playback finishes (user quits or file ends).
    pub fn wait(self) -> Result<()> {
        self.done_rx.recv().context("player thread disconnected")?
    }
}

fn event_loop(mut mpv: Mpv) -> Result<()> {
    loop {
        match mpv.event_context_mut().wait_event(10.0) {
            Some(Ok(Event::Shutdown)) => {
                info!("mpv: shutdown");
                break;
            }
            Some(Ok(Event::EndFile(reason))) => {
                info!("mpv: end-of-file (reason {:?})", reason);
                break;
            }
            Some(Ok(Event::PropertyChange { name, change, .. })) if name == "vsync-ratio" => {
                debug!("vsync-ratio = {:?}", change);
            }
            Some(Ok(ev)) => debug!("mpv event: {:?}", ev),
            Some(Err(e)) => {
                return Err(anyhow::anyhow!("mpv error event: {:?}", e));
            }
            None => {}
        }
    }
    Ok(())
}
