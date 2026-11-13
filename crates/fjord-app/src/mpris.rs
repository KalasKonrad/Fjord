// ── fjord-app · mpris.rs ──────────────────────────────────────────────────────
//   MPRIS (org.mpris.MediaPlayer2) on the session bus: the desktop learns what Fjord plays and can
//   control it — KDE routes the media keys (Play/Pause, Next, Previous, Stop) only to MPRIS players,
//   and its media widget, lock screen and KDE Connect use the same interface (9c P3, 2026-11-13).
//   MprisState      what's playing: status, title, artist, album, cover (a file:// URL into the
//                   poster cache — never a server URL, those carry the token), length/position (µs)
//   Command         what the desktop asked for, run on the UI thread by `run` (the keys' callbacks:
//                   video → pause-play-toggle/stop-playback/seek-to, music → music-bar-*; next/previous
//                   → queue-next/prev-track)
//   spawn           its own thread: the connection (name org.mpris.MediaPlayer2.fjord, or
//                   …fjord.instance<pid> when taken) serving Root + Player at /org/mpris/MediaPlayer2;
//                   then it applies each published state and signals what changed (PropertiesChanged
//                   for status/metadata/abilities, Seeked on a jump). No session bus → logged, no MPRIS
//   publish         from the player tick (playback/timer.rs, every ~0.5 s): the current state, cheap
//                   (a channel send; the thread compares)
//   Root / Player   the two interfaces (zbus); Player's methods only hand a Command to the sink
//   metadata_map / seek_target  pure, unit-tested; an in-process peer-to-peer test drives the real
//                   interfaces (no bus daemon)
// ─────────────────────────────────────────────────────────────────────────────
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use slint::Global;
use tracing::{debug, info, warn};
use zbus::interface;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use crate::{AppState, MainWindow};

const PATH: &str = "/org/mpris/MediaPlayer2";
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";
/// A position this much off from where it should be after the last publish is a seek (→ Seeked).
const JUMP_US: i64 = 3_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Status {
    Playing,
    Paused,
    #[default]
    Stopped,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Playing => "Playing",
            Status::Paused => "Paused",
            Status::Stopped => "Stopped",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct MprisState {
    pub status: Status,
    /// The Jellyfin item id (32 hex) — the track id's last part; "" = no track.
    pub track_id: String,
    pub title: String,
    pub artist: Vec<String>,
    pub album: String,
    pub art_url: String,
    pub length_us: i64,
    pub position_us: i64,
    pub volume: f64,
    pub audio: bool,
}

impl MprisState {
    /// The same item and how it's shown (everything but the position).
    fn same_show(&self, other: &MprisState) -> bool {
        MprisState {
            position_us: 0,
            ..self.clone()
        } == MprisState {
            position_us: 0,
            ..other.clone()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Command {
    PlayPause,
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    /// An absolute position in µs, with the length it's a fraction of.
    SeekTo {
        position_us: i64,
        length_us: i64,
    },
}

/// mpris:trackid for an item: an object path (Jellyfin ids are hex, so always valid).
fn track_path(id: &str) -> String {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        NO_TRACK.to_string()
    } else {
        format!("/org/fjord/track/{id}")
    }
}

/// The Metadata property. Unit-tested.
pub(crate) fn metadata_map(s: &MprisState) -> HashMap<String, OwnedValue> {
    let mut m = HashMap::new();
    let mut put = |k: &str, v: Value<'_>| {
        if let Ok(owned) = v.try_to_owned() {
            m.insert(k.to_string(), owned);
        }
    };
    if let Ok(p) = ObjectPath::try_from(track_path(&s.track_id)) {
        put("mpris:trackid", Value::from(p));
    }
    if s.status == Status::Stopped {
        return m;
    }
    if s.length_us > 0 {
        put("mpris:length", Value::from(s.length_us));
    }
    if !s.title.is_empty() {
        put("xesam:title", Value::from(s.title.clone()));
    }
    if !s.artist.is_empty() {
        put("xesam:artist", Value::from(s.artist.clone()));
    }
    if !s.album.is_empty() {
        put("xesam:album", Value::from(s.album.clone()));
    }
    if !s.art_url.is_empty() {
        put("mpris:artUrl", Value::from(s.art_url.clone()));
    }
    m
}

/// Where a relative Seek lands: clamped to the item (a seek past the end goes to it, mpv then
/// ends the file like a key seek would). Unit-tested.
pub(crate) fn seek_target(position_us: i64, offset_us: i64, length_us: i64) -> i64 {
    let to = position_us.saturating_add(offset_us).max(0);
    if length_us > 0 { to.min(length_us) } else { to }
}

type Sink = Arc<dyn Fn(Command) + Send + Sync>;

// ── Root ─────────────────────────────────────────────────────────────────────
struct Root;

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    /// Wayland gives no way to raise a window from outside it — CanRaise is false.
    fn raise(&self) {}
    fn quit(&self) {}
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> &str {
        "Fjord"
    }
    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        "fjord"
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

// ── Player ───────────────────────────────────────────────────────────────────
struct Player {
    state: Arc<Mutex<MprisState>>,
    sink: Sink,
}

impl Player {
    fn now(&self) -> MprisState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn next(&self) {
        (self.sink)(Command::Next);
    }
    fn previous(&self) {
        (self.sink)(Command::Previous);
    }
    fn pause(&self) {
        (self.sink)(Command::Pause);
    }
    fn play_pause(&self) {
        (self.sink)(Command::PlayPause);
    }
    fn stop(&self) {
        (self.sink)(Command::Stop);
    }
    fn play(&self) {
        (self.sink)(Command::Play);
    }
    fn seek(&self, offset: i64) {
        let s = self.now();
        if s.status != Status::Stopped {
            (self.sink)(Command::SeekTo {
                position_us: seek_target(s.position_us, offset, s.length_us),
                length_us: s.length_us,
            });
        }
    }
    /// Ignored unless it names the current track (the spec: a stale track id is a no-op).
    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        let s = self.now();
        if s.status != Status::Stopped
            && track_id.as_str() == track_path(&s.track_id)
            && (0..=s.length_us).contains(&position)
        {
            (self.sink)(Command::SeekTo {
                position_us: position,
                length_us: s.length_us,
            });
        }
    }
    fn open_uri(&self, _uri: &str) {}

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.now().status.as_str().to_string()
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        metadata_map(&self.now())
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.now().volume
    }
    /// Clients poll it (the spec: no change signal; Seeked covers jumps).
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        self.now().position_us
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.now().status != Status::Stopped
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.now().status != Status::Stopped
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.now().status != Status::Stopped
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.now().status != Status::Stopped
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        let s = self.now();
        s.status != Status::Stopped && s.length_us > 0
    }
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

// ── publish / the thread ───────────────────────────────────────────────────────
static TX: OnceLock<Mutex<Sender<MprisState>>> = OnceLock::new();

/// The current state, from the player tick. A no-op until spawn ran (or without a session bus).
pub(crate) fn publish(state: MprisState) {
    if let Some(tx) = TX.get()
        && let Ok(tx) = tx.lock()
    {
        let _ = tx.send(state);
    }
}

/// Start serving MPRIS (once, at startup).
pub(crate) fn spawn(ww: slint::Weak<MainWindow>) {
    let (tx, rx) = std::sync::mpsc::channel::<MprisState>();
    if TX.set(Mutex::new(tx)).is_err() {
        return;
    }
    let sink: Sink = Arc::new(move |cmd| {
        let ww = ww.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww.upgrade() {
                run(&w, cmd);
            }
        });
    });
    let spawned = std::thread::Builder::new()
        .name("mpris".into())
        .spawn(move || serve(rx, sink));
    if let Err(e) = spawned {
        warn!("mpris: couldn't start its thread: {e}");
    }
}

fn connect(
    state: &Arc<Mutex<MprisState>>,
    sink: &Sink,
) -> zbus::Result<zbus::blocking::Connection> {
    let build = |name: String| {
        zbus::blocking::connection::Builder::session()?
            .name(name)?
            .serve_at(PATH, Root)?
            .serve_at(
                PATH,
                Player {
                    state: Arc::clone(state),
                    sink: Arc::clone(sink),
                },
            )?
            .build()
    };
    // Another Fjord already owns the plain name: the spec's per-instance suffix.
    build("org.mpris.MediaPlayer2.fjord".into()).or_else(|e| {
        debug!("mpris: plain name unavailable ({e}) — trying the per-instance one");
        build(format!(
            "org.mpris.MediaPlayer2.fjord.instance{}",
            std::process::id()
        ))
    })
}

fn serve(rx: Receiver<MprisState>, sink: Sink) {
    let state = Arc::new(Mutex::new(MprisState::default()));
    let conn = match connect(&state, &sink) {
        Ok(c) => c,
        Err(e) => {
            warn!(
                "mpris: not available — {e} (media keys and the desktop's media controls won't see Fjord)"
            );
            return;
        }
    };
    info!("mpris: serving at {PATH}");
    let iface = match conn.object_server().interface::<_, Player>(PATH) {
        Ok(i) => i,
        Err(e) => {
            warn!("mpris: player interface missing — {e}");
            return;
        }
    };
    while let Ok(mut next) = rx.recv() {
        // Only the newest state matters.
        while let Ok(newer) = rx.try_recv() {
            next = newer;
        }
        let prev = {
            let Ok(mut s) = state.lock() else { return };
            std::mem::replace(&mut *s, next.clone())
        };
        let emitter = iface.signal_emitter();
        let player = iface.get();
        if !prev.same_show(&next) {
            debug!(
                "mpris: {} — {:?}",
                next.status.as_str(),
                if next.title.is_empty() {
                    "(nothing)"
                } else {
                    next.title.as_str()
                }
            );
            let sent = zbus::block_on(async {
                player.playback_status_changed(emitter).await?;
                player.metadata_changed(emitter).await?;
                player.volume_changed(emitter).await?;
                player.can_go_next_changed(emitter).await?;
                player.can_go_previous_changed(emitter).await?;
                player.can_play_changed(emitter).await?;
                player.can_pause_changed(emitter).await?;
                player.can_seek_changed(emitter).await
            });
            if let Err(e) = sent {
                debug!("mpris: change signal failed — {e}");
            }
        }
        // A jump (a seek from Fjord's own keys, a new item's resume point) → Seeked, so clients
        // re-read the position. ~0.5 s between publishes: playing moves it by about that much.
        let expected = if prev.status == Status::Playing {
            prev.position_us + 500_000
        } else {
            prev.position_us
        };
        if next.status != Status::Stopped && (next.position_us - expected).abs() > JUMP_US {
            let _ = zbus::block_on(Player::seeked(emitter, next.position_us));
        }
    }
}

// ── commands on the UI thread ───────────────────────────────────────────────────
/// A desktop command, as the keys would do it: music through the music bar's callbacks, video
/// through the player's.
pub(crate) fn run(w: &MainWindow, cmd: Command) {
    let g = AppState::get(w);
    let audio = g.get_is_audio_playing();
    let paused = if audio {
        g.get_music_bar_paused()
    } else {
        g.get_is_paused()
    };
    debug!("mpris: {cmd:?} (audio={audio}, paused={paused})");
    let toggle = || {
        if audio {
            g.invoke_music_bar_play_pause();
        } else {
            g.invoke_pause_play_toggle();
        }
    };
    match cmd {
        Command::PlayPause => toggle(),
        Command::Play if paused => toggle(),
        Command::Pause if !paused => toggle(),
        Command::Play | Command::Pause => {}
        Command::Stop if audio => g.invoke_music_bar_stop(),
        Command::Stop => g.invoke_stop_playback(),
        Command::Next => g.invoke_queue_next_track(),
        Command::Previous => g.invoke_queue_prev_track(),
        Command::SeekTo {
            position_us,
            length_us,
        } => {
            if length_us <= 0 {
                return;
            }
            let ratio = (position_us as f64 / length_us as f64).clamp(0.0, 1.0) as f32;
            if audio {
                g.invoke_music_bar_seek(ratio);
            } else {
                g.invoke_seek_to(ratio);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing() -> MprisState {
        MprisState {
            status: Status::Playing,
            track_id: "0123456789abcdef0123456789abcdef".into(),
            title: "A song".into(),
            artist: vec!["An artist".into()],
            album: "An album".into(),
            art_url: "file:///tmp/cover".into(),
            length_us: 200_000_000,
            position_us: 10_000_000,
            volume: 0.8,
            audio: true,
        }
    }

    #[test]
    fn metadata_names_the_track() {
        let m = metadata_map(&playing());
        assert_eq!(
            m.get("xesam:title")
                .and_then(|v| String::try_from(v.clone()).ok())
                .as_deref(),
            Some("A song")
        );
        assert_eq!(
            m.get("mpris:length").and_then(|v| i64::try_from(v).ok()),
            Some(200_000_000)
        );
        assert!(m.contains_key("mpris:trackid") && m.contains_key("mpris:artUrl"));
        // Stopped: only the "no track" id.
        let stopped = metadata_map(&MprisState::default());
        assert_eq!(stopped.len(), 1);
        assert_eq!(track_path(""), NO_TRACK);
    }

    #[test]
    fn a_seek_stays_inside_the_item() {
        assert_eq!(seek_target(10_000_000, 5_000_000, 200_000_000), 15_000_000);
        assert_eq!(seek_target(10_000_000, -60_000_000, 200_000_000), 0);
        assert_eq!(
            seek_target(190_000_000, 60_000_000, 200_000_000),
            200_000_000
        );
    }

    /// The real interfaces over an in-process peer-to-peer connection (no bus daemon): properties
    /// read back, methods reach the sink.
    #[test]
    fn the_interfaces_answer_a_client() {
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let state = Arc::new(Mutex::new(playing()));
        let got: Arc<Mutex<Vec<Command>>> = Arc::default();
        let got2 = Arc::clone(&got);
        let sink: Sink = Arc::new(move |c| got2.lock().unwrap().push(c));
        let guid = zbus::Guid::generate();
        let server = std::thread::spawn({
            let state = Arc::clone(&state);
            move || {
                zbus::blocking::connection::Builder::unix_stream(a)
                    .server(guid)
                    .unwrap()
                    .p2p()
                    .serve_at(PATH, Root)
                    .unwrap()
                    .serve_at(PATH, Player { state, sink })
                    .unwrap()
                    .build()
                    .unwrap()
            }
        });
        let client = zbus::blocking::connection::Builder::unix_stream(b)
            .p2p()
            .build()
            .unwrap();
        let _server = server.join().unwrap();
        let player = zbus::blocking::Proxy::new(
            &client,
            "org.mpris.MediaPlayer2.fjord",
            PATH,
            "org.mpris.MediaPlayer2.Player",
        )
        .unwrap();
        let status: String = player.get_property("PlaybackStatus").unwrap();
        assert_eq!(status, "Playing");
        let pos: i64 = player.get_property("Position").unwrap();
        assert_eq!(pos, 10_000_000);
        let meta: HashMap<String, OwnedValue> = player.get_property("Metadata").unwrap();
        assert!(meta.contains_key("xesam:title"));
        player.call_method("PlayPause", &()).unwrap();
        player.call_method("Seek", &(5_000_000i64)).unwrap();
        let root = zbus::blocking::Proxy::new(
            &client,
            "org.mpris.MediaPlayer2.fjord",
            PATH,
            "org.mpris.MediaPlayer2",
        )
        .unwrap();
        let identity: String = root.get_property("Identity").unwrap();
        assert_eq!(identity, "Fjord");
        assert_eq!(
            *got.lock().unwrap(),
            vec![
                Command::PlayPause,
                Command::SeekTo {
                    position_us: 15_000_000,
                    length_us: 200_000_000
                }
            ]
        );
    }
}
