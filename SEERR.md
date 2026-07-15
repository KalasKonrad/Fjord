# Seerr API Reference

A curated reference for Fjord's Seerr integration (Discover screen + media
requests). Seerr (docs.seerr.dev) is the unified successor to Overseerr and
Jellyseerr — a media-request manager with native Jellyfin support. Endpoints
marked **[used]** are wired up in `fjord-seerr/src/client.rs`.

Researched directly against the real OpenAPI spec
(`https://raw.githubusercontent.com/seerr-team/seerr/refs/heads/develop/seerr-api.yml`),
not assumed from memory. All paths below are relative to `{seerr_url}/api/v1`.

---

## Auth

Seerr supports four sign-in methods; Fjord implements all four
(`fjord-seerr::SeerrAuth` — `ApiKey(String)` or `Session(String)`, the latter
shared by the three cookie-based methods below).

### API key **[used]**
Generated once in Seerr's own admin Settings. Attach on every request:
```
X-Api-Key: <key>
```
Never expires. No login call needed — just attach the header.

### Jellyfin username/password **[used]**
```
POST /auth/jellyfin
{ "username": "...", "password": "..." }
```
Returns `200` + `User`, sets a `connect.sid` session cookie (read from
`Set-Cookie` on the response — Fjord parses this manually rather than using
reqwest's built-in cookie jar, since only the `connect.sid` name=value pair
needs to be echoed back, not a full jar).

### Jellyfin Quick Connect **[used]**
Passwordless — the user approves a code from inside their own Jellyfin
app/web UI.
```
POST /auth/jellyfin/quickconnect/initiate          → { "code": "123456", "secret": "..." }
GET  /auth/jellyfin/quickconnect/check?secret=...   → { "authenticated": false }   (poll ~2s)
POST /auth/jellyfin/quickconnect/authenticate       → 200 + User + session cookie
     { "secret": "..." }
```
`check` returns `404` if the Quick Connect session expired — Fjord surfaces
this as "Code expired, try again" rather than polling forever.

### Local Seerr account **[used]**
For instances configured with their own accounts, independent of Jellyfin:
```
POST /auth/local
{ "email": "...", "password": "..." }
```
Returns `200` + `User` + session cookie, same shape as Jellyfin login.

### Sign out **[used]**
```
POST /auth/logout
```
Clears the session cookie server-side. No-op for API-key auth (nothing
server-side to clear).

### Session expiry
A `401` on any authenticated call under session auth (not API-key, which
doesn't expire) means the cookie expired — Fjord clears the local connection
and surfaces "Seerr session expired — reconnect in Settings" rather than
retrying indefinitely. See `discover.rs::handle_seerr_error`.

---

## Search & content

### Multi-search **[used]**
```
GET /search?query=...&page=1&language=en
→ { page, totalPages, totalResults, results: (MovieResult|TvResult|PersonResult)[] }
```
Discriminate by `mediaType` (`"movie"` | `"tv"` | `"person"`). Fjord filters
out `person` results — v1 shows movies/TV only. Each result carries
`posterPath` (TMDB-relative, not proxied by Seerr — see Images below) and an
optional `mediaInfo` (present only once Seerr has ever seen the item).

### Movie details **[used]**
```
GET /movie/{tmdbId}
```
`MovieDetails`: title, overview, genres, `posterPath`/`backdropPath`,
`releaseDate`, `mediaInfo`.

### TV details **[used]**
```
GET /tv/{tvId}
```
`TvDetails`: same shape + `seasons: Season[]`, each
`{seasonNumber, name, episodeCount, airDate, posterPath}`. **The published
schema has no per-season Jellyfin-availability field** — Fjord's season
picker is pure selection (default all-checked), not an availability display.
Some self-hosted deployments may nest richer per-season status that the
auto-generated spec doesn't capture; not verified against a live instance as
of writing.

### Availability status
`MediaInfo.status` (only present once Seerr has seen the item):

| Value | Meaning | Fjord badge |
|---|---|---|
| *(absent)* | Never requested | none — "Request" shown |
| `1` UNKNOWN | — | none — "Request" shown |
| `2` PENDING | Requested, awaiting approval/processing | "Requested" |
| `3` PROCESSING | Being fetched by Radarr/Sonarr | "Processing" |
| `4` PARTIALLY_AVAILABLE | Some seasons available (TV) | "Partially Available" — Request still shown |
| `5` AVAILABLE | Fully in the library | "In Library" — no Request button |
| `6` DELETED | — | none — "Request" shown |

---

## Requests

### Create **[used]**
```
POST /request
{ "mediaType": "movie"|"tv", "mediaId": <tmdbId>, "seasons": [1,2,3] | "all" }
→ 201 + MediaRequest
```
`seasons` is omitted entirely for movies. For TV, Fjord sends the literal
string `"all"` when every season is selected (matching the API's own
shorthand) rather than enumerating every season number.

### Sign-out cleanup
No `DELETE`/cancel endpoint used by Fjord v1 — requests are managed from
Seerr's own UI once created.

---

## Images

Poster/backdrop images are served directly from **TMDB's CDN**, not proxied
through Seerr:
```
https://image.tmdb.org/t/p/w500{posterPath}     — posters
https://image.tmdb.org/t/p/w1280{backdropPath}  — backdrops
```
This is the first time Fjord fetches images from anywhere other than the
user's own Jellyfin server. Cached separately from Jellyfin posters
(`~/.cache/fjord/discover_posters/`, keyed `"<movie|tv>-<tmdbId>[-bg]"`) since
TMDB has no `ImageTags`-style revalidation concept to reuse Jellyfin's
tag-checking cache logic.

---

## Status (unauthenticated)

```
GET /status   (security: [])
```
Used as a pre-auth "is this even a Seerr server" sanity check before
attempting the API-key verification flow (which has no dedicated
verify-a-key endpoint — a bad key is only caught on first authenticated use).

---

## Not used in v1

- `/discover/*`, `/collection/{id}`, `/person/{id}` — a query-less "Trending"
  landing view was considered but deferred; v1 is search-only.
- `/watchlist`, `/issue/*`, `/blacklist`, `/blocklist` — request management
  beyond the initial create, left to Seerr's own UI.
- Radarr/Sonarr-specific settings (`profileId`, `rootFolder`, `serverId`) on
  the request body — Fjord always requests with server defaults.
