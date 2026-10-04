#!/usr/bin/env python3
"""Write adversarial tag/image fixtures; production still delegates parsing to FFmpeg.

No FFmpeg subprocess is used. Writers use checked-in CC0 media as the payload.
"""
import base64
import hashlib
import json
from pathlib import Path
import struct
import zlib

ROOT = Path(__file__).resolve().parents[2]
BASE = ROOT / 'assets/fixtures'
OUT = BASE / 'metadata'


def block(kind, payload, last=False):
    return bytes([kind | (128 if last else 0)]) + len(payload).to_bytes(3, 'big') + payload


def comments(values):
    vendor = b'Reborn adversarial fixture writer'
    entries = [v.encode('utf-8') if isinstance(v, str) else v for v in values]
    return struct.pack('<I', len(vendor)) + vendor + struct.pack('<I', len(entries)) + b''.join(struct.pack('<I', len(v)) + v for v in entries)


def flac(name, values=(), picture=None):
    # Keep STREAMINFO and the original encoded audio, replace only fixture metadata.
    original = (BASE / 'flac-24-96000.flac').read_bytes()
    offset = 4
    while True:
        header = original[offset:offset + 4]
        size = int.from_bytes(header[1:], 'big')
        offset += 4 + size
        if header[0] & 128:
            break
    streaminfo = original[8:42]
    blocks = [block(0, streaminfo), block(4, comments(values), picture is None)]
    if picture is not None:
        blocks.append(block(6, picture, True))
    (OUT / name).write_bytes(b'fLaC' + b''.join(blocks) + original[offset:])


def picture(data, mime=b'image/png', width=8, height=8, declared=None):
    return struct.pack('>II', 3, len(mime)) + mime + struct.pack('>I', 0) + struct.pack('>IIIII', width, height, 24, 0, len(data) if declared is None else declared) + data


def id3(name, frames):
    def syncsafe(value):
        return bytes([(value >> shift) & 127 for shift in [21, 14, 7, 0]])
    body = b''.join(key.encode('ascii') + syncsafe(len(value)) + b'\0\0' + value for key, value in frames)
    original = (BASE / 'mp3.mp3').read_bytes()
    length = sum((original[6 + i] & 127) << (21 - 7 * i) for i in range(4)) + 10
    (OUT / name).write_bytes(b'ID3\4\0\0' + syncsafe(len(body)) + body + original[length:])


def ape(name, values):
    data = b''.join(struct.pack('<II', len(v.encode()), 0) + k.encode() + b'\0' + v.encode() for k, v in values)
    footer = b'APETAGEX' + struct.pack('<IIII', 2000, len(data) + 32, len(values), 0) + b'\0' * 8
    (OUT / name).write_bytes((BASE / 'ape-silence.ape').read_bytes() + data + footer)


def png(width, height):
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data) & 0xffffffff)
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(b'\0' + b'\0' * 24)) + chunk(b'IEND', b'')


def wav_tags():
    def info(key, value):
        data = value.encode('utf-8') + b'\0'
        return key + struct.pack('<I', len(data)) + data + (b'\0' if len(data) % 2 else b'')
    metadata = b'INFO' + info(b'INAM', '日本語 – café') + info(b'IART', 'Björk') + info(b'IPRD', 'Album') + info(b'ICRD', '2024-02-29') + info(b'IGNR', 'Ambient') + info(b'ITRK', '3/12')
    original = (BASE / 'wav-pcm24.wav').read_bytes()
    body = b'WAVE' + b'LIST' + struct.pack('<I', len(metadata)) + metadata + original[12:]
    (OUT / 'tags.wav').write_bytes(b'RIFF' + struct.pack('<I', len(body)) + body)


def aiff_tags():
    def text(key, value):
        data = value.encode('utf-8')
        return key + struct.pack('>I', len(data)) + data + (b'\0' if len(data) % 2 else b'')
    original = (BASE / 'aiff-pcm24.aiff').read_bytes()
    body = original[8:12] + text(b'NAME', '日本語 – café') + text(b'AUTH', 'Björk') + original[12:]
    (OUT / 'tags.aiff').write_bytes(b'FORM' + struct.pack('>I', len(body)) + body)


def main():
    OUT.mkdir(exist_ok=True)
    wav_tags()
    aiff_tags()
    tags = ['TITLE=日本語 – café', 'ARTIST=Björk', 'ALBUM=Album', 'ALBUMARTIST=Various', 'TRACKNUMBER=3/12', 'DISCNUMBER=1/2', 'DATE=2024-02-29', 'GENRE=Ambient', 'REPLAYGAIN_TRACK_GAIN=-4.25 dB', 'REPLAYGAIN_TRACK_PEAK=0.95', 'REPLAYGAIN_ALBUM_GAIN=0.0 dB', 'REPLAYGAIN_ALBUM_PEAK=0.85']
    flac('tags.flac', tags)
    flac('long.flac', ['TITLE=' + 'é' * 10000, 'ARTIST=', 'ALBUM=', 'TRACK=-1', 'DISC=4294967296', 'REPLAYGAIN_TRACK_GAIN=nan', 'REPLAYGAIN_ALBUM_GAIN=1000 dB', 'REPLAYGAIN_TRACK_PEAK=0.9 dB'])
    flac('duplicate.flac', ['TITLE=first', 'TITLE=second', 'ARTIST=one', 'ARTIST=two', 'TRACK=3garbage', 'DISC=1/2garbage'])
    flac('invalid-utf8.flac', [b'TITLE=bad\xffencoding', 'TRACK= 3 / 12 '])
    for ext in ['jpg', 'png', 'webp']:
        data = (BASE / f'artwork-external.{ext}').read_bytes()
        flac(f'art-{ext}.flac', tags, picture(data, b'image/jpeg' if ext == 'jpg' else f'image/{ext}'.encode()))
    flac('corrupt-art.flac', tags, picture(b'not a png'))
    flac('huge-art-metadata.flac', tags, picture(b'x', declared=0xffffffff))
    oversized = png(100000, 100000)
    (OUT / 'huge.png').write_bytes(oversized)
    flac('huge-art-dimensions.flac', tags, picture(oversized, width=100000, height=100000))
    id3('id3.mp3', [('TIT2', b'\3' + '日本語 – café'.encode()), ('TPE1', b'\1' + 'Björk'.encode('utf-16')), ('TALB', b'\3Album'), ('TPE2', b'\3Various'), ('TRCK', b'\3' + b'3/12'), ('TPOS', b'\3' + b'1/2'), ('TDRC', b'\3' + b'2024-02-29'), ('TCON', b'\3Ambient'), ('TXXX', b'\3replaygain_track_gain\0-4.25 dB')])
    ape('tags.ape', [('Title', '日本語 – café'), ('Artist', 'Björk'), ('Album', 'Album'), ('AlbumArtist', 'Various'), ('Track', '3/12'), ('Disc', '1/2'), ('Year', '2024'), ('Genre', 'Ambient'), ('replaygain_track_gain', '-4.25 dB')])
    (OUT / 'empty.flac').write_bytes(b'')
    (OUT / 'manifest.json').write_text(json.dumps({'license':'CC0-1.0 fixture metadata and generated images; original media provenance in ../manifest.json', 'large_jpeg_origin':'4000x4000 YUV encoded by direct host FFmpeg libraries; regenerate with tools/build/jpeg_fixture.c', 'files':{p.name:{'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()} for p in sorted(OUT.iterdir()) if p.name != 'manifest.json'}}, indent=2) + '\n')


if __name__ == '__main__':
    main()
