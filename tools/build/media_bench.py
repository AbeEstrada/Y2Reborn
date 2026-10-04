#!/usr/bin/env python3
"""Measure product parse + initial/incremental scan paths on disposable mixed libraries.

Build: cargo build -p reborn-library --example media_bench --release --locked
Run: python3 tools/build/media_bench.py --output out/media-benchmark
Output must not already exist. No global drop_caches, device access or media mutation.
For ARM, --binary may be a wrapper that runs the built example through qemu-arm.
"""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import shutil
import subprocess
import struct

ROOT = Path(__file__).resolve().parents[2]
NAMES = ['flac-24-96000.flac', 'mp3.mp3', 'aac.aac', 'm4a-aac.m4a', 'm4a-alac.m4a', 'vorbis.ogg', 'opus.opus', 'wav-pcm24.wav', 'wav-float32.wav', 'aiff-pcm24.aiff', 'ape-silence.ape', 'wavpack-silence.wv', 'artwork-jpeg.flac', 'artwork-png.flac', 'artwork-webp.flac']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/examples/media_bench')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--counts', type=int, nargs='+', default=[100, 1000, 10000])
    parser.add_argument('--long-wave', action='store_true', help='also profile a sparse 180-second PCM WAV without copying it into libraries')
    parser.add_argument('--repeats', type=int, default=3)
    args = parser.parse_args()
    if not 1 <= args.repeats <= 10 or any(not 1 <= n <= 20000 for n in args.counts):
        parser.error('bounded counts 1..20000; repeats 1..10')
    output = args.output.resolve()
    output.mkdir()  # Deliberately refuses reuse of old DB/evidence.
    media = output / 'parse-fixtures'
    media.mkdir()
    for name in NAMES:
        shutil.copyfile(ROOT / 'assets/fixtures' / name, media / name)
    if args.long_wave:
        size=180*48000*4
        with (media/'long-pcm16.wav').open('wb') as file:
            file.write(b'RIFF'+struct.pack('<I',size+36)+b'WAVEfmt '+struct.pack('<IHHIIHH',16,1,2,48000,192000,4,16)+b'data'+struct.pack('<I',size))
            file.truncate(size+44)
    def run(mode, directory, *extra):
        command = [str(args.binary.resolve()), mode, str(directory), *map(str, extra)]
        result = subprocess.run(command, capture_output=True, text=True, check=True, timeout=900)
        return {'command': command, 'result': json.loads(result.stdout), 'stderr': result.stderr}
    report = {'machine':platform.uname()._asdict(), 'cache':'OS page cache uncontrolled; short deterministic mixed fixtures, not cold SD bandwidth', 'fixtures':{name:hashlib.sha256((media/name).read_bytes()).hexdigest() for name in NAMES}, 'parse':run('parse',media), 'scans':[]}
    (output / 'results.json').write_text(json.dumps(report, indent=2)+'\n')
    for count in args.counts:
        library = output / f'library-{count}'
        library.mkdir()
        for i in range(count):
            name=NAMES[i%len(NAMES)]
            parent=library/f'artist-{i//1000:02}'/f'album-{i//10:04}'
            parent.mkdir(parents=True,exist_ok=True)
            shutil.copyfile(media/name,parent/f'{i:05}{Path(name).suffix}')
        for repeat in range(args.repeats):
            report['scans'].append({'tracks':count,'repeat':repeat,**run('scan',library,output/f'db-{count}-{repeat}')})
            (output/'results.json').write_text(json.dumps(report,indent=2)+'\n')
        print(f'{count} tracks: {args.repeats} initial + incremental scans completed',flush=True)


if __name__ == '__main__':
    main()
