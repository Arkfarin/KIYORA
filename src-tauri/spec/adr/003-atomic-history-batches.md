# ADR 003: Atomic history replay batches

Status: accepted.

## Context

Reconstructing the entire SQLite projection and its receipts for every event makes
long-history replay expensive. Server message order, original operation outcomes,
and the atomic projection/receipt/cursor invariant must survive optimization.

## Decision

After sorting and deduplicating history, the sync engine processes bounded groups
of 100 messages. It parses each group before applying its TMV records in one
`IMMEDIATE` SQLite transaction. Cancellation is checked between groups.

The transaction reads the projection once, applies every event through the pure
reducer, and writes the final projection delta. Receipts, command outcomes,
observed-message digests and cursor changes commit in the same transaction.
Duplicates return the original receipt from the in-transaction state, including
duplicates whose first occurrence has not yet been written to SQLite.

Any error rolls back the whole current group. Earlier committed groups remain
durable; the sync engine refuses ONLINE_READY until reconciliation succeeds.
An invalid record may therefore leave a valid prefix of its group unapplied. This
is intentional: no cursor crosses an unverified record, and replay is retryable.

## Verification and limits

- Trigger-injected receipt failure after projection writes, followed by database
  reopen, verifies rollback of all associated state and successful retry.
- Property tests compare mixed actions, duplicates and arbitrary group boundaries
  with the pure reducer and individual-record SQLite application.
- Sync integration exercises a 1,000-event history; the file-backed WAL/FULL
  benchmark also checks reducer equivalence at 10,000 events.

Batching reduces repeated work but still reconstructs the entire state per group.
The sync engine also audits post-barrier receipts for manual deletion. This change
does not establish linear replay complexity or measure live Telegram throughput.
