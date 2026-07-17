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

**Pagination — Fjord fetches subsequent pages on demand, not up front.**
`page`/`totalPages` are read from every response; `discover::
spawn_discover_search_more` fetches `page+1` and appends when the user's
keyboard nav reaches the last row of the results grid (see
`AppState.discover-load-more()`). Real bug, live-reported: v1 only ever
requested `page=1`, so a common word (hundreds of pages on Seerr/TMDB) was
silently capped at ~20 raw results (fewer once `person` is filtered out) —
a small fraction of what Seerr's own web UI shows for the same query via its
own infinite scroll over the same endpoint.

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
posterPath` below. `status` (TMDB's own production-status string — "Released",
"In Production", "Planned", ... — a different concept from `MediaInfo.status`
below, kept as a separate Rust/AppState field to avoid the two ever being
conflated), `originalLanguage` (ISO 639-1 code, no display name in the
response itself — `discover.rs::language_display_name` maps a small common
set), `productionCountries: {iso_3166_1, name}[]`, `watchProviders:
WatchProviders[]` — one entry per region (`{iso_3166_1, link?, buy?,
flatrate?}`), not a single object keyed by region despite how the raw TMDB
proxy shape looks; only `flatrate` (subscription-included) is used, for the
Details panel's "Currently Streaming On." All confirmed directly from
Seerr's real `server/models/Movie.ts` (not the OpenAPI spec, which doesn't
cover any of these) and live against a real `/movie/{id}` response before
being considered done (2026-07-17).

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
of writing. Same `voteAverage`/`credits`/`status`/`originalLanguage`/
`productionCountries`/`watchProviders` fields as `MovieDetails` above, plus
TV-only `networks: {id, name, logoPath?}[]` and `nextEpisodeToAir?:
{airDate}` (absent between/after seasons, not just for ended shows — a
`Returning Series` with no scheduled next episode is a normal state, not a
bug). Same live-verification note as Movie details above.

Both `MovieDetails` and `TvDetails` also carry `relatedVideos:
{site, key, name, size, type, url}[]` — YouTube trailer/teaser/clip links,
used by the **Watch Trailer** feature (Discover only). Confirmed from
Seerr's real source (`server/models/common.ts`'s `mapVideos`/
`siteUrlCreator`) that `url` is already a fully-formed
`https://www.youtube.com/watch?v={key}` link — no URL construction needed
client-side — and that `site` is always `"YouTube"` in practice (the
mapper's own type signature only ever maps that one site). `fjord_seerr::
Video` only models `type` (renamed `kind`, `type` being a Rust keyword) and
`url` — `site`/`key`/`name`/`size` are unused, same "only what's consumed"
style as `NextEpisode`. `discover::find_trailer_url` prefers a `"Trailer"`,
falls back to `"Teaser"`, else no button — `"Clip"`/`"Featurette"`/etc.
aren't what "Watch Trailer" implies.

### Streaming region **[used]**
```
GET  /auth/me
GET  /user/{id}/settings/main
POST /user/{id}/settings/main
```
`GET /auth/me` resolves "who am I" for whichever of Fjord's 4 auth methods
is active — session-cookie and API-key auth both land here (an API key
resolves to Seerr's "owner" user internally). Its `id` feeds `/user/{id}/
settings/main`, which reads/writes the **connected user's own**
`streamingRegion` preference. **This does NOT require Seerr admin
permission** — an earlier version of this doc (and an earlier version of
Fjord's own reasoning) wrongly assumed it did, conflating it with the
genuinely admin-gated server-wide default at `/settings/main`
(`main.streamingRegion`, not used by Fjord at all). Confirmed from source:
the whole `/settings/*` router is gated by `isAuthenticated(Permission.
ADMIN)` (`server/routes/index.ts`), but `/user/{id}/settings/main` lives
under `server/routes/user/usersettings.ts` and is gated by
`isOwnProfileOrAdmin()` instead — any user, editing their own `id`, passes
regardless of permission level. Confirmed live against a real owner-level
account (`id: 1`) — the POST handler has a special extra check for `user.id
=== 1`, but only blocks a *different* user editing the owner, not the owner
editing themselves.

**The POST body is a full-object replace, not a partial patch** — the
handler unconditionally does `user.username = req.body.username` (etc.)
with no merge logic, so sending just `{"streamingRegion": "SE"}` would null
out the user's username/email/etc. server-side. `fjord_seerr::
UserGeneralSettings` therefore always round-trips: `GET`, mutate the one
field, `POST` the whole struct back. Quota fields present in the `GET`
response (`movieQuotaLimit` etc.) are deliberately not modeled at all —
the POST handler only applies them when the requester has `MANAGE_USERS`
*and* isn't editing their own account, so a self-edit (Fjord's only use
case) never touches them regardless of whether they're present in the body.

**Real bug, live-reproduced, 2026-07-17: the write path 500'd on first use.**
An earlier pass through this doc claimed the round-trip was "confirmed safe
live" — that was wrong; only the read side (`GET /auth/me`, `GET
.../settings/main`) had actually been exercised against a live server, not
a real `POST`. The first genuine write attempt (Settings → Integrations →
Streaming Region, a real user picking a region) failed with a raw 500,
surfaced by Fjord only as "HTTP status server error (500 Internal Server
Error)" — no detail, because `update_user_settings` called
`error_for_status()` before ever reading the response body. Hand-crafting
the identical POST directly against the live instance got the real message:
`{"message":"SQLITE_CONSTRAINT: NOT NULL constraint failed:
user_settings.locale"}`. Root cause: for any account that has never saved
anything under Seerr's own Settings → General in its web UI — a genuinely
common, unremarkable state, confirmed via that account's `GET /auth/me`
returning `"settings": null` — `GET .../settings/main` simply **omits**
keys like `locale` rather than returning them as `null` (confirmed live:
the response was `{"username":null,"email":"...","movieQuotaLimit":null,
"movieQuotaDays":null,"tvQuotaLimit":null,"tvQuotaDays":null}`, no `locale`
key at all). `fjord_seerr::UserGeneralSettings`'s `locale: Option<String>`
deserializes the missing key as `None` correctly, but plain `Option`
serialization re-emits it on the way out as JSON `null` — and Seerr's
`user_settings.locale` column is `NOT NULL` (empty-string default), so that
explicit `null` reaches the SQL layer unchanged and the whole write fails.
Confirmed live, isolated field-by-field with hand-crafted POSTs against the
same instance: omitting the key entirely succeeds (Seerr falls back to its
own column default and implicitly creates the settings row), sending
`"locale": null` explicitly 500s every time. **Fix:** every field on
`UserGeneralSettings` now carries `#[serde(skip_serializing_if =
"Option::is_none")]` — a field this client never received a real value for
is omitted from the POST body outright rather than sent as `null`, letting
Seerr's own default apply; applied to all eight fields, not just `locale`,
since the same NOT NULL mismatch could exist on any column on a different
Seerr version. `update_user_settings` was also fixed to read the response
body on failure instead of discarding it via `error_for_status()`, so the
next real failure surfaces Seerr's actual error message through the toast
rather than a bare status code — this alone would have made the root cause
obvious on the first try instead of needing several rounds of hand-crafted
diagnostic POSTs.

`GET /watchproviders/regions` (unauthenticated, `{iso_3166_1, english_name,
native_name}[]`, 139 entries confirmed live) populates the Settings →
Integrations → Streaming Region dropdown's list; empty/unset
`streamingRegion` falls back to `"US"` in both `discover::
resolve_streaming_region` (read path, "Currently Streaming On") and
`main.rs::spawn_streaming_region_fetch` (Settings picker) — matching Seerr's
own frontend's identical fallback, found in `src/components/Settings/
SettingsMain/index.tsx`.

### Availability status
`MediaInfo.status` (only present once Seerr has seen the item). **Numbering
confirmed against Seerr's real `server/constants/media.ts`, not assumed —
Fjord originally had this wrong past `Available`** (see `MediaStatus`'s own
doc comment in `fjord-seerr/src/models.rs` for the live bug this caused):

| Value | Meaning | Fjord badge |
|---|---|---|
| *(absent)* | Never requested | none — "Request" shown |
| `1` UNKNOWN | — | none — "Request" shown |
| `2` PENDING | Requested, awaiting approval/processing | "Requested" |
| `3` PROCESSING | Being fetched by Radarr/Sonarr | "Processing" |
| `4` PARTIALLY_AVAILABLE | Some seasons available (TV) | "Partially Available" — Request still shown |
| `5` AVAILABLE | Fully in the library | "In Library" — no Request button |
| `6` BLOCKLISTED | Release blocklisted by Radarr/Sonarr | none — "Request" shown |
| `7` DELETED | — | none — "Request" shown |

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
