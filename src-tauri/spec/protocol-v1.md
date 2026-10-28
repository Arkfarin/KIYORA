# kiyora / TMV protocol v1

Normative base: [RFC](telegram_music_architecture_ru.md), especially sections 3–10.
Wire codecs: `vault-protocol`; domain payload schemas: `vault-domain::Action`.
The JSON event envelope is documented in `schema/event-v1.schema.json`, and
`test-vectors/evt-playlist-create-v1.json` is a canonical byte fixture exercised
by the protocol integration tests. The Rust decoder applies stricter
action-specific payload and invariant validation than this envelope schema.

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
| track.metadata.update | track UUID | patch (optional expected_order CAS) |
| track.media.replace | track UUID | metadata, source, expected_order |
| track.delete | track UUID | {} |
| playlist.create / playlist.rename | playlist UUID | title |
| playlist.metadata.update | playlist UUID | patch: optional title, artwork, clear_artwork, expected_order CAS |
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

## Metadata and source extension (2026-10-28)

Optional `composer`, `comment`, `copyright`, `lyrics` and `preferred_source_id`
fields deserialize with defaults for earlier data. A lyrics reference contains a
SHA-256 and a source for a separate bounded version-1 LyricsDocument; snapshots
and GC reference analysis retain it independently of audio. Lyrics text is never
inline HTML. `clear_lyrics` explicitly removes that reference.

`expected_order` is compared to the track's last server update during reducer
replay. A mismatch rejects the entire edit. `track.media.replace` adds a confirmed
new source and switches the preferred source plus metadata atomically, retaining
old references, track identity and playlist/favorite relationships. Telegram
message bytes are not overwritten. The durable media journal handles lost ACKs.

This decoder reads earlier data, but older strict decoders reject these new fields
or actions. Upgrade every participating client before enabling these features;
protocol number 1 does not imply old-client support for this extension. No
existing event, snapshot or user database is rewritten in place by UI startup.
Portable full backups additionally carry checked `lyrics/<track_id>` files. A
new-vault restore must rebind them to confirmed uploads in the target channel;
missing or foreign references are not silently copied or discarded.

### Playlist artwork extension

`Playlist.artwork` is optional and omitted when absent; legacy playlists and
snapshots retain their serialized representation. `playlist.metadata.update`
updates the name and/or cover atomically, rejecting stale `expected_order` and
respecting playlist tombstones. Editor IPC requires an expected version; restore
plans may omit it when recreating a new UUID in an empty vault. An artwork source
must include its SHA-256, confirmed Telegram message reference and positive size
at most 4 MiB. It is a MEDIA document, never an audio overwrite. Live playlist
references participate in snapshot GC protection and orphan classification.

Full backups carry `playlist-artwork/<playlist_id>` with manifest checksums;
new-vault restore uploads and verifies those documents before publishing any
playlist operations. Metadata-only archives require access to the original
attachment. Missing covers fail full export/restore rather than disappearing.

This is an additive v1 extension with fail-closed readers: old clients that do
not recognize the action/field must stay read-only. Upgrade all participating
clients before editing playlist covers; compatibility is old data -> new client,
not new events -> old client.
