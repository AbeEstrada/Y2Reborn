#!/usr/bin/env python3
"""Exercise artwork dimensions, color, progressive JPEG and rejection limits.

Synthetic inputs live only in a temporary host directory. GPU presentation and
physical power are not exercised. Optional --baseline compares unchanged paths.
"""
import argparse
import ctypes
import json
from pathlib import Path
import tempfile

from PIL import Image


def decoder(path):
    library = ctypes.CDLL(str(path.resolve()))
    function = library.rb_media_art_file
    function.argtypes = [ctypes.c_char_p, ctypes.POINTER(ctypes.c_uint8), ctypes.c_int]
    function.restype = ctypes.c_int
    return function


def pixels(function, path):
    output = (ctypes.c_uint8 * (160 * 160 * 4))()
    result = function(str(path).encode(), output, 160)
    return result, bytes(output)


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--library', type=Path, required=True)
parser.add_argument('--baseline', type=Path)
args = parser.parse_args()
decode = decoder(args.library)
baseline = decoder(args.baseline) if args.baseline else None
checks = []
with tempfile.TemporaryDirectory(prefix='graphics-art-') as directory:
    directory = Path(directory)
    for size in [(160, 160), (640, 320), (4096, 4096)]:
        image = Image.new('RGB', size, (40, 160, 90))
        image.paste((210, 40, 100), (0, 0, size[0] // 2, size[1]))
        for progressive in (False, True):
            path = directory / f'{size[0]}-{size[1]}-{progressive}.jpg'
            image.save(path, quality=90, progressive=progressive)
            result, output = pixels(decode, path)
            assert result == 0, (path, result)
            for x, rgb in [(20, (210, 40, 100)), (140, (40, 160, 90))]:
                index = (80 * 160 + x) * 4
                sample = output[index:index + 4]
                assert max(abs(sample[i] - rgb[i]) for i in range(3)) <= 8, (path, sample)
                assert sample[3] == 255
            if size == (160, 160) and baseline:
                assert (result, output) == pixels(baseline, path), 'small artwork changed'
            checks.append(path.name)

    for extension in ['png', 'webp']:
        path = directory / f'ordinary.{extension}'
        Image.new('RGB', (320, 240), (40, 160, 90)).save(path)
        result, output = pixels(decode, path)
        assert result == 0
        if baseline:
            assert (result, output) == pixels(baseline, path), path
        checks.append(path.name)

    path = directory / 'too-large.png'
    Image.new('RGB', (5000, 5000), (40, 160, 90)).save(path)
    assert pixels(decode, path)[0] < 0
    checks.append('25MP PNG rejected')
    path = directory / 'broken.jpg'
    path.write_bytes(b'\xff\xd8\xff\xc0\x00\x11')
    assert pixels(decode, path)[0] < 0
    checks.append('truncated JPEG rejected')

print(json.dumps({'passed': True, 'checks': checks,
                  'qualification': 'host FFmpeg membrane; no GPU or battery claim'}))
