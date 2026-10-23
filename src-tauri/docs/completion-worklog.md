# Backend completion worklog

Requirements: `spec/backend-task.md`, `spec/telegram_music_architecture_ru.md`; audit: `backend-audit-2026-10-23.md`.

Implemented since audit (verification still being expanded):
- Schema regeneration for staging (regenerate again after API additions).
- Metadata backup restore copies and verifies referenced media; unknown upload outcomes are journaled, never blindly repeated. Fake-server available/missing cases pass.
- Source health persisted locally, explicit missing-source error and unavailable download status. Deleted media leaves track metadata intact; regression passes.
- Named vault creation and JSON-lines backend harness.
- Pending/scheduled TDLib message updates cannot falsely disconnect the vault; regression passes.
- Full export downloads uncached registered media (new; needs live verification).

Real Telegram account run, 2026-10-23, isolated private test vault:
- TDLib QR authorization via existing authorized session, 2FA, session reopen: passed. Password temporary file removed.
- INIT, initial snapshot A/B read-back and commit: passed.
- Import upload and registration, playlist create/add, source health: passed.
- Verified offline download, metadata/full exports, snapshot generation and GC dry-run: passed.
- No GC deletion or writes to personal chats performed.
- Runner state is private under ignored `target/live-acceptance`; credentials are not checked in.

Remaining work tracks:
1. Recovery: corrupt SQLite repair with quarantine/atomic install/crash recovery; same-vault recovery; safe fallback/repair snapshots.
2. Sources: relink workflow and alternate-source selection.
3. Metadata: artwork lifecycle, Telegram audio metadata, heuristic duplicate hints.
4. Bulk import, cancellable jobs, Android SAF.
5. Discovery hints, sync cost/compaction signals, scale acceptance.
6. Remote replay events, diagnostics, storage accounting/TDLib eviction.
7. Playback queue, CLI completeness.
8. Packaging/CI, permanent opt-in account suite, acceptance matrix.
9. Full fmt/schema/clippy/workspace/Android checks and documentation refresh.

Historical audit results are not current acceptance results. No claim of full completion until these tracks are resolved or an external verification limitation is recorded explicitly.
