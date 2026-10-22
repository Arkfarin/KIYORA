# ADR 005: Bounded streaming playback workers

Status: accepted.

## Decision

Playback resolves a registered track/source using the reconciled sync engine. A
verified complete cache entry remains the first choice and works offline. Otherwise
an online source is resolved to a TDLib file and a seekable remote reader.

The reader requests aligned 256 KiB ranges and prefetches one subsequent range.
It copies only the ready bytes from TDLib's local file, validates range boundaries,
and supports backward/forward seeks without downloading intervening bytes. TDLib
stores ranges at absolute local file offsets; requests for a file are serialized
across cloned transports. Transient requests have bounded retries and honor rate
limits; cancellation and session lifetime stop in-flight reads/prefetch.

A dedicated decoder thread converts audio to 48 kHz stereo PCM. At most eight
4,096-sample blocks wait in its queue, plus the producer and consumer blocks. The
OS audio callback consumes queued samples with a nonblocking lock attempt and
returns silence on underflow. Silence does not advance track position. Filesystem,
network, decoder initialization and decoding never run in that callback.

Seek invalidates queued PCM, cancels the previous read and reconstructs/seeks the
decoder on its worker. Pause stops consumption, so the queue bounds prefetch.
Stop, profile switch, logout and shutdown cancel reads and join decoder/output
workers before closing the native session. `buffering` is an explicit playback
state; read/decode failures emit failure instead of appearing as successful EOF.

The CLI keeps `playback play` alive and accepts controls within that process.
Independent CLI invocations do not share an audio engine.

## Integrity and limits

Streaming chunks are transient and are not published as fully verified cache
entries. A whole-file digest cannot authenticate an arbitrary prefix before the
remaining bytes are available. Explicit offline downloads still verify full size
and digest before publishing to the cache. TDLib's own disk storage is separate
from the bounded compressed/PCM memory used here.

Tests prove remote reading, prefetch, retry, cancellation, seek, buffer bounds,
cache leases, session fencing and real decoding of WAV/MP3/FLAC/M4A/Vorbis data
through the simulator. They do not prove live Telegram throughput, audible output
on hardware, or Android media-session/foreground-service behavior.

Rodio 0.21.1 may initially expose a zero-length Vorbis packet. The decoder worker
primes one sample before constructing the uniform source, preserving that sample,
so an empty packet is not mistaken for end-of-stream. A checked-in tone covers it.
