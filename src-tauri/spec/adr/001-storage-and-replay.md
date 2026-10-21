# ADR 001: protocol and local projection

Use rusqlite with bundled SQLite/FTS5: synchronous transactions make the reducer,
outcome and cursor atomic. The application runs database work on a blocking worker;
the domain crate has no runtime, network, filesystem or clock dependencies.

Use signed i64 identifiers in Rust and INTEGER in SQLite. Public JSON uses canonical
decimal strings (no plus, whitespace, leading zero, negative zero or exponent).
Final message IDs must be positive; chat IDs may be negative. Temporary TDLib IDs
never enter replay. Object identifiers are independent UUIDs.

v1 retains tombstones and operation receipts indefinitely, including in snapshots.
This proves idempotence even when an old op_id reappears after compaction. No bounded
receipt pruning is claimed safe. Conflicting reuse of an op_id is an integrity error.

Entry ordering is an acyclic after-entry forest, siblings sorted by the last valid
position's (server message ID, entry UUID), ascending, visited depth first. Move
detaches the entry and reparents its children to the old parent before reinserting;
remove does the same and retains the removed UUID tombstone. References to deleted
anchors are rejected. A null anchor means the root slot, whose concurrent insertions
use the same sibling rule. Track/playlist deletion tombstones all affected entries.

Shared settings are a closed schema, never arbitrary local paths or secrets.
Invalid known actions produce persisted rejected outcomes. Unknown actions/fields,
versions, wrong vault and out-of-order replay stop before cursor advancement.

The RFC JSON examples are illustrative; the complete implemented wire schema and
canonical fixtures are authoritative refinements, with no silent unknown fields.
