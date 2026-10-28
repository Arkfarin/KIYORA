# Playlist artwork and recovery completion

Date: 2026-10-28. Release status remains **RELEASE_BLOCKED** pending the real-device
and real-account scenarios in [music UI acceptance](MUSIC_UI_ACCEPTANCE.md).

## Audit and scope

The clean local working tree already contained the responsive Kiyora app, setup,
authentication, discovery/recovery, library/search/albums/artists, Rust playback
and queue, imports, metadata/artwork/lyrics editing, downloads, settings and
backups. These systems were retained. This increment closes the custom playlist
cover gap and the playlist form's mutable-payload retry gap.

## User-visible changes

- Existing playlists have an **Изменить плейлист** editor for the title and custom
  cover. JPG/PNG/WebP selection, bounded Rust validation, square crop, zoom and
  position controls reuse the music metadata editor. Cover removal restores the
  generated track mosaic and does not delete the remote document.
- The cover appears in the playlist page/cards, home playlist rows and sidebar.
  Read requests fetch a small image independently of audio, with the shared lazy
  cover cache, request concurrency limit and object URL cleanup.
- Creation and editing retain drafts in profile/Vault-scoped IndexedDB. They
  preserve UUIDs and the exact request across close/reopen and app reload. The
  editor is accessible offline; only local drafting is allowed until ONLINE_READY.
- An uncertain send locks its payload and keeps its operation ID. The retry button
  checks the same operation. A definitive preflight/reducer rejection permits a
  corrected draft. Concurrent metadata changes require explicit review/rebase.
  Deleted playlists cannot be recreated by an old editor.
- Page headings wrap before actions can squeeze a title out of the tablet layout.
  The existing palette, fonts, theme system, mobile navigation and player remain.

New playlist creation keeps its small title form; custom artwork can be added
through the playlist editor after creation. No new Telegram channel or account is
created by this feature.

## Backend, protocol and data compatibility

`VaultRequest::EditPlaylist` / `edit_playlist` takes `op_id`, `playlist_id` and a
`PlaylistEdit` containing title, optional image bytes, explicit cover clearing and
required `expected_order`. Rust enforces size limits, valid image decoding, online
readiness, persisted request fingerprints and server ordering. A cover uses the
existing durable attachment publication journal, stable derived UUID, SHA-256,
server confirmation and independent read-back. No audio is fetched or rewritten.

The `playlist.metadata.update` event carries a `PlaylistPatch`. The reducer checks
its expected version, validates the entire patch and changes title/cover
atomically; playlist and entry UUIDs remain unchanged. Playlist tombstones win.
`Playlist.artwork` is an optional `Artwork` reference, omitted when absent. A
regression test preserves the old playlist JSON representation. SQLite already
stores versioned JSON projections, so no SQL table migration is required.

`BackendCommand::PlaylistArtwork` / `playlist_artwork` reads the current cover.
The shared native loader checks the active session, Vault channel, ownership,
size and digest, and uses the existing leased cache. Frontend types and the
checked-in Rust-generated IPC schemas include the extension.

Snapshots include the new optional field. `State::media_sources()` covers both
track attachments and playlist artwork; snapshot GC, execution-time GC protection
and orphan classification use it. No physical-delete shortcut is introduced.

Full portable backups carry bounded, checksummed `playlist-artwork/<playlist_id>`
files. Missing custom covers fail full export. New-vault restore validates the
local file, publishes and reads back its bytes using the attachment journal,
then builds playlist operations with the new source. Lost ACKs reuse that
publication. Metadata archives need access to the original attachment. Old
archives without custom covers retain their existing import path.

New clients read old data. **Old strict clients cannot read this new action or
field** and must fail closed/read-only. Upgrade all participating clients before
using the extension. Existing snapshots/events are not rewritten on startup.
The protocol compatibility constraint is documented in
[protocol-v1](../src-tauri/spec/protocol-v1.md).

## Changed modules

| Area | Files |
|---|---|
| Playlist UI | `src/features/playlists/PlaylistForm.tsx`, `Playlists.tsx` |
| Shared cover rendering | `src/components/music/Cover.tsx`, `src/services/artwork.ts`, `src/features/metadata-editor/ArtworkEditor.tsx` |
| Home/sidebar/responsive layout | `src/features/library/LibraryScreens.tsx`, `src/components/layout/Shell.tsx`, `src/styles/app.css` |
| Frontend contracts/drafts | `src/backend.ts`, `src/services/drafts.ts` |
| Native application service | `src-tauri/crates/vault-core/src/playlists.rs`, `api.rs`, `ipc.rs`, `lib.rs`, `service.rs`, `backend/artwork.rs` |
| Backup export/restore and orphan tracking | `vault-core/src/backend.rs`, `backups.rs`, `imports.rs`; `vault-snapshot/src/backup.rs` |
| Domain and safe collection | `vault-domain/src/model.rs`, `reducer.rs`; `vault-snapshot/src/gc.rs`; `vault-sync/src/gc.rs` |
| Supporting compatibility/lint | `vault-sync/examples/scale-benchmark.rs`; `vault-core/src/imports/rewrite.rs` (Clippy-only simplification) |
| Contracts/documentation | `src-tauri/spec/schema/{backend-request-v1,backend-response-v1,backend-event-v1,event-v1}.schema.json`, `src-tauri/spec/protocol-v1.md`, `docs/MUSIC_UI_ACCEPTANCE.md`, this report |
| Tests | `vault-core/tests/service.rs`, `vault-domain/tests/playlist_metadata.rs`, `vault-protocol/tests/wire.rs`, `tests/ui/harness.tsx`, `tests/ui/playlists.spec.ts` |

Crate paths in the table are under `src-tauri/crates/`. No dependency, permission,
CSP, TDLib adapter or audio-engine replacement was required.

## Verification

Final checks on this working tree:

- `pnpm build`: passed (strict TypeScript and production Vite bundle).
- `pnpm format:check`: passed.
- `pnpm test:ui`: **17 passed**, including 320/390/768/1440 px and 10,001-track paging/search.
- `env -u KIYORA_TDJSON bash src-tauri/scripts/check-backend.sh`: passed;
  Rust formatting, locked workspace/all-targets check, strict Clippy, **220 tests
  passed / 0 failed / 2 opt-in tests ignored**, CLI smoke check and regenerated
  request/response/event schema equality.
- Production-minified component build plus real Chromium inspection:
  **0 layout failures**, with `covered-text` explicitly allowed for native modal
  background occlusion and one documented secondary-text warning.
- Targeted frontend design lint: 0 failures, 0 warnings; `git diff --check`: passed.

The Rust integration tests use temporary databases/files and the existing
account-free Telegram transport. Browser fixtures remain isolated in `tests/ui`
and are excluded from the app bundle. They reject cloud mutations and verify
request identity/draft UX, not real Telegram connectivity.

New coverage includes atomic/stale/deleted playlist edits, invalid cover hash and
size, old JSON compatibility, lost attachment ACK followed by process restart,
changed-payload rejection, duplicate save, corrupt image rejection without remote
side effects, orphan classification, snapshot cold restore, full-archive cover
preservation, missing-cover rejection, restore ACK recovery and cover clearing
without physical deletion. Browser checks cover offline drafts at 320/390/768/1440
px, reload/retry identity for creation and editing, and definitive preflight failure.

Design follows the existing `true-black-flat` sheet (seed 890120914), with the
user-specified Kiyora palette and bundled Manrope/Space Grotesk; no redraw or sample
music was added. The editor answers “Which name and cover will this playlist use?”
with the editable title and artwork preview. The browser inspector's `covered-text`
rule is explicitly allowed for this view: a native modal intentionally covers
and makes the background app inert. Secondary metadata text remains 11–12 px.
Screenshots/inspection artifacts are local under `.taste-skill/playlist-production`.
Desktop/tablet/mobile screenshots were inspected: the title and cover stay readable,
actions remain reachable, columns stack on phones and the modal scrolls at 320 px.
No change to the established design direction was necessary.

## Remaining release acceptance

The new cover path still needs a real Linux/Android pair: edit on one device,
receive on another, restart and view from cache, conflict two editors, interrupt
a real upload, and restore a disposable Vault from a full archive. Android's
photo picker, cutouts, keyboard, text scaling and crop interaction need physical
hardware verification. Existing audio/auth/TDLib/installer acceptance obligations
remain unchanged. No live-account test, remote deletion, release, commit or push
was performed for this increment.

For the existing-track metadata update/explicit retagging mechanism, audio and
lyrics format matrix, cache/deletion semantics and Linux/Android launch commands,
see [music UI implementation and acceptance](MUSIC_UI_ACCEPTANCE.md).
