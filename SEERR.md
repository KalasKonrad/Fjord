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

**`query` must be percent-encoded with `%20` for spaces, not `+`.** Real bug,
found live: any multi-word search 400'd. `fjord_seerr::SeerrClient::search`
originally built the URL via `url`'s `query_pairs_mut()`, which follows the
WHATWG `application/x-www-form-urlencoded` serializer and always encodes
space as `+`. Confirmed from Seerr's actual `/search` route source that it
reads `req.query.query` and passes it straight to `tmdb.searchMulti()` with
no validation and no `+`-to-space decoding anywhere in that path — so a `+`
survived as a literal character all the way to TMDB, which rejected it.
Fixed by percent-encoding the query by hand (`percent_encoding::
utf8_percent_encode` with `NON_ALPHANUMERIC`) and building the query string
directly via `Url::set_query` instead of `query_pairs_mut()`. `%20` is
unambiguous under RFC 3986 percent-decoding regardless of which layer
handles it, unlike `+`, which only means space under the specific
form-urlencoded convention — nothing in this particular request/response
path honors that convention, so `+` was always going to be wrong here.

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
  "is4k": bool, "tags": [<tagId>, ...], "profileId": <id> }
→ 201 + MediaRequest
```
`seasons` is omitted entirely for movies. For TV, Fjord sends the literal
string `"all"` when every season is selected (matching the API's own
shorthand) rather than enumerating every season number. **`is4k`, `tags`,
and `profileId` are not documented in the published OpenAPI spec at all**
(the spec does list `profileId` on the request body's schema — the gap here
is narrower than `is4k`/`tags`, which are missing outright — but confirmed
via Seerr's actual TypeScript source (`MediaRequestBody`) either way, same
discipline as everywhere else in this doc). `tags` is an array of numeric
Radarr/Sonarr tag ids, not free-text strings; `profileId` is a numeric
quality-profile id — see "Tags & quality profiles" below for where both
come from. Both are omitted from the body entirely when nothing is
selected/chosen, matching how `seasons` is omitted for movies.

### Tags & quality profiles **[used]**
`tags` isn't in the OpenAPI spec at all; `profiles`' array-ness isn't either
(the spec shows it as a single `ServiceProfile` object with no array
wrapper — confirmed via Seerr's TypeScript source that it's really
`QualityProfile[]`, same class of spec-imprecision as `tags`). Fjord fetches
the **default** Radarr (movie) / Sonarr (tv) server's configured tags *and*
quality profiles together (one fetch covers both):
```
GET /service/radarr          → RadarrSettings[] (find isDefault: true, matching the
                                requested quality tier — is4k per entry — falling back
                                to any default if no tier-specific server exists)
GET /service/radarr/{id}     → { server, profiles: Profile[], rootFolders, tags: Tag[], ... }
                                Tag = { id: number, label: string }
                                Profile = { id: number, name: string }
```
(Same shape for `/service/sonarr` and `/service/sonarr/{id}`.) Best-effort —
an empty/no-default-server result, or a permissions error (these endpoints
may require elevated Seerr permissions on some instances), just means no
tag/profile picker shows; it never blocks the request flow. Fjord's profile
picker always prepends a synthetic "Default" entry (id 0 — real Radarr/
Sonarr profile ids start at 1) so there's an explicit way to send no
`profileId` at all, not just whatever profile happens to be focused first;
if the server has no profiles configured, the whole picker is hidden rather
than showing just the Default entry with nothing else to pick.

**Both quality tiers are fetched up front** — `available_request_options_both_tiers`
resolves both the regular and 4K tier's server and fetches both sets of
tags/profiles before the request-detail screen ever shows the Request
Options modal. Toggling Quality in the modal swaps between the two
pre-fetched sets instantly — no re-fetch, no loading state, no race on rapid
toggling. The common single-instance setup (both tiers resolve to the same
server) costs only the one `/service/{kind}` list call, not a duplicate
detail fetch — the two detail fetches only both run (in parallel) when a
genuinely separate 4K instance exists.

`pick_default_server`'s tier resolution is a three-step cascade, confirmed
against Seerr's real source (`server/lib/settings/index.ts`'s `DVRSettings`
interface — `is4k`/`isDefault` are independent per-server-entry booleans;
`server/routes/service.ts`'s list handler includes both in its response):
1. A server matching the tier **and** marked `isDefault` — the case when an
   admin runs multiple servers per tier and picks one as default.
2. **Any** server matching the tier, regardless of `isDefault` — a lone
   dedicated 4K (or lone regular) instance doesn't need its own `isDefault`
   flag set to be the only sensible choice for that tier. Without this step,
   an admin who never explicitly marked their sole 4K instance "Default"
   would silently fall straight through to step 3 and get the *other*
   tier's server instead — a real bug, found live via a user's Seerr admin
   screenshots showing genuinely different profile/tag lists per tier that
   Fjord wasn't reflecting.
3. Any `isDefault` server at all, regardless of tier — the single
   combined-instance fallback, now the last resort rather than the only
   fallback.

`available_request_options_both_tiers` logs the resolved server list
(`(id, isDefault, is4k)` for every entry) and which id was picked per tier
at `debug!` level — visible in `fjord.log` with `Settings → General → Log
level` set to Debug (default is Info).

### Get all requests **[used]**
```
GET /request?take=&filter=all&sort=added&sortDirection=desc&mediaType=movie|tv
→ { pageInfo, results: MediaRequest[] }
```
`MediaRequest.media` is a `MediaInfo` — same minimal shape as everywhere else
in this doc (`id, tmdbId, tvdbId, status, ...`), **no title or poster**. For
the Discover "Requested" landing row (below), each kept request needs its
own `/movie/{tmdbId}` or `/tv/{tmdbId}` detail fetch to get one — `mediaType`
is queried separately per type (`SeerrClient::requested_not_available` makes
two calls, one per type) since `MediaRequest` itself carries no field to
infer it from. The `filter` query enum (`all/approved/available/pending/
processing/unavailable/failed/deleted/completed`) blends request-approval
state and media-fulfillment state in ways not worth depending on precisely —
Fjord fetches `filter=all` and filters client-side instead: excludes
`MediaRequest.status == 3` (DECLINED) and `MediaInfo.status` 5/7
(AVAILABLE/DELETED), using the same `MediaStatus` enum already modeled
elsewhere in this crate. `MediaStatus`'s real numbering, confirmed directly
against Seerr's `server/constants/media.ts` after a live bug (items showing
in the Requested row that were actually gone): `Unknown=1, Pending=2,
Processing=3, PartiallyAvailable=4, Available=5, Blocklisted=6, Deleted=7`
— Fjord's enum originally had `Deleted=6` with no `Blocklisted` at all, so
every genuinely-`Deleted` (7) request fell through `from_code` to `None`
and was never excluded.

### Sign-out cleanup
No `DELETE`/cancel endpoint used by Fjord v1 — requests are managed from
Seerr's own UI once created.

---

## Discover landing rows **[used]**

Shown on the Discover screen when no search query is active — Trending,
Popular Movies, Popular TV, Upcoming Movies, Upcoming TV, Requested. The
first five return the **exact same shape as `/search`** (`{page, totalPages,
totalResults, results}`), confirmed from the OpenAPI spec, so Fjord reuses
`SearchResponse` verbatim with no new model types:
```
GET /discover/trending?page=1              — movies + TV, mixed
GET /discover/movies?page=1                — popular movies (server default sort)
GET /discover/movies/upcoming?page=1
GET /discover/tv?page=1                    — popular TV (server default sort)
GET /discover/tv/upcoming?page=1
```
The sixth (Requested) is built from `GET /request` instead — see "Get all
requests" above — with a per-item detail fetch for title/poster, capped at
20 items (newest requested first), bounded concurrency (`Semaphore(6)`, same
shape as the Cast & Crew portrait fetch). All six fetched once per session,
in parallel, on first arrival at the Discover tab.

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
- `rootFolder`/`serverId` on the request body — Fjord always uses the
  resolved default server for the chosen quality tier; `profileId` **is**
  now sent when the user picks a non-Default profile in the Request Options
  modal (see "Tags & quality profiles" above).
