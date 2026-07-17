# Jellyfin API Reference

A curated reference for building a Jellyfin frontend. Covers what is available,
not just what Fjord currently uses. Endpoints marked **[used]** are already
wired up in `fjord-api/src/client.rs`.

---

## Auth & headers

### Login
```
POST /Users/AuthenticateByName
Content-Type: application/json
Authorization: MediaBrowser Client="Fjord", Device="Desktop", DeviceId="<uuid>", Version="1.0.0"

{ "Username": "...", "Pw": "..." }
```
Returns `{ "AccessToken": "...", "User": { "Id": "..." } }`.

### Subsequent requests **[used]**
Every request must carry:
```
X-Emby-Token: <token>
Authorization: MediaBrowser Client="Fjord", Device="Desktop", DeviceId="<uuid>", Version="1.0.0", Token="<token>"
```
Both headers are required. `DeviceId` must be unique per install — two installs
sharing a DeviceId will invalidate each other's session when either authenticates.

### Token lifetime
Tokens do not expire under normal use. They are invalidated if:
- Another device authenticates with the same `DeviceId`
- The user changes their password
- An admin revokes the session

On 401, show the login screen. On other errors, proceed (transient network issue).

---

## Time: ticks

Jellyfin measures all durations and positions in **ticks**.
`1 tick = 100 nanoseconds`, so `10,000,000 ticks = 1 second`.

```rust
fn ticks_to_secs(ticks: i64) -> f64 { ticks as f64 / 10_000_000.0 }
fn secs_to_ticks(secs: f64) -> i64  { (secs * 10_000_000.0) as i64 }
```

---

## Common query parameters

Most fields are **omitted by default**. You must explicitly request them.

| Parameter | Values | Notes |
|---|---|---|
| `Fields` | `Overview,People,Chapters,MediaStreams,MediaSources,Genres,Studios,CommunityRating,CriticRating,OfficialRating,ExternalUrls,ProviderIds` | Comma-separated. None included unless requested. |
| `EnableUserData` | `true` / `false` | Include `UserData` (played, position, favorite). Default false on some endpoints. |
| `ImageTypeLimit` | integer | Max images of each type to return in metadata |
| `EnableImages` | `true` / `false` | Include image tag info in response |
| `Recursive` | `true` / `false` | Search recursively through all sub-folders |
| `SortBy` | `DateCreated,SortName,Random,PremiereDate,CommunityRating,Runtime,PlayCount` | Comma-separated, multiple sort keys supported |
| `SortOrder` | `Ascending` / `Descending` | |
| `Filters` | `IsResumable,IsUnplayed,IsPlayed,IsFavorite` | Comma-separated |
| `IncludeItemTypes` | `Movie,Series,Episode,Season,MusicAlbum,Audio,Person` | Comma-separated |
| `ExcludeItemTypes` | same as above | |
| `Limit` | integer | Page size |
| `StartIndex` | integer | Pagination offset |
| `ParentId` | item id | Restrict to a library or folder |
| `MediaTypes` | `Video,Audio,Photo,Book` | |
| `searchTerm` | string | Server-side search (case-insensitive, matches title) |
| `PersonIds` | comma-separated ids | Filter items featuring specific people |
| `GenreIds` | comma-separated ids | Filter by genre |
| `StudioIds` | comma-separated ids | Filter by studio |
| `Years` | comma-separated years | Filter by production year |
| `HasTmdbId` | `true` | Only items with a TMDB match |

---

## Library browsing

### Get items **[used]**
```
GET /Users/{userId}/Items
    ?Recursive=true
    &IncludeItemTypes=Movie,Series
    &Fields=Overview,Genres,People
    &EnableUserData=true
    &SortBy=SortName
    &SortOrder=Ascending
    &Limit=50&StartIndex=0
```

### Single item detail **[used]** (via `on_open_detail`)
```
GET /Users/{userId}/Items/{itemId}
    ?Fields=Overview,People,Chapters,MediaStreams,Genres,Studios,ExternalUrls
    &EnableUserData=true
```
Returns full item including `UserData`, `People[]`, `Chapters[]`, `MediaStreams[]`.

### Continue watching **[used]**
```
GET /Users/{userId}/Items
    ?Filters=IsResumable
    &MediaTypes=Video
    &Recursive=true
    &Limit=15
```

### Next up (cross-series) **[used]**
```
GET /Shows/NextUp
    ?UserId={userId}
    &Fields=Overview
    &Limit=15
```

### Next up for a specific series **[used]**
```
GET /Shows/NextUp?SeriesId={seriesId}&UserId={userId}
```
Returns the single next unwatched episode. Used for auto-advance and Enter on
series cards.

### Recently added **[used]**
```
GET /Users/{userId}/Items
    ?Filters=IsUnplayed
    &SortBy=DateCreated
    &SortOrder=Descending
    &IncludeItemTypes=Movie   (or Series, Episode)
    &Recursive=true
    &Limit=15
```

### Not watched (random selection)
```
GET /Users/{userId}/Items
    ?Filters=IsUnplayed
    &SortBy=Random
    &IncludeItemTypes=Movie
    &Recursive=true
    &Limit=15
```

### Similar / related items
```
GET /Users/{userId}/Items/{itemId}/Similar
    ?Limit=12
    &Fields=Overview
```

### Server-side search
```
GET /Users/{userId}/Items
    ?searchTerm=<query>
    &Recursive=true
    &IncludeItemTypes=Movie,Series,Episode
    &Fields=Overview
    &EnableUserData=true
    &Limit=50
```
Server searches titles, sort names, and original titles. Debounce keystrokes
before firing (~300 ms).

---

## TV shows

### Season list **[used]**
```
GET /Shows/{seriesId}/Seasons
    ?UserId={userId}
    &Fields=Overview
    &EnableUserData=true
```

### Episode list **[used]**
```
GET /Shows/{seriesId}/Episodes
    ?UserId={userId}
    &SeasonId={seasonId}
    &Fields=Overview
    &EnableUserData=true
```

### Episode list for a whole series (all seasons)
```
GET /Shows/{seriesId}/Episodes
    ?UserId={userId}
    &Fields=Overview
    &EnableUserData=true
```
Omit `SeasonId` to get all episodes across all seasons in one call.

---

## Images

Base URL: `{server}/Items/{itemId}/Images/{type}`

| Type | Notes |
|---|---|
| `Primary` | Poster (portrait). Series, movies, episodes. **[used]** |
| `Backdrop/0` | Wide backdrop. Index 0 = first backdrop. **[used]** |
| `Backdrop/{n}` | Additional backdrops (carousel) |
| `Thumb` | Thumbnail / wide card image. Episodes use this. |
| `Logo` | Transparent logo overlay. Good for hero banners. |
| `Banner` | Wide banner strip. Older Kodi-style art. |
| `Art` | Fan art (square). |
| `Disc` | Disc art. |

Useful query parameters:
```
?maxWidth=400&quality=90          — resize + compress server-side
?fillWidth=400&fillHeight=600     — fit within box, no crop
?tag=<imageTag>                   — cache-busting; use tag from item response
```

The `tag` from the item's `ImageTags.Primary` ensures the browser/client cache
is invalidated when the image changes on the server.

---

## Playback

### Direct play URL **[used]**
```
GET /Videos/{itemId}/stream
    ?static=true
    &api_key={token}
    &MediaSourceId={itemId}
```
Streams the original file with no transcoding. Use for local network where
bandwidth is not a constraint.

### HLS adaptive stream (transcoding)
```
GET /Videos/{itemId}/master.m3u8
    ?api_key={token}
    &VideoCodec=h264
    &AudioCodec=aac
    &MaxStreamingBitrate=20000000
```
Server transcodes to HLS. Useful for remote access or unsupported codecs.

### Playback info (codec / container details)
```
POST /Items/{itemId}/PlaybackInfo
    ?UserId={userId}

{ "DeviceProfile": { ... } }
```
Returns available `MediaSources` with codec, container, bitrate, stream info.
Use to decide direct-play vs transcode before starting.

### Report playback started **[used]**
```
POST /Sessions/Playing
{ "ItemId": "...", "MediaSourceId": "...", "PlayMethod": "DirectPlay",
  "PositionTicks": 0, "IsPaused": false, "IsMuted": false,
  "PlaySessionId": "..." }
```

### Report progress **[used]**
```
POST /Sessions/Playing/Progress
{ "ItemId": "...", "PositionTicks": 12340000, "IsPaused": false, ... }
```
Call every 10–30 seconds during playback. Jellyfin uses this to update
`UserData.PlaybackPositionTicks`.

### Report stopped **[used]**
```
POST /Sessions/Playing/Stopped
{ "ItemId": "...", "PositionTicks": 12340000, "PositionTicks": ..., ... }
```
Jellyfin marks the item as fully played if position is within ~90% of runtime.

---

## User data mutations

### Mark as played
```
POST /Users/{userId}/PlayedItems/{itemId}
```
Sets `UserData.Played = true` and records `UserData.LastPlayedDate`. Returns
updated `UserData`.

### Mark as unplayed
```
DELETE /Users/{userId}/PlayedItems/{itemId}
```

### Favorite
```
POST /Users/{userId}/FavoriteItems/{itemId}
```

### Unfavorite
```
DELETE /Users/{userId}/FavoriteItems/{itemId}
```

### Update user data directly
```
POST /Users/{userId}/Items/{itemId}/UserData
{ "PlaybackPositionTicks": 12340000, "Played": false, "IsFavorite": false }
```

---

## People / cast

### Items featuring a person
```
GET /Users/{userId}/Items
    ?PersonIds={personId}
    &IncludeItemTypes=Movie,Series
    &Recursive=true
    &SortBy=SortName
```

### Person detail
```
GET /Persons/{personId}
```
Returns name, overview, birth date, image tags.

### Person image
```
GET /Items/{personId}/Images/Primary
```

---

## Chapters

Chapters are embedded in the item detail response when `Fields=Chapters` is
requested. They are **not** a separate endpoint.

```json
"Chapters": [
  { "StartPositionTicks": 0, "Name": "Opening", "ImageTag": "..." },
  { "StartPositionTicks": 1800000000, "Name": "Chapter 2", "ImageTag": "..." }
]
```

### Chapter thumbnail image
```
GET /Videos/{itemId}/Chapters/{index}/Images
```
Or using the item images endpoint with type `Chapter`:
```
GET /Items/{itemId}/Images/Chapter/{index}?tag={imageTag}
```

---

## SyncPlay

SyncPlay lets multiple clients watch the same content in sync. Requires a
WebSocket connection alongside the REST calls.

### Playlists

- `GET /Users/{userId}/Items?IncludeItemTypes=Playlist&Recursive=true&Fields=ChildCount` — all playlists. `MediaType` ("Audio"/"Video") distinguishes music playlists; filter client-side.
- `GET /Playlists/{playlistId}/Items?UserId=…` — entries in playlist order. Each item carries `PlaylistItemId` — the *entry* id, required for removal (one track can appear twice).
- `POST /Playlists` — JSON body `{ "Name", "Ids": [itemIds], "UserId", "MediaType": "Audio" }` → `{ "Id" }`.
- `POST /Playlists/{playlistId}/Items?Ids=a,b,c&UserId=…` — append items.
- `DELETE /Playlists/{playlistId}/Items?EntryIds=x,y` — remove entries (PlaylistItemIds, NOT item ids).

## WebSocket connection
```
wss://{server}/socket?api_key={token}&deviceId={deviceId}
```
All SyncPlay real-time events (play/pause/seek/buffer from other clients) arrive
as WebSocket messages with `MessageType: "SyncPlayGroupUpdate"`.

### Create a group
```
POST /SyncPlay/New
{ "GroupName": "Movie Night" }
```

### List available groups
```
GET /SyncPlay/List
```

### Join a group
```
POST /SyncPlay/Join
{ "GroupId": "..." }
```

### Leave a group
```
POST /SyncPlay/Leave
```

### Set what's playing (group admin)
```
POST /SyncPlay/SetPlaylistItem
{ "PlaylistItemId": "..." }
```

### Report buffer state
```
POST /SyncPlay/Buffering
{ "When": "<ISO8601>", "PositionTicks": 12340000, "IsPlaying": false }
```
Call when playback stalls waiting for data. Other clients will pause to wait.

### Report ready to play
```
POST /SyncPlay/ReadyToPlay
{ "When": "<ISO8601>", "PositionTicks": 12340000, "IsPlaying": true }
```

### Keep-alive ping
```
POST /SyncPlay/Ping
{ "Ping": 42 }   // round-trip latency in ms
```

---

## Plugins

Plugins extend the Jellyfin server API. Check `GET /Plugins` to see what's
installed on the server before calling plugin-specific endpoints.

### Intro Skipper **[used]**
Detects intro and credits segments in TV episodes. Plugin v2+ uses a single combined endpoint.
```
GET /Episode/{itemId}/Timestamps
```
Returns:
```json
{
  "Introduction": { "Start": 15.0, "End": 90.0 },
  "Credits":      { "Start": 1200.0, "End": 1260.0 },
  "Recap": ..., "Preview": ..., "Commercial": ...
}
```
`End > 0` means the segment was detected. Returns 404 if the plugin is absent or the episode has not been analyzed — handle gracefully.

### Playback Reporting
Records detailed watch history and generates statistics.
```
GET /UserActivity?userId={userId}&days=30
GET /PlaybackReport?userId={userId}
```

### Trakt
Syncs watched status and ratings with Trakt.tv. No client-side API calls
needed — sync is server-initiated. Check sync status via:
```
GET /Trakt/Users/{userId}/SyncStatus
```

### Open Subtitles / Subtitle providers
```
GET /Items/{itemId}/RemoteSearch/Subtitle?language=en
POST /Items/{itemId}/RemoteSubtitles/{subtitleId}   — download and attach
```

### Fanart.tv / Artwork providers
Extra artwork (logos, clearart, disc art) is fetched by the server and stored
as additional image types. Access via the standard Images endpoint with the
relevant type (`Logo`, `Art`, `Disc`).

---

## WebSocket events

Beyond SyncPlay, the Jellyfin WebSocket pushes real-time events useful for a
live UI:

| `MessageType` | Trigger |
|---|---|
| `LibraryChanged` | Item added, removed, or metadata updated |
| `UserDataChanged` | Played state, position, or favorite changed (e.g. from another client) |
| `Sessions` | Another session started, stopped, or changed state |
| `ActivityLogEntry` | Server activity log |
| `ScheduledTaskEnded` | Background scan/task completed |
| `ServerShuttingDown` | Server is about to stop |
| `ServerRestarting` | Server is about to restart |

`UserDataChanged` is particularly useful for keeping progress bars accurate
when the user is watching on another device simultaneously.

### WebSocket reliability caveats

Jellyfin's WebSocket has well-documented reliability problems that affect how
much you should rely on it:

- **Connections silently drop** and don't always reconnect
  ([jellyfin-androidtv #3461](https://github.com/jellyfin/jellyfin-androidtv/issues/3461)).
  Even first-party clients work around this by reconnecting on a timer.
- **Only the last connected client receives messages** when multiple clients
  share a session ([jellyfin #11755](https://github.com/jellyfin/jellyfin/issues/11755)).

**Recommendation:** Don't build the data model around WebSocket push. Use it
as a lightweight enhancement on top of polling/manual refresh. The reliable
baseline for Fjord is:
1. Fresh `GET /Users/{userId}/Items/{itemId}` immediately before `start_playback`
   for accurate `PlaybackPositionTicks`.
2. Manual `fetch_home_data` after `on_stop_playback` to refresh Continue Watching.
3. Polling timer for Not Watched rows.

---

## Non-obvious behaviors

- **Fields are opt-in.** `Overview`, `People`, `Chapters`, `MediaStreams` are
  all omitted from list responses unless you add them to `&Fields=`. The full
  item detail endpoint (`/Items/{id}`) returns more by default but still not
  everything.

- **`UserData` requires `EnableUserData=true`** on list endpoints. The single
  item endpoint (`/Items/{id}`) includes it by default.

- **Image tags are cache keys.** The `ImageTags.Primary` value changes when the
  poster is updated on the server. Always append `?tag={imageTag}` so clients
  invalidate their cache when art changes.

- **`PlaybackPositionTicks` updates asynchronously.** After
  `POST /Sessions/Playing/Stopped`, the position may not immediately reflect in
  a subsequent `GET /Users/{userId}/Items/{itemId}`. Add a short delay (~250 ms)
  before re-fetching user data after stopping.

- **`Played` is set automatically** when the reported stop position is within
  ~90% of `RunTimeTicks`. You don't need to call `POST /PlayedItems` manually
  for normal watching — only for explicit mark-as-played actions.

- **`SortBy=Random` is re-randomised on every request.** There is no seed
  parameter. Two calls with identical parameters return different orderings.

- **Pagination is 0-indexed.** `StartIndex=0` is the first page. Total count
  is in `TotalRecordCount` of the response envelope.

- **`SeriesId` on episodes.** Episode items carry a `SeriesId` field pointing
  to their parent series. Use this to fetch the series poster for episode cards
  rather than the episode thumbnail.

- **Special values in `IncludeItemTypes`.** `"Season"` and `"Episode"` only
  appear in results when `ParentId` or `SeriesId` is set, or when
  `Recursive=true` is combined with a specific type. Querying all episodes
  across the whole library without a series filter can be very slow on large
  servers.

- **No server-side "find item by external provider id" query.** Confirmed
  against Jellyfin's own `ItemsController` source — `hasTmdbId`/`hasImdbId`/
  `hasTvdbId` exist but are presence-only booleans, not value-matching
  filters (nothing like an `AnyProviderIdEquals=Tmdb.12345` parameter).
  Matching a TMDB id (e.g. a Seerr/Discover search result) back to a local
  library item has to be done client-side, scanning `ProviderIds` on
  already-fetched items — see `discover.rs::find_local_item` in Fjord for
  the concrete implementation (movies/series only; `get_all_movies`/
  `get_all_series` and the WS delta-sync upsert path all request
  `ProviderIds` in their `Fields=` for exactly this reason).
