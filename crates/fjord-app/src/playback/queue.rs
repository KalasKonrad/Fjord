// ── fjord-app · playback/queue.rs ─────────────────────────────────────────
//   QueueItem               { id, item_type, series_id, title, audio_meta } — one entry in the playback queue
//   RepeatMode              Off / All / One — queue repeat behaviour
//   repeat_one_target       Repeat One replays now_playing (the song actually playing), with or without a
//                           playlist — used by natural end, the gapless peek, and commit_natural_next
//   repeat_all_ring /       Repeat All with no album playlist: the queue is a ring (ended song → back of
//     take_repeat_all_ring_next  the queue, head plays) — natural end, gapless peek/commit and ⏭ Next
//   upcoming_count          queue-count definition: playlist tracks after current + queue items
//   playlist_prev/_next, peek_natural_next/commit_natural_next, invalidate_preload,
//   rebuild_shuffle_order/toggle_shuffle, shuffle_indices — playlist + queue stepping (every advance:
//                           gapless, a song's normal end, ⏭)
//   first_queued_song / join_playlist  songs queued during an album/playlist play before it continues
//                           and join it as they start (Repeat All loops them, ⏮ goes back past them)
//   move_playlist_to        a new shuffle round (Repeat All) reshuffles — joined songs land anywhere
//   resolve_true_next_episode  the ONLY resolver for "what's next" auto-advance — trusts
//                           /Shows/NextUp's answer only when verifiably forward of the current
//                           episode (position check against the series' own ordered list),
//                           else falls back to strict position+1; NextUp alone is unreliable at
//                           an episode boundary (returns the current episode before its stop/
//                           played report lands, or a rewatch suggestion once fully watched,
//                           e.g. right after the credits-trigger mark above) but blindly
//                           ignoring it entirely throws away its watched-state awareness for
//                           legitimate skip-ahead cases (e.g. an episode already watched
//                           from another client)
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// ── RepeatMode ────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub(crate) enum RepeatMode {
    #[default]
    Off = 0,
    All = 1,
    One = 2,
}

// ── QueueItem ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub(crate) struct QueueItem {
    pub id: String,
    pub item_type: String,
    pub series_id: Option<String>,
    pub title: String,
    pub audio_meta: Option<(String, String)>, // (artist, album_art_id)
}

// ── upcoming_count ────────────────────────────────────────────────────────────
// Single definition of what queue-count means: tracks still ahead in the
// playlist (after the current one) plus all context-menu queue items (CR10-6).
pub(crate) fn upcoming_count(vs: &VideoState) -> i32 {
    let ahead = if vs.playlist.is_empty() {
        0
    } else {
        vs.playlist.len().saturating_sub(vs.playlist_index + 1)
    };
    (ahead + vs.queue.len()) as i32
}

// ── shuffle_indices ───────────────────────────────────────────────────────────
// LCG Fisher-Yates shuffle of 0..n into a Vec<usize>.
pub(crate) fn shuffle_indices(n: usize) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..n).collect();
    if n <= 1 {
        return indices;
    }
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(12345);
    let mut rng = seed;
    for i in (1..n).rev() {
        rng = rng
            .wrapping_mul(6364136223846793005u64)
            .wrapping_add(1442695040888963407u64);
        let j = (rng >> 33) as usize % (i + 1);
        indices.swap(i, j);
    }
    indices
}

// ── playlist_prev / playlist_next ────────────────────────────────────────────
// Called by queue-prev-track / queue-next-track callbacks in main.rs.
// prev: if pos < 2 s and index > 0 → go back; else restart current.
// next: advance to next in playlist (or queue if no playlist).
// Returns Some(QueueItem) if a new item should start; None if nothing to do.

pub(crate) fn playlist_prev(vs: &mut VideoState) -> Option<QueueItem> {
    if vs.playlist.is_empty() {
        return None;
    }
    let pos = vs.player.as_ref().map(|p| p.get_position()).unwrap_or(0.0);
    // Deep into the track: restart it (caller seeks to 0 on None).
    if pos >= 2.0 {
        return None;
    }
    if vs.shuffle && !vs.shuffle_order.is_empty() {
        // "Has a previous track" must be judged in SHUFFLE order — the old
        // playlist_index > 0 gate meant prev did nothing whenever the current
        // track happened to be playlist index 0, regardless of its shuffle
        // position; and shuffle position 0 restarted the track via a full
        // start_playback instead of a seek.
        let cur_pos = vs
            .shuffle_order
            .iter()
            .position(|&i| i == vs.playlist_index)
            .unwrap_or(0);
        if cur_pos == 0 {
            return None;
        } // first shuffled track — nothing to go back to
        let prev_idx = vs.shuffle_order[cur_pos - 1];
        vs.playlist_index = prev_idx;
        Some(vs.playlist[prev_idx].clone())
    } else {
        if vs.playlist_index == 0 {
            return None;
        } // first track — nothing to go back to
        vs.playlist_index -= 1;
        Some(vs.playlist[vs.playlist_index].clone())
    }
}

// The playlist index natural end will move to — mirrors the advance block in
// wire_mpv_timer (RepeatMode::One repeats the CURRENT track, unlike
// playlist_next, which is the ⏭ button and always moves on).
fn natural_next_index(vs: &VideoState) -> Option<usize> {
    let len = vs.playlist.len();
    if len == 0 {
        return None;
    }
    match vs.repeat_mode {
        RepeatMode::One => Some(vs.playlist_index),
        RepeatMode::Off | RepeatMode::All => {
            if vs.shuffle && !vs.shuffle_order.is_empty() {
                let cur_pos = vs
                    .shuffle_order
                    .iter()
                    .position(|&i| i == vs.playlist_index)
                    .unwrap_or(0);
                match vs.repeat_mode {
                    RepeatMode::Off => vs.shuffle_order.get(cur_pos + 1).copied(),
                    RepeatMode::All => Some(vs.shuffle_order[(cur_pos + 1) % len]),
                    RepeatMode::One => unreachable!(),
                }
            } else {
                let next = vs.playlist_index + 1;
                match vs.repeat_mode {
                    RepeatMode::Off => {
                        if next < len {
                            Some(next)
                        } else {
                            None
                        }
                    }
                    RepeatMode::All => Some(next % len),
                    RepeatMode::One => unreachable!(),
                }
            }
        }
    }
}

// Repeat One repeats whatever is actually playing. `now_playing` is the source
// of truth, not `playlist[playlist_index]`: a song played on its own leaves the
// playlist empty, and one played while an album playlist is loaded doesn't move
// playlist_index (start_playback never touches it). Both cases used to fall
// through — the lone song just stopped, the off-list song replayed the album's
// current track instead.
pub(crate) fn repeat_one_target(vs: &VideoState) -> Option<QueueItem> {
    if vs.current_is_audio && vs.repeat_mode == RepeatMode::One {
        vs.now_playing.clone().filter(|q| q.item_type == "Audio")
    } else {
        None
    }
}

// Repeat All with no album playlist: the queue is a ring. The song that just
// ended goes to the back of the queue before the head plays, so a set of
// queued songs keeps going round and a lone song loops. (Queue items are
// removed as they play, so without this Repeat All had nothing to repeat and
// just stopped.) Not applied when the queue head is a video — class-gated
// like every other advance.
pub(crate) fn repeat_all_ring(vs: &VideoState) -> bool {
    vs.current_is_audio
        && vs.repeat_mode == RepeatMode::All
        && vs.playlist.is_empty()
        && vs.queue.first().is_none_or(|q| q.item_type == "Audio")
}

// One step of the Repeat All ring: re-queue the song that just ended, play the head.
pub(crate) fn take_repeat_all_ring_next(vs: &mut VideoState) -> Option<QueueItem> {
    if let Some(np) = vs.now_playing.clone().filter(|q| q.item_type == "Audio") {
        vs.queue.push(np);
    }
    if vs.queue.is_empty() {
        None
    } else {
        Some(vs.queue.remove(0))
    }
}

// ── Songs queued during an album / playlist ───────────────────────────────────
// "Add to Queue" / "Play Next" while an album or playlist plays: the queued songs play before
// the playlist continues (Play Next at the front, Add to Queue in the order added), and each one
// joins the session's playlist as it starts — right after the song it follows — so Repeat All
// loops it and ⏮ goes back past it. Only songs: a queued video waits in the queue (class-gated).

/// The queue index of the next song to play before the playlist continues, if any.
fn first_queued_song(vs: &VideoState) -> Option<usize> {
    if !vs.current_is_audio || vs.playlist.is_empty() {
        return None;
    }
    vs.queue.iter().position(|q| q.item_type == "Audio")
}

/// A queued song that starts playing joins the session playlist right after the current song
/// and becomes the current one. In shuffle it takes the slot after the current one in this
/// round's order (the round continues after it); the next round reshuffles everything.
pub(crate) fn join_playlist(vs: &mut VideoState, song: QueueItem) {
    let at = (vs.playlist_index + 1).min(vs.playlist.len());
    let cur_pos = vs
        .shuffle_order
        .iter()
        .position(|&i| i == vs.playlist_index);
    vs.playlist.insert(at, song);
    for idx in vs.shuffle_order.iter_mut() {
        if *idx >= at {
            *idx += 1;
        }
    }
    if vs.shuffle && !vs.shuffle_order.is_empty() {
        let pos = cur_pos.map_or(vs.shuffle_order.len(), |p| p + 1);
        vs.shuffle_order.insert(pos, at);
    }
    vs.playlist_index = at;
}

/// Whether moving on from the current song starts a new shuffle round (Repeat All).
fn shuffle_round_ends(vs: &VideoState) -> bool {
    vs.shuffle
        && vs.repeat_mode == RepeatMode::All
        && !vs.shuffle_order.is_empty()
        && vs
            .shuffle_order
            .iter()
            .position(|&i| i == vs.playlist_index)
            .is_none_or(|p| p + 1 >= vs.shuffle_order.len())
}

/// Moves the playlist to `idx`. A new shuffle round (Repeat All) gets a fresh order — the song
/// starting it first — so songs that joined during a round land at random spots in the next.
fn move_playlist_to(vs: &mut VideoState, idx: usize) {
    let new_round = shuffle_round_ends(vs);
    vs.playlist_index = idx;
    if new_round {
        rebuild_shuffle_order(vs);
    }
}

// Non-mutating preview of what natural end will play (class-gated like the
// timer's advance). Used by the gapless preload check.
pub(crate) fn peek_natural_next(vs: &VideoState) -> Option<QueueItem> {
    if let Some(q) = repeat_one_target(vs) {
        return Some(q);
    }
    if repeat_all_ring(vs) {
        return vs
            .queue
            .first()
            .cloned()
            .or_else(|| vs.now_playing.clone().filter(|q| q.item_type == "Audio"));
    }
    if let Some(i) = first_queued_song(vs) {
        return vs.queue.get(i).cloned();
    }
    let ended_audio = vs.current_is_audio;
    let queue_head_matches = vs
        .queue
        .first()
        .map(|q| (q.item_type == "Audio") == ended_audio)
        .unwrap_or(false);
    if ended_audio && !vs.playlist.is_empty() {
        if let Some(i) = natural_next_index(vs) {
            return vs.playlist.get(i).cloned();
        }
        return if queue_head_matches {
            vs.queue.first().cloned()
        } else {
            None
        };
    }
    if queue_head_matches {
        vs.queue.first().cloned()
    } else {
        None
    }
}

// Advance the bookkeeping to match the entry mpv just started gaplessly.
pub(crate) fn commit_natural_next(vs: &mut VideoState, qi: &QueueItem) {
    // Repeat One replayed the same song: playlist position and queue unchanged.
    if repeat_one_target(vs).is_some_and(|q| q.id == qi.id) {
        return;
    }
    if repeat_all_ring(vs) {
        let next = take_repeat_all_ring_next(vs);
        if next.as_ref().map(|q| q.id.as_str()) != Some(qi.id.as_str()) {
            warn!(
                "gapless: repeat-all ring head {:?} != preloaded {}",
                next.map(|q| q.id),
                qi.id
            );
        }
        return;
    }
    if let Some(i) = first_queued_song(vs)
        && vs.queue[i].id == qi.id
    {
        let song = vs.queue.remove(i);
        join_playlist(vs, song);
        return;
    }
    if vs.current_is_audio
        && !vs.playlist.is_empty()
        && let Some(i) = natural_next_index(vs)
        && vs.playlist.get(i).map(|q| q.id == qi.id).unwrap_or(false)
    {
        move_playlist_to(vs, i);
        return;
    }
    if vs.queue.first().map(|q| q.id == qi.id).unwrap_or(false) {
        vs.queue.remove(0);
    }
}

// Drop the gapless-preloaded entry — call whenever the upcoming order changes
// (shuffle/repeat toggles, queue edits). The next preload check re-peeks.
pub(crate) fn invalidate_preload(vs: &mut VideoState) {
    if vs.preloaded_next.take().is_some()
        && let Some(p) = vs.player.as_mut()
    {
        p.cancel_pending();
    }
}

pub(crate) fn playlist_next(vs: &mut VideoState) -> Option<QueueItem> {
    // A queued song comes before the playlist continues (and joins it).
    if let Some(i) = first_queued_song(vs) {
        let song = vs.queue.remove(i);
        join_playlist(vs, song.clone());
        return Some(song);
    }
    let len = vs.playlist.len();
    if len > 0 {
        let next_idx = if vs.shuffle && !vs.shuffle_order.is_empty() {
            let cur_pos = vs
                .shuffle_order
                .iter()
                .position(|&i| i == vs.playlist_index)
                .unwrap_or(0);
            let next_pos = cur_pos + 1;
            match vs.repeat_mode {
                RepeatMode::Off => vs.shuffle_order.get(next_pos).copied(),
                RepeatMode::All | RepeatMode::One => Some(vs.shuffle_order[next_pos % len]),
            }
        } else {
            let next = vs.playlist_index + 1;
            match vs.repeat_mode {
                RepeatMode::Off => {
                    if next < len {
                        Some(next)
                    } else {
                        None
                    }
                }
                RepeatMode::All | RepeatMode::One => Some(next % len),
            }
        };
        if let Some(idx) = next_idx {
            move_playlist_to(vs, idx);
            return Some(vs.playlist[idx].clone());
        }
        // Playlist exhausted (Repeat Off) — fall through to the queue below.
        // Before this fix the queue only played when the playlist was EMPTY,
        // so queued items never played after an album finished.
    }
    // ⏭ under Repeat All with no album playlist: keep the skipped song in the loop.
    if repeat_all_ring(vs) {
        return take_repeat_all_ring_next(vs);
    }
    if vs.queue.is_empty() {
        None
    } else {
        Some(vs.queue.remove(0))
    }
}

// The TRUE next episode after `current_id` in `series_id`. /Shows/NextUp's answer is
// used when it's strictly forward of the current episode (it knows watched state —
// e.g. skips an episode already watched elsewhere); otherwise strict position + 1.
// NextUp is unreliable at an episode boundary: before the stop/played report lands
// it returns the CURRENT episode, and once the series is fully watched (including
// after the credits mark) it suggests a rewatch — both made a series' last episode
// restart itself. A same-id or earlier suggestion fails the forward check.
pub(crate) async fn resolve_true_next_episode(
    cli: &JellyfinClient,
    series_id: &str,
    current_id: &str,
) -> Option<MediaItem> {
    let eps = cli.get_series_episodes(series_id).await.ok()?;
    let cur_pos = eps.iter().position(|e| e.id == current_id)?;

    if let Ok(Some(next)) = cli.get_next_up_for_series(series_id).await
        && eps
            .iter()
            .position(|e| e.id == next.id)
            .is_some_and(|p| p > cur_pos)
    {
        return Some(next);
    }
    eps.into_iter().nth(cur_pos + 1)
}

// Regenerate shuffle_order for the current playlist with the currently-playing
// item at slot 0 (so the next advance moves naturally). No-op when shuffle is
// off or the playlist is empty. Called from toggle_shuffle, queue_remove, and
// the Play All paths — Play All used to leave shuffle_order empty, so a new
// album started with shuffle ON played sequentially while ⇌ showed active.
pub(crate) fn rebuild_shuffle_order(vs: &mut VideoState) {
    if !vs.shuffle || vs.playlist.is_empty() {
        vs.shuffle_order.clear();
        return;
    }
    vs.shuffle_order = shuffle_indices(vs.playlist.len());
    if let Some(pos) = vs
        .shuffle_order
        .iter()
        .position(|&i| i == vs.playlist_index)
    {
        vs.shuffle_order.swap(0, pos);
    }
}

pub(crate) fn toggle_shuffle(vs: &mut VideoState) {
    invalidate_preload(vs);
    vs.shuffle = !vs.shuffle;
    rebuild_shuffle_order(vs);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str) -> QueueItem {
        QueueItem {
            id: id.into(),
            item_type: "Audio".into(),
            series_id: None,
            title: id.into(),
            audio_meta: None,
        }
    }

    fn playing(id: &str, repeat: RepeatMode) -> VideoState {
        VideoState {
            current_is_audio: true,
            now_playing: Some(song(id)),
            repeat_mode: repeat,
            ..Default::default()
        }
    }

    #[test]
    fn repeat_one_replays_a_song_played_without_a_playlist() {
        let vs = playing("a", RepeatMode::One);
        assert_eq!(peek_natural_next(&vs).map(|q| q.id), Some("a".into()));
    }

    #[test]
    fn repeat_one_replays_off_list_song_not_the_playlist_track() {
        let mut vs = playing("x", RepeatMode::One);
        vs.playlist = vec![song("a"), song("b")];
        vs.playlist_index = 0;
        assert_eq!(peek_natural_next(&vs).map(|q| q.id), Some("x".into()));
        commit_natural_next(&mut vs, &song("x"));
        assert_eq!(vs.playlist_index, 0);
    }

    #[test]
    fn repeat_one_in_playlist_keeps_position() {
        let mut vs = playing("b", RepeatMode::One);
        vs.playlist = vec![song("a"), song("b"), song("c")];
        vs.playlist_index = 1;
        assert_eq!(peek_natural_next(&vs).map(|q| q.id), Some("b".into()));
        commit_natural_next(&mut vs, &song("b"));
        assert_eq!(vs.playlist_index, 1);
    }

    #[test]
    fn repeat_off_single_song_has_nothing_next() {
        let vs = playing("a", RepeatMode::Off);
        assert!(peek_natural_next(&vs).is_none());
    }

    #[test]
    fn repeat_one_never_applies_to_video() {
        let mut vs = playing("m", RepeatMode::One);
        vs.current_is_audio = false;
        assert!(repeat_one_target(&vs).is_none());
    }

    #[test]
    fn repeat_all_loops_a_lone_song() {
        let vs = playing("a", RepeatMode::All);
        assert_eq!(peek_natural_next(&vs).map(|q| q.id), Some("a".into()));
    }

    #[test]
    fn repeat_all_plays_queued_audio_before_looping() {
        let mut vs = playing("a", RepeatMode::All);
        vs.queue = vec![song("q")];
        assert_eq!(peek_natural_next(&vs).map(|q| q.id), Some("q".into()));
    }

    #[test]
    fn repeat_all_ring_loops_the_whole_queued_set() {
        let mut vs = playing("a", RepeatMode::All);
        vs.queue = vec![song("b"), song("c")];
        let order: Vec<String> = (0..6)
            .map(|_| {
                let q = take_repeat_all_ring_next(&mut vs).unwrap();
                vs.now_playing = Some(q.clone());
                q.id
            })
            .collect();
        assert_eq!(order, ["b", "c", "a", "b", "c", "a"]);
    }

    #[test]
    fn repeat_all_gapless_commit_rotates_the_ring() {
        let mut vs = playing("a", RepeatMode::All);
        vs.queue = vec![song("b")];
        commit_natural_next(&mut vs, &song("b"));
        assert_eq!(
            vs.queue.iter().map(|q| q.id.as_str()).collect::<Vec<_>>(),
            ["a"]
        );
    }

    #[test]
    fn repeat_all_next_button_keeps_skipped_song_in_the_loop() {
        let mut vs = playing("a", RepeatMode::All);
        vs.queue = vec![song("b")];
        assert_eq!(playlist_next(&mut vs).map(|q| q.id), Some("b".into()));
        assert_eq!(
            vs.queue.iter().map(|q| q.id.as_str()).collect::<Vec<_>>(),
            ["a"]
        );
    }

    #[test]
    fn repeat_all_ring_stops_at_a_queued_video() {
        let mut vs = playing("a", RepeatMode::All);
        vs.queue = vec![QueueItem {
            item_type: "Movie".into(),
            ..song("m")
        }];
        assert!(!repeat_all_ring(&vs));
        assert!(peek_natural_next(&vs).is_none());
    }

    #[test]
    fn repeat_all_with_playlist_wraps_the_playlist() {
        let mut vs = playing("b", RepeatMode::All);
        vs.playlist = vec![song("a"), song("b")];
        vs.playlist_index = 1;
        assert_eq!(peek_natural_next(&vs).map(|q| q.id), Some("a".into()));
    }

    /// Plays everything natural end would, `n` times, and returns the ids in order.
    fn play_on(vs: &mut VideoState, n: usize) -> Vec<String> {
        (0..n)
            .map(|_| {
                let q = peek_natural_next(vs).expect("something next");
                commit_natural_next(vs, &q);
                vs.now_playing = Some(q.clone());
                q.id
            })
            .collect()
    }

    fn ids(v: &[QueueItem]) -> Vec<&str> {
        v.iter().map(|q| q.id.as_str()).collect()
    }

    #[test]
    fn queued_songs_play_before_the_playlist_continues_and_join_the_loop() {
        // The HTPC case: a playlist on Repeat All, songs added to the queue — they never played.
        let mut vs = playing("a", RepeatMode::All);
        vs.playlist = vec![song("a"), song("b"), song("c")];
        vs.playlist_index = 0;
        vs.queue = vec![song("x"), song("y")]; // Add to Queue: in the order added
        assert_eq!(play_on(&mut vs, 3), ["x", "y", "b"]);
        assert!(vs.queue.is_empty());
        assert_eq!(ids(&vs.playlist), ["a", "x", "y", "b", "c"]);
        // Next round: they're part of the loop, where they were played.
        assert_eq!(play_on(&mut vs, 4), ["c", "a", "x", "y"]);
    }

    #[test]
    fn play_next_comes_before_earlier_queued_songs() {
        let mut vs = playing("a", RepeatMode::Off);
        vs.playlist = vec![song("a"), song("b")];
        vs.queue = vec![song("x")];
        vs.queue.insert(0, song("p")); // Play Next = the queue's front (context_menu::enqueue_item)
        assert_eq!(play_on(&mut vs, 3), ["p", "x", "b"]);
    }

    #[test]
    fn next_button_takes_a_queued_song_first() {
        let mut vs = playing("a", RepeatMode::Off);
        vs.playlist = vec![song("a"), song("b")];
        vs.queue = vec![song("x")];
        assert_eq!(playlist_next(&mut vs).map(|q| q.id), Some("x".into()));
        assert_eq!(ids(&vs.playlist), ["a", "x", "b"]);
        assert_eq!(vs.playlist_index, 1);
        assert_eq!(playlist_next(&mut vs).map(|q| q.id), Some("b".into()));
        // ⏮ from b goes back to x, then a.
        assert_eq!(playlist_prev(&mut vs).map(|q| q.id), Some("x".into()));
    }

    #[test]
    fn a_queued_video_doesnt_jump_ahead_of_the_songs() {
        let mut vs = playing("a", RepeatMode::Off);
        vs.playlist = vec![song("a"), song("b")];
        let mut video = song("v");
        video.item_type = "Episode".into();
        vs.queue = vec![video, song("x")];
        assert_eq!(play_on(&mut vs, 2), ["x", "b"]);
        assert_eq!(ids(&vs.queue), ["v"], "the video waits");
    }

    #[test]
    fn shuffle_a_joined_song_plays_next_and_the_round_continues() {
        let mut vs = playing("a", RepeatMode::All);
        vs.shuffle = true;
        vs.playlist = vec![song("a"), song("b"), song("c"), song("d")];
        vs.shuffle_order = vec![0, 2, 1, 3]; // a, c, b, d
        vs.playlist_index = 0;
        vs.queue = vec![song("x")];
        assert_eq!(play_on(&mut vs, 4), ["x", "c", "b", "d"]);
        assert_eq!(vs.playlist.len(), 5);
    }

    #[test]
    fn shuffle_repeat_all_reshuffles_each_new_round() {
        let mut vs = playing("d", RepeatMode::All);
        vs.shuffle = true;
        vs.playlist = vec![song("a"), song("b"), song("c"), song("d")];
        vs.shuffle_order = vec![0, 1, 2, 3];
        vs.playlist_index = 3; // the round's last song
        let next = play_on(&mut vs, 1);
        assert_eq!(
            next,
            ["a"],
            "the next round starts where the old order wrapped to"
        );
        // A fresh order with the playing song first, every song once.
        assert_eq!(vs.shuffle_order[0], vs.playlist_index);
        let mut sorted = vs.shuffle_order.clone();
        sorted.sort();
        assert_eq!(sorted, [0, 1, 2, 3]);
    }
}
