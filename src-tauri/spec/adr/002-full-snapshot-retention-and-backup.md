# Full snapshot retention and portable backups

Status: accepted for protocol v1.

A verified SNAPSHOT_COMMIT depends on its own complete A/B pair, barrier and
post-barrier events. Its previous-commit pointer records ancestry; recovery does
not need the previous generation's bytes. GC may retire an older group only when
all its members are before the verified cut, its markers match, no other retained
commit references its copies, and both current copies have been reverified. A
partial batch resumes from its persisted exact-message digest allowlist. Unknown
records, staged generations, audio and user messages remain untouched.

Portable `.tmv-backup` uses an uncompressed USTAR envelope with a bounded zstd
state member. Avoiding whole-archive compression permits bounded streaming of
large audio. Only exact documented names and regular files are accepted; no
archive path is passed to extraction. Media is written to generated temporary
names, then exposed only after manifest, state and SHA-256 validation. These hashes
detect corruption; they do not establish ownership or authenticity.

Full backups require all active audio bytes. Metadata-only backups retain source
references as historical information. Restoration into another vault requires
new confirmed source references, regenerates track/playlist/entry identities and
operation IDs, and clears the previous device's coordinator role. It never imports
old Telegram references as if they were available in the destination.
