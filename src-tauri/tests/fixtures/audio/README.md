# Controlled audio fixtures

Synthetic 440 Hz tones, 0.25 seconds, stereo. These files are only test inputs; the app has no FFmpeg dependency.

FFprobe independently confirmed the expected values used by the public library workflow tests:

| File | Codec | Sample rate | Channels | Duration |
| --- | --- | --- | --- | --- |
| stereo44100.mp3 | MP3 | 44,100 Hz | 2 | 0.250 seconds |
| stereo32000.ogg | Vorbis | 32,000 Hz | 2 | 0.250 seconds |
| stereo48000.flac | FLAC | 48,000 Hz | 2 | 0.250 seconds |
| unsupported-ogg-flac.ogg | FLAC in Ogg | 32,000 Hz | 2 | 0.250 seconds |

Generated once with FFmpeg 9.0.2 using `sine=frequency=440:sample_rate=<rate>:duration=0.25`, stereo output, and respectively `libmp3lame`, `vorbis -strict -2`, or `flac`. The unsupported input uses `flac -f ogg`, verifying that an `.ogg` extension does not establish Vorbis support.

The PCM WAV test writes its own ordinary RIFF header and two seconds of 22,050 Hz mono 16-bit silence. No encoder executable is needed when running tests.

`malformed-vorbis-lookup.ogg` is a bounded 186-byte malformed Ogg setup: one
ordered codebook, 32 dimensions, one entry and lookup type 1. Its valid page CRCs
let Symphonia's mapper reach an unchecked integer-power assertion during format
probing. It has no large allocation payload. Public Library tests require ordinary
metadata/waveform error results instead of unwinding; it is not a playback fixture.
