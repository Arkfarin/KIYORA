# kiyora / TMV protocol v1

Normative base: [RFC](telegram_music_architecture_ru.md), especially sections 3–10.
Wire codecs: `vault-protocol`; domain payload schemas: `vault-domain::Action`.

Service text/captions use `TMV/1 TYPE\n` followed by compact JSON with `magic=TMV`,
`protocol=1`, `type=TYPE`, `vault_id`. JSON-only records are also accepted if they
explicitly contain the reserved TMV marker. Unknown reserved records, unknown
fields/actions/versions, duplicate keys, invalid UUIDs and malformed identifiers
block replay. User notes are unclassified and are never GC candidates.

Supported records: VAULT_INIT, MEDIA, EVT, BARRIER, SNAPSHOT, SNAPSHOT_COMMIT,
GC_REPORT. Snapshot payload documents will use FULL_SNAPSHOT/state_schema=1.
An EVT contains op_id, device_id, base_generation, entity_id, action and payload,
as in the RFC. Large Telegram IDs are canonical decimal JSON strings. Never use
floating point or lexicographic ordering for them.

Payloads:

| action | entity_id | payload |
|---|---|---|
| track.register | track UUID | metadata, source |
| track.metadata.update | track UUID | patch |
| track.delete | track UUID | {} |
| playlist.create / playlist.rename | playlist UUID | title |
| playlist.delete | playlist UUID | {} |
| playlist.track.add | playlist UUID | track_id, entry_id, after_entry_id |
| playlist.track.remove | playlist UUID | entry_id |
| playlist.entry.move | playlist UUID | entry_id, after_entry_id |
| favorite.set | track UUID | set_name, value |
| source.relink | track UUID | source |
| vault.settings.update | vault UUID | settings |

Metadata patch only changes supplied non-null fields. Empty strings clear optional
text and zero clears optional numeric metadata. Title must remain nonempty.
`clear_artwork=true` clears art and cannot coexist with `artwork`.
Source relink replaces the reference with the same source_id, or adds a new source.
Only gc_device_id and display_name are shared settings in v1.

Canonical bytes sort object keys recursively and emit compact UTF-8 JSON with
deterministic arrays/maps/sets from the domain state. This is a project-specific
integer-only schema, not a claim of arbitrary-number RFC 8785 conformance.
Service JSON has a 64 KiB parser cap in addition to actual Telegram send limits.
The JSON parser retains its depth limit. State validation rejects cycles, dangling
references, live tombstoned objects, invalid hashes and invalid replay boundaries.

Operation receipts and tombstones survive indefinitely (ADR 001). A repeated
identical op_id returns its original result; altered reuse is an integrity error.
SQLite can advance past a later duplicate server message without reapplying it.

This document describes implemented core encoding. Transport, snapshot and API
acceptance remain tracked separately in `docs/implementation-status.md`.
