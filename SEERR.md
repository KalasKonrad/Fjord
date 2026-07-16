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
`releaseDate`, `mediaInfo`, `voteAverage` (TMDB 0-10 rating, `"★ 7.9"` badge
on the Request Detail screen when > 0), `credits: {cast: Cast[], crew:
Crew[]}` — `Cast = {id, name, character, order, profilePath}`, `Crew =
{id, name, job, department, profilePath}`. Both `voteAverage` and `credits`
were present in the spec from the start but not deserialized until the
Request Detail redesign — same previously-unread-field situation as `Season.
posterPath` below.

### TV details **[used]**
```
GET /tv/{tvId}
```
`TvDetails`: same shape + `seasons: Season[]`, each
`{seasonNumber, name, episodeCount, airDate, posterPath}` — `posterPath` now
fetched (TMDB CDN, `w500`) for the season-card strip. **The published
schema has no per-season Jellyfin-availability field** — Fjord's season
picker is pure selection (default all-checked), not an availability display.
Some self-hosted deployments may nest richer per-season status that the
auto-generated spec doesn't capture; not verified against a live instance as
of writing. Same `voteAverage`/`credits` fields as `MovieDetails` above.

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
{ "mediaType": "movie"|"tv", "mediaId": <tmdbId>, "seasons": [1,2,3] | "all",
  "is4k": bool, "tags": [<tagId>, ...] }
→ 201 + MediaRequest
```
`seasons` is omitted entirely for movies. For TV, Fjord sends the literal
string `"all"` when every season is selected (matching the API's own
shorthand) rather than enumerating every season number. **`is4k` and `tags`
are not documented in the published OpenAPI spec at all** — confirmed from
Seerr's actual TypeScript source (`MediaRequestBody`) after the OpenAPI spec
turned out to be incomplete a second time (first was the `media_type`
camelCase mismatch). `tags` is an array of numeric Radarr/Sonarr tag ids, not
free-text strings — see "Tags" below for where they come from. `tags` is
omitted from the body entirely when nothing is selected, matching how
`seasons` is omitted for movies.

### Tags **[used]**
Not in the OpenAPI spec either (same source-vs-spec gap). Fjord fetches the
**default** Radarr (movie) / Sonarr (tv) server's configured tags for the
request-detail tag picker:
```
GET /service/radarr          → ServiceCommonServer[] (find the one with isDefault: true)
GET /service/radarr/{id}     → { server, profiles, rootFolders, tags: Tag[], ... }
                                Tag = { id: number, label: string }
```
(Same shape for `/service/sonarr` and `/service/sonarr/{id}`.) Best-effort —
an empty/no-default-server result, or a permissions error (these endpoints
may require elevated Seerr permissions on some instances), just means no tag
picker shows; it never blocks the request flow.

### Sign-out cleanup
No `DELETE`/cancel endpoint used by Fjord v1 — requests are managed from
Seerr's own UI once created.

---

## Discover landing rows **[used]**

Shown on the Discover screen when no search query is active — Trending,
Popular Movies, Popular TV, Upcoming Movies, Upcoming TV. All five return the
**exact same shape as `/search`** (`{page, totalPages, totalResults,
results}`), confirmed from the OpenAPI spec, so Fjord reuses `SearchResponse`
verbatim with no new model types:
```
GET /discover/trending?page=1              — movies + TV, mixed
GET /discover/movies?page=1                — popular movies (server default sort)
GET /discover/movies/upcoming?page=1
GET /discover/tv?page=1                    — popular TV (server default sort)
GET /discover/tv/upcoming?page=1
```
Fetched once per session, in parallel, on first arrival at the Discover tab.

---

## Images

Poster/backdrop images are served directly from **TMDB's CDN**, not proxied
through Seerr:
```
https://image.tmdb.org/t/p/w500{posterPath}     — posters (movie/tv, season cards)
https://image.tmdb.org/t/p/w1280{backdropPath}  — backdrops
https://image.tmdb.org/t/p/w185{profilePath}    — cast/crew portraits (Request Detail's Cast & Crew row)
```
This is the first time Fjord fetches images from anywhere other than the
user's own Jellyfin server. Cached separately from Jellyfin posters
(`~/.cache/fjord/discover_posters/`, keyed `"<movie|tv>-<tmdbId>[-bg]"`) since
TMDB has no `ImageTags`-style revalidation concept to reuse Jellyfin's
tag-checking cache logic.

---

## Status (unauthenticated) **[used]**

```
GET /status   (security: [])
→ { version, commitTag, updateAvailable, commitsBehind, restartRequired }
```
Two uses: (1) a pre-auth "is this even a Seerr server" sanity check before
attempting the API-key verification flow (which has no dedicated
verify-a-key endpoint — a bad key is only caught on first authenticated use);
(2) `version` is fetched after every successful connect and once at startup,
shown in Fjord's Settings sidebar next to Seerr's connection status.

---

## Not used in v1

- `/collection/{id}`, `/person/{id}` — not surfaced anywhere in Fjord's UI yet.
- `/watchlist`, `/issue/*`, `/blacklist`, `/blocklist` — request management
  beyond the initial create, left to Seerr's own UI.
- Radarr/Sonarr-specific settings (`profileId`, `rootFolder`, `serverId`) on
  the request body — Fjord always requests with server defaults.
