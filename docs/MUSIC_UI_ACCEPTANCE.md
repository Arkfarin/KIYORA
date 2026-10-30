# Kiyora music interface — implementation and acceptance

Updated 2026-10-28. Release status: **RELEASE_BLOCKED**. This report describes the
current working tree, including work already present when implementation resumed.
It does not turn earlier backend/platform reports into acceptance of this UI.

The latest increment adds synchronized custom playlist covers and durable playlist
editor/create drafts. Details and changed modules: [playlist artwork implementation](PLAYLIST_ARTWORK_IMPLEMENTATION.md).

## UX overhaul increment (2026-10-29)

The implemented player/navigation/native-media changes, current verification results,
comparison screenshots and remaining device acceptance are recorded in
[UX_OVERHAUL_REPORT.md](UX_OVERHAUL_REPORT.md). That report supersedes the player
composition, volume UI and lyrics presentation described below; release remains blocked
on device acceptance. Earlier test results below describe their original increment.

## Implemented paths

- Native bootstrap and profile/session/Vault recovery; welcome, QR/phone/code/2FA
  and supported TDLib authorization states; discovery, deep/manual attachment,
  explicit creation/resumption and initial synchronization. An empty valid Vault
  opens the library. An existing binding is not replaced by a new-vault wizard
  after a transient connection failure.
- Dark/light/system appearance, bundled Manrope and Space Grotesk with OFL notices,
  desktop sidebar, mobile bottom navigation, accessible dialogs and focus,
  persistent Rust-backed player, queue editing, shuffle/repeat, fullscreen player,
  bounded cached cover colour sampling and reduced-motion support.
- Real-library home, recent additions, local completed-listening history and play
  counts, random selection, tracks/albums/artists/favorites/playlists, local search,
  sorting, selection, grid/list and bounded pages (50 tracks / 100 playlist entries).
  Playlist search uses SQLite FTS across the complete playlist; results retain
  occurrence IDs and original playback positions. Move anchors cross page borders.
- Native file selection and desktop drop, Rust staging/preview, separate original
  upload and edit paths, duplicate review, durable jobs and recovery. Audio bytes
  never pass through React. Original upload retains the original SHA-256.
- Shared metadata editor, cover replacement/crop and normalized lyrics editor;
  catalog edits and explicit file-copy retagging have distinct controls. IndexedDB
  stores profile/Vault-scoped drafts, not Telegram credentials. Unknown send
  outcomes retain the exact request, mode and operation ID across reopening.
- Lyrics are separate hashed Vault documents referenced by metadata events and
  snapshots, so another device can fetch them without downloading audio. Full
  backups now carry bounded `lyrics/<track_id>` documents; new-vault restore checks
  them and uploads/rebinds references before publishing catalog operations.
- Playlist cover upload/crop/removal, atomically versioned title/cover edits,
  persistent offline drafts and exact retries after lost acknowledgements. Covers
  appear in playlist cards, the home page and sidebar; the default remains a mosaic.
  Full backups contain verified `playlist-artwork/<playlist_id>` documents; restore
  rebinds them to the target Vault before publishing playlist operations.
- Downloads, cancellation/retry, pinned cache, cache quota/cleanup, session and
  Vault settings, redacted diagnostics, snapshot/backup controls, logical deletion
  with tombstones and playlist-only/local-cache-only removal.

## Backend and compatibility

The Rust playback, TDLib adapter, SQLite projection, session ownership and sync
engine remain authoritative. `src-tauri` is the active Rust workspace.

The existing implementation adds optional metadata fields (composer, comment,
copyright, lyrics), `MetadataPatch.expected_order` and `Track.preferred_source_id`.
`track.media.replace` publishes a confirmed new source plus metadata atomically,
checks the expected server order, preserves `track_id`/playlist entries/favorites,
and retains older sources. A playing audio lease is not a frontend audio session.

Normal logical editing publishes only attachments that changed and a versioned
metadata event. It does not fetch/reupload audio. Explicit retagging obtains the
current source, writes a private copy through Lofty, verifies it, hashes and uploads
it, then publishes the source switch. Lost acknowledgements are reconciled with
stable markers and journals. Retagging legacy embedded-only covers now preserves
and separately publishes their artwork. Backup export chooses the preferred audio
version, rather than silently archiving a retired version.

New read IPC includes `playlist_artwork`, `downloads`, `listening_history` and local
`playlist_search`. Playlist items include original `position`, `move_up_after` and
`next_entry_id`. `EDIT_NOT_SENT` is a definitive fresh-editor preflight rejection;
it permits correction of a draft. Network/timeouts/unknown outcomes do not.
`edit_playlist` accepts local artwork bytes and an expected playlist version;
`playlist.metadata.update` applies the name/cover patch atomically. Playlist cover
references are retained by GC protection and orphan analysis.
Checked-in request/response/event schemas are generated from Rust and compared by
the backend gate. Existing commands retain their names.

No SQL table migration is needed for these additions: projections already store
versioned JSON; new fields deserialize with defaults. Device-only listening
history is a bounded `listening.v1:<vault_id>` entry in `local_settings`, capped at
1,000 tracks, recording completed playback IDs idempotently. It is deliberately
not presented as cross-device listening history.

New code reads old events/snapshots. **Old binaries with strict unknown-field and
unknown-action rejection cannot consume the new metadata/source extensions.** All
participating clients must be upgraded before using those features. This is an
explicit protocol compatibility/release acceptance constraint, not a claim of
bidirectional compatibility. Old archives without standalone lyrics remain
readable; restoring referenced attachments requires their accessible original
Vault unless they are present in a full archive. Unsafe foreign references are
never silently accepted into a new-vault restore plan.

## Formats and limits

- Audio input allowlist: MP3, FLAC, M4A/MP4, AAC, OGG, Opus, WAV, AIFF/AIF; actual
  parsing is performed in Rust. The backend input cap is 8 GiB, with Telegram's
  effective account limits applied by the existing upload pipeline. Codec support
  on each target must be checked with actual native playback, not inferred from
  extension or successful metadata parsing.
- Embedded tags: Lofty-supported ID3/Vorbis/MP4 and other supported container tags;
  explicit existing-track rewriting excludes raw AAC. Unsupported field/container
  combinations remain in Kiyora metadata. Synchronized lyrics are embedded as LRC
  text, not advertised as native ID3 SYLT or per-word karaoke support.
- Lyrics: plain text, LRC (including multiple timestamps and offset), SRT, WebVTT;
  typed version-1 line model with optional start/end milliseconds. Import text is
  capped at 128 KiB, documents at 2,000 lines and stored JSON at 512 KiB. Invalid,
  mixed and decreasing timings are rejected. Enhanced LRC word timing and VTT
  styling are not implemented; lyrics render as text without HTML interpretation.
- Artwork: JPG/PNG/WebP, bounded decoding (8192 px per dimension, 128 MiB decoder
  allocation); edit input up to 8 MiB, normalized output up to 4 MiB, at most 1600 px.
  Crop works on an owned copy. Original files are preserved.

## Automated evidence

Latest working-tree results: frontend build/format passed; 17 Chromium tests passed;
backend gate passed with 220 tests and 2 opt-in tests ignored. Detailed evidence for
the latest increment is in [playlist artwork implementation](PLAYLIST_ARTWORK_IMPLEMENTATION.md).
Tests use temporary SQLite/files and the existing deterministic test transport.
The isolated browser entry in `tests/ui` never enters the production app build;
its mutation transport rejects sends and proves request/draft behavior only.

- `pnpm build`: TypeScript and production frontend bundle.
- `pnpm format:check`: frontend/test formatting.
- `pnpm test:ui`: real Chromium component checks for 320/390/768/1440 px, 10,001
  track paging/search, offline draft restoration, explicit tag mode persistence,
  exact retry identity, definitive preflight rejection, and queue preservation.
- `env -u KIYORA_TDJSON bash src-tauri/scripts/check-backend.sh`: Rust formatting,
  locked workspace/all-targets check, strict Clippy, workspace tests, CLI help and
  generated-schema equality. Opt-in account/native tests are not enabled.
- Integration coverage includes original upload/hash/idempotence, logical lyrics
  edits and snapshot restore, CAS conflicts, lost-ACK physical replacement,
  unchanged original files/identity/favorites/playlist entries, legacy artwork,
  complete lyrics archive/rebinding, FTS duplicate occurrences and history reopen.
- Browser inspection of real components and production-minified component build
  is stored in `.taste-skill/production-library`; normal-width measurements have
  no layout failures. Secondary metadata typography is intentionally 11–12 px.
  The skill inspector assigns an artificial 200 px height to an unlisted 320 px
  width; the 320 px acceptance checks use explicit browser viewport sizes instead.

## Scenario coverage and remaining manual checks

| Requested scenarios | Implemented protection | Still needs release acceptance |
|---|---|---|
| 1–4: first run, lost local data, auth without Vault, partial creation | Profile-aware onboarding, discovery/manual binding, durable creation journal, explicit resume | Real-account discovery and interrupted creation |
| 5–6: syncing and offline startup | Matching local projection, read-only state, backend mutation guard | Reopen a real cached Vault without network |
| 7–10: interruption, lost ACK, double import, duplicates | Durable operation/attachment markers, stable IDs, original-byte staging, duplicate review | Actual TDLib reconnect/FloodWait and lost response |
| 11–14: absent tags/art, corrupt cover, bad lyrics | Filename fallback, cover fallback, bounded image decoder and lyrics validation | Native picker/codec/tag fixtures on each platform |
| 15–16, 30: offline/concurrent edit | Persistent exact drafts, expected-order CAS, explicit conflict review, definitive-not-sent distinction | Two real devices editing metadata/lyrics concurrently |
| 17–19: delete playing/playlist track, metadata vs remote cache | Tombstones, stable IDs, source-aware cache, existing engine leases | Audible continuity and cache refresh across devices |
| 20: crash during replacement | Durable copy/upload intent, atomic source event, old references retained | Kill/restart installed app at each actual transfer boundary |
| 21–22: invalid auth/profile switch during upload | Session-scoped commands/events, cancellation and recovery journals | Real TDLib session invalidation and switched-account uploads |
| 23–25: Android background, lockscreen, route change | Existing native media service, Rust engine and native lifecycle handling retained | Physical Android media notification/headset/route/background tests |
| 26: 10,000+ tracks | Bounded rendered pages, lazy cover cache, SQLite search | Device memory/latency measurement and very large cover collections |
| 27–29: missing source, old Vault, new snapshot while viewing | Source health/recovery, verified snapshots+tail, session/sequence checks | Old production Vault fixtures, actual remote missing messages |

No real Telegram account, remote deletion or release publication was used for
this increment. Browser tests do not validate native authorization/audio/JNI.
Historical Android/Linux packaging evidence does not validate this final UI.

Physical deletion of Telegram audio is not exposed: the current GC coordinator's
supported scope must be respected; no direct-delete shortcut was introduced.
Local removal, playlist removal and shared tombstone deletion are separate paths.
Output-device selection and download pause are not shown where their backend
contracts do not exist. Custom playlist covers now synchronize; removal returns
to the default mosaic. Cover selection is available in the existing playlist editor
after creating a playlist. Uncertain attachment sends can remain blocked until reconciliation can
prove their outcome; the UI does not automatically resend them.

Before distribution: test installed Linux/Windows/Android builds, keychain/session
restoration, QR rotation/2FA, safe areas/keyboard/large system text, content-URI
multi-file imports, audible streaming/offline/seek, hardware output changes,
background/lockscreen playback, cross-device conflicts/lyrics, both snapshot copies,
backup restore into a disposable Vault, installer CSP/capabilities and old-client
compatibility. Live-account work and destructive acceptance need separate consent.

## Run locally

From the repository root:

```sh
pnpm install --frozen-lockfile
pnpm build
pnpm test:ui
# Optional custom Chromium location:
KIYORA_TEST_BROWSER=/path/to/chromium pnpm test:ui
# Desktop development with a locally prepared, pinned TDLib:
KIYORA_TDJSON=/absolute/path/to/libtdjson.so pnpm tauri dev
```

Use [native packaging](../src-tauri/docs/packaging.md) for the pinned TDLib build,
licenses and required Linux GTK/WebKit/ALSA toolchain. From `src-tauri`:

```sh
python3 scripts/package-desktop.py --debug
# Android: Rust aarch64-linux-android target, JDK 17+, SDK 36, NDK r28c
export ANDROID_HOME=/absolute/path/to/android-sdk
export NDK_HOME="$ANDROID_HOME/ndk/28.2.13676358"
bash scripts/package-android.sh --debug
# Install/run on a selected development device (commands not executed here):
adb install -r gen/android/app/build/outputs/apk/universal/debug/*.apk
adb shell am start -n io.github.arkfarin.kiyora/.MainActivity
```

These commands build locally; they do not publish a release. Android physical-device
acceptance remains separate from compiling an APK.
