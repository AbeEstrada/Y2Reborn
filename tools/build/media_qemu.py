#!/usr/bin/env python3
"""Run freshly built Reborn media/control checks in a device-isolated QEMU environment.

Caller provides qemu-arm and a pinned ARM rootfs. Scratch must be new.
This is userspace/ABI evidence, never physical sound or SD bandwidth evidence.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time
import tempfile

ROOT=Path(__file__).resolve().parents[2]


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--sysroot',type=Path,required=True)
    parser.add_argument('--binaries',type=Path,required=True)
    parser.add_argument('--scratch',type=Path,required=True)
    args=parser.parse_args()
    scratch=args.scratch.resolve();scratch.mkdir()
    binaries=args.binaries.resolve()
    qemu=['qemu-arm','-cpu','cortex-a7','-L',str(args.sysroot)]
    socket_dir=tempfile.TemporaryDirectory(prefix='reborn-media-qemu-')
    socket=Path(socket_dir.name)/'control.sock'
    env=os.environ.copy()
    membranes=list((binaries/'build').glob('reborn-media-*/out/libreborn_media.so'))
    env['REBORN_MEDIA_LIB']=str(max(membranes,key=lambda p:p.stat().st_mtime_ns))
    command=qemu+[str(binaries/'reborn'),'--headless','--data-dir',str(scratch/'data'),'--socket',str(socket),'--music-dir',str(ROOT/'assets/fixtures'),'--fixtures',str(ROOT/'assets/fixtures')]
    with (scratch/'daemon.stdout').open('w') as stdout,(scratch/'daemon.stderr').open('w') as stderr:
        daemon=subprocess.Popen(command,stdout=stdout,stderr=stderr,env=env)
        try:
            for _ in range(200):
                if socket.exists():break
                if daemon.poll() is not None:raise RuntimeError('daemon failed; inspect scratch stderr')
                time.sleep(.1)
            results=[]
            for command in [['status'],['test','decoder'],['test','database'],['test','library'],['metrics'],['status']]:
                result=subprocess.run(qemu+[str(binaries/'rebornctl'),*command,'--socket',str(socket),'--json'],capture_output=True,text=True,check=True,timeout=90,env=env)
                payload=json.loads(result.stdout)
                if command[0]=='test':assert payload.get('passed') is True,(command,payload)
                results.append({'command':command,'result':payload})
            runtime=results[-1]['result']['decoder']['runtime']
            assert runtime['libraries']=={'libavutil':'61.1.102','libavcodec':'63.1.102','libavformat':'63.1.102','libavfilter':'12.1.102','libswresample':'7.1.102','libswscale':'10.1.102'},runtime
            assert '9.0.2' in runtime['version']
            assert set(runtime['protocols'])=={'file'} and not runtime['encoders'] and not runtime['muxers']
            for option in ['--disable-network','--disable-avdevice','--disable-programs']:assert option in runtime['configuration']
            required={'demuxers':{'flac','mp3','mov','ogg','wav','aac','aiff','ape','wv'},'audio_decoders':{'flac','mp3','mp3float','aac','alac','vorbis','opus','ape','wavpack','pcm_s16le','pcm_s16be','pcm_s24le','pcm_s24be','pcm_s32le','pcm_s32be','pcm_f32le','pcm_f32be'},'artwork_decoders':{'mjpeg','png','webp'},'parsers':{'flac','mpegaudio','aac','opus','vorbis'},'filters':{'abuffer','abuffersink','aformat','aresample','volume','equalizer','lowshelf','highshelf','alimiter','acrossfade','amix','atrim','afade','asetnsamples','atempo'}}
            for field,names in required.items():assert names<=set(runtime[field]),field
            for name in ['ffmpeg','ffplay','ffprobe']:assert not (args.sysroot/'usr/bin'/name).exists()
            assert not list((args.sysroot/'usr/lib').glob('libavdevice.so*'))
            elf=subprocess.check_output(['readelf','-d',env['REBORN_MEDIA_LIB']],text=True)
            for name in ['avformat','avcodec','avutil','avfilter','swresample','swscale']:assert f'lib{name}.so' in elf
            report={'passed':True,'physical_validation':False,'membrane':env['REBORN_MEDIA_LIB'],'results':results,'membrane_dynamic_dependencies':elf}
            (scratch/'results.json').write_text(json.dumps(report,indent=2)+'\n')
            print(json.dumps({'passed':True,'checks':len(results),'native_policy':'direct FFmpeg 9.0.2; local file only; no CLI, network, encoders or muxers'}))
        finally:
            daemon.terminate()
            try:daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:daemon.kill();daemon.wait();raise
            socket_dir.cleanup()


if __name__=='__main__':main()
