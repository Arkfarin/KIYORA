# Generated audio fixtures

These files contain a generated 440 Hz sine tone, 48 kHz, stereo, 250 ms.
They contain no sampled recording. They test the configured MP3, FLAC, M4A/AAC
and Ogg/Vorbis decoders and seeking through the remote reader without hardware.

Generated locally with FFmpeg using:

```sh
ffmpeg -nostdin -f lavfi -i sine=frequency=440:sample_rate=48000:duration=0.25 \
  -ac 2 -c:a CODEC tone.EXT
```

Pairs: `libmp3lame/mp3`, `flac/flac`, `aac/m4a`, `libvorbis/ogg`.
Tests use the checked-in files and do not need FFmpeg installed. WAV fixtures are
generated in Rust tests so longer multi-range inputs do not enlarge the repository.
