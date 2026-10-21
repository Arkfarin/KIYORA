---
name: tmv-sync-recovery
description: Build and verify Telegram Music Vault event reducers, SQLite transaction projections, online-only mutation guards, live/catch-up synchronization, full barrier snapshots, crash-safe recovery and conservative garbage collection. Use for sync correctness, protocol migration and safety-critical data lifecycle work.
---

# TMV: Synchronization, snapshots and safe GC

Follow `AGENTS.md` and `telegram_music_architecture_ru(1).md` (or renamed equivalent), especially §§3, 5–10, 14–16, 18, 20–23. Preserve wire-format compatibility and client safety.

## Engineering procedure

1. Define a pure, deterministic reducer and an explicit `Applied | Rejected(reason) | NoOp(reason)` outcome contract. Identity, server ordering and delete-wins tombstones are central invariants.
2. Persist outcomes, unique `op_id` and cursor in one atomic SQLite transaction. Treat unknown control records/protocol versions as read-only blockers, not ignored messages.
3. Build sync as a state machine: authorization, validated vault, subscribing/buffering updates, history catch-up (reverse pages), sorted replay, deduplication, head reconciliation, and only then `ONLINE_READY`. A send ACK is not application success.
4. Make snapshot creation barrier-based: publish and confirm the server barrier; replay all valid events before it; canonically serialize full objects, order, source references and tombstones; zstd-compress and hash; upload **two** copies; independently read/verify both; publish and verify a separate commit.
5. During cold restore, select the newest **valid** commit, validate A then B, transactionally install projection, replay only post-barrier events and reconcile buffered updates. If restoration cannot be proven safe, use `DEGRADED_READONLY`.
6. Make GC conservative and opt-in: one designated coordinator, a persisted allowlist/dry-run plan, only obsolete service records, batch verification and crash recovery. Never delete active or unclassified MEDIA or ordinary user messages. Old snapshots/commits may have complex dependencies; do not use only `id <= barrier` as proof of safety.
7. Use a fake multi-client Telegram adapter with interleavings, missing ACK, `FLOOD_WAIT`, partial history, corrupt A/B, server message ID gaps, crashes at each commit/GC step and incompatible protocol versions.

## Invariant-oriented acceptance tests

- Duplicate `op_id` produces exactly one mutation; repeated replay converges.
- No stale add/rename/create resurrects a tombstoned UUID.
- Two or three clients converge despite concurrent renames/moves/deletes and delayed updates.
- `restore(Snapshot@B) + events(>B)` matches pre-GC full replay state.
- Missing/corrupt A recovers using B; missing both cannot silently reinitialize.
- New post-barrier events are never pruned; GC never selects active media.
- A failed DB transaction cannot advance the cursor without applying outcome.
- Inflight commands with unknown ACK are reconciled before potential retries.

If correctness can't be proven, return a meaningful error and remain read-only. Document the known limits of Telegram (no cross-message transactions/CAS and no guarantee against manual deletion of critical records).
