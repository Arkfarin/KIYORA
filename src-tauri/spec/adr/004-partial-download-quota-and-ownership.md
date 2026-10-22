# ADR 004: Partial download quota and ownership

Status: accepted.

## Decision

A profile owns one cache manager, shared by cloning while the profile's process
lock is held. Each partial download owns a non-cloneable writer lease, keyed by
the hash of its cache key. Another writer, replacement or explicit deletion of
that partial is rejected until its lease is released. Cache clearing skips leased
partials and complete files in use by a decoder.

Starting a download reserves its full declared size. Quota accounting includes
complete files, inactive partial bytes, and active downloads' reserved sizes.
Unused, unpinned complete files are evicted in LRU order when necessary. Inactive
partials stay resumable until explicit cache clearing; they are not silently
discarded to admit another download. Dropping a writer lease releases only future
space: the existing partial bytes remain charged after cancellation or restart.
Stats expose actual bytes and additional reserved bytes separately.

A completed partial is size/hash checked, renamed to its final media path and
published through an atomic index replacement. This avoids making another full
copy during publication. On an index-write failure the file is moved back for
retry. A crash before index publication may require downloading again: reopen
removes unindexed UUID media files, orphaned partial metadata and files in the
reserved `.kiyora-cache-` staging namespace. Unrelated files are left alone.

The quota covers managed payloads and download reservations, not filesystem
metadata, atomic index-write overhead, external TDLib storage, or transient copies
used when importing files from outside the cache.

## Evidence

Tests cover simultaneous reservations beyond quota, duplicate writer refusal,
active-file cleanup protection, pinned/leased LRU exclusions, partial accounting
after reopen, incomplete/digest-invalid publication, index-write rollback and
reopen after a simulated crash between media rename and index publication.
