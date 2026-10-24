# Generated audio fixtures

These files contain a generated 440 Hz sine tone, 48 kHz, stereo, 250 ms.
They contain no sampled recording. They test the configured MP3, FLAC, M4A/AAC
and Ogg/Vorbis decoders and seeking through the remote reader without hardware.
Additional fixtures cover raw AAC, AIFF, M4A/ALAC and Ogg/Opus.

Generated locally with FFmpeg using:

```sh
ffmpeg -nostdin -f lavfi -i sine=frequency=440:sample_rate=48000:duration=0.25 \
  -ac 2 -c:a CODEC tone.EXT
```

Pairs: `libmp3lame/mp3`, `flac/flac`, `aac/m4a`, `libvorbis/ogg`.
Additional pairs: `aac/aac`, `pcm_s16be/aiff`, `alac/m4a` (named
`tone-alac.m4a`), `libopus/opus`.
`tone-long.opus` uses the same command with duration 3.25 seconds; it verifies
cross-page seeks, exact decoded sample counts and bounded pre-roll convergence.
Tests use the checked-in files and do not need FFmpeg installed. WAV fixtures are
generated in Rust tests so longer multi-range inputs do not enlarge the repository.
