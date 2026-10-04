# Reborn audio fixtures

The generated fixtures are deterministic 440 Hz, ramp, artwork, metadata and
corruption inputs produced locally with FFmpeg 9.0.1 by
`tools/build/fixtures.py`. Their hashes are recorded in `manifest.json`.

`ape-silence.ape` and `wavpack-silence.wv` are small decoder specimens because
the host FFmpeg build does not provide APE or WavPack encoders. They were
retrieved on 2026-09-18 from the Wavecor test-disc files
[`Track73.ape`](https://www.wavecor.co.uk/TestDisc/Track73.ape) and
[`Track73.wv`](https://www.wavecor.co.uk/TestDisc/Track73.wv). They remain
decoder test inputs; the generated-fixture CC0 statement does not apply to
these two files.

The 2026-10-04 [metadata fixtures](metadata/manifest.json) add Unicode/ID3/APE/
WAV/AIFF tags, malformed values and bounded-artwork cases. Regenerate their
metadata with `python3 tools/build/metadata_fixtures.py`; these fixture writers
are not production parsers. `tools/build/jpeg_fixture.c` uses direct host FFmpeg
libraries to generate the 4000×4000 JPEG. Generated metadata/images are CC0;
the underlying original media keeps the provenance above. The
[Platform 1.0 media receipt](../../docs/validation/REBORN-PLATFORM-V1-FFMPEG-METADATA.md)
records tests and reproducible benchmarks.
