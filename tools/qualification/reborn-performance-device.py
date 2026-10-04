#!/usr/bin/env python3
"""Reversible Y2 performance fixture; caller MUST hold host device flock.

Run via owner-pinned SSH with temporary ARM binaries. Stops/restores S90reborn,
uses a private bind-mounted copy of application data, injects supported evdev
records (not human/IRQ timing), and preserves the installed executable and data.
No flashing, CPU policy, radios, partitions or kernel settings are changed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import struct
import subprocess
import tempfile
import time
import wave


def call(op, **fields):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(5)
        client.connect('/run/reborn/control.sock')
        client.sendall((json.dumps({'version': 1, 'id': 1, 'command': {'op': op, **fields}})+'\n').encode())
        data = b''
        while chunk := client.recv(65536):
            data += chunk
    reply = json.loads(data)
    if not reply['ok']:
        raise RuntimeError(reply['error'])
    return reply['result']


def wait_ready(process=None):
    end = time.monotonic()+15
    while time.monotonic() < end:
        if process and process.poll() is not None:
            raise RuntimeError('fixture process exited')
        try:
            status = call('status')
            if status['library']['tracks_loaded'] and not status['library']['scanning']:
                time.sleep(1)
                return status
        except (OSError, ValueError):
            pass
        time.sleep(.1)
    raise RuntimeError('application readiness timeout')


def counters(pid):
    tasks = {}
    for p in Path(f'/proc/{pid}/task').iterdir():
        try:
            stat = (p/'stat').read_text().split(') ')[1].split()
            tasks[p.name] = {'name': (p/'comm').read_text().strip(),
                'ticks': int(stat[11])+int(stat[12]),
                'user_ticks':int(stat[11]), 'system_ticks':int(stat[12]),
                'switches': sum(int(x.split(':')[1]) for x in (p/'status').read_text().splitlines() if 'ctxt_switches:' in x)}
        except FileNotFoundError:
            pass
    return {'at': time.monotonic(), 'tasks': tasks, 'frames': call('metrics')['graphics_frames'],
        'idle': {str(p): int(p.read_text()) for p in Path('/sys/devices/system/cpu').glob('cpu*/cpuidle/state*/usage')}}


def idle(pid, state, seconds=10):
    time.sleep(2)
    a = counters(pid)
    time.sleep(seconds)
    b = counters(pid)
    dt = b['at']-a['at']
    hz = os.sysconf('SC_CLK_TCK')
    return {'state': state, 'duration_s': dt, 'frames': b['frames']-a['frames'],
        'threads': [{'name': v['name'], 'cpu_percent': (v['ticks']-a['tasks'][k]['ticks'])/hz/dt*100,
            'user_cpu_percent':(v['user_ticks']-a['tasks'][k]['user_ticks'])/hz/dt*100,
            'system_cpu_percent':(v['system_ticks']-a['tasks'][k]['system_ticks'])/hz/dt*100,
            'switches_per_s': (v['switches']-a['tasks'][k]['switches'])/dt}
            for k, v in b['tasks'].items() if k in a['tasks']],
        'idle_entries': {k: b['idle'][k]-v for k, v in a['idle'].items()}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--benchmark', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--profile-audio', action='store_true', help='Untimed one-second syscall sample after playback counters')
    args = parser.parse_args()
    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    original = call('status')
    if original['playback']['state'] not in ('paused', 'stopped'):
        raise RuntimeError('active owner playback; cannot isolate fixture')
    boot = Path('/proc/sys/kernel/random/boot_id').read_text().strip()
    receipt = {'boot': boot, 'versions': json.loads(Path('/etc/y2linux/versions.json').read_text()),
        'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(),
        'benchmark_sha256': hashlib.sha256(args.benchmark.read_bytes()).hexdigest(),
        'input_origin': 'synthetic records injected into real supported evdev nodes', 'idle': [], 'injected': []}
    process = None
    bound = False
    stopped = False
    scratch = Path(tempfile.mkdtemp(prefix='reborn-performance-', dir='/data'))
    try:
        subprocess.run(['/etc/init.d/S90reborn', 'stop'], check=True)
        stopped = True
        end = time.monotonic()+10
        while Path('/run/reborn/process.pid').exists():
            if time.monotonic() > end:
                raise RuntimeError('production service did not stop')
            time.sleep(.1)
        # Only fresh scratch data is mutated by the SQLite/10k-track fixture.
        with tempfile.TemporaryDirectory(prefix='library-', dir=scratch) as directory:
            fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
            try:
                result = subprocess.run([str(args.benchmark), '--scratch-fd', str(fd), '--tracks', '10000', '--graphics', '--capture-dir', str(args.output/'pixels'), '--artwork-dir', '/usr/share/reborn/fixtures'],
                    pass_fds=(fd,), capture_output=True, timeout=240, check=True)
                (args.output/'library-10000.json').write_bytes(result.stdout)
            finally:
                os.close(fd)
        shutil.copytree('/data/reborn', scratch/'app')
        session_path = scratch/'app/state/session.json'
        session = json.loads(session_path.read_text())
        model = session.get('model', session)
        model['settings']['screen_timeout_seconds'] = 0
        # Extend the installed one-second tone in owned scratch, keeping the
        # playback window active throughout the timed controls and idle sample.
        tone = Path('/data/reborn/performance-tone.wav')
        physical_tone = scratch/'app/performance-tone.wav'
        with wave.open('/usr/share/reborn/fixtures/tone.wav', 'rb') as source:
            parameters = source.getparams()
            pcm = source.readframes(source.getnframes())
        with wave.open(str(physical_tone), 'wb') as destination:
            destination.setparams(parameters)
            for _ in range(40): destination.writeframesraw(pcm)
        track = {'id': 900001, 'source_id': 'internal', 'path': str(tone), 'filename':tone.name,
                 'title':'Performance tone', 'artist':'Reborn fixture', 'album':'Performance', 'album_artist':'Reborn fixture',
                 'duration_ms':40000, 'sample_rate':parameters.framerate, 'channels':parameters.nchannels, 'codec':'pcm_s16le', 'online':True,
                 'size':physical_tone.stat().st_size, 'mtime':int(physical_tone.stat().st_mtime),
                 'track':1, 'disc':1, 'bitrate':parameters.framerate*parameters.nchannels*parameters.sampwidth*8, 'artwork':False}
        model.update(queue=[track], queue_entry_ids=[1], queue_position=0, position_ms=0, playback='paused')
        session_path.write_text(json.dumps(session))
        subprocess.run(['mount', '--bind', str(scratch/'app'), '/data/reborn'], check=True)
        bound = True
        with (args.output/'stderr.txt').open('wb') as stderr:
            receipt['exec_started_s'] = time.monotonic()
            process = subprocess.Popen([str(args.binary), '--performance-trace', str(args.output/'trace.json')], stderr=stderr)
        status = wait_ready(process)
        if status['playback']['queue_length'] != 1: raise RuntimeError('private fixture session was rejected')
        receipt['pid'] = process.pid
        receipt['idle'].append(idle(process.pid, 'home_static_paused'))
        paths = {p.joinpath('device/name').read_text().strip(): Path('/dev/input')/p.name
                 for p in Path('/sys/class/input').glob('event*')}
        input_files = {name: path.open('wb', buffering=0) for name, path in paths.items() if name in ('Y2 navigation buttons', 'APT32F click-wheel', 'mt6582-keypad', 'mtk-pmic-keys')}
        def record(device, code, value):
            at = time.monotonic()
            f = input_files[device]
            f.write(struct.pack('llHHi', 0, 0, 1, code, value))
            f.write(struct.pack('llHHi', 0, 0, 0, 0, 0))
            return at
        def key(code, label, hold=.025):
            record('Y2 navigation buttons', code, 1)
            time.sleep(hold)
            at = record('Y2 navigation buttons', code, 0)
            receipt['injected'].append({'label': label, 'monotonic_s': at})
            time.sleep(.18)
        def wheel(direction, label='wheel', delay=.18):
            code = 109 if direction > 0 else 104
            at = record('APT32F click-wheel', code, 1)
            record('APT32F click-wheel', code, 0)
            receipt['injected'].append({'label': label, 'monotonic_s': at})
            time.sleep(delay)
        key(28, 'select_library')
        if call('status')['current_screen'] != 'music':
            raise RuntimeError('evdev injection did not reach Music')
        # Each traversal includes actual Select, Back, collection selection and
        # scrolling. Read status outside timed sequences only.
        for _ in range(16):
            key(28, 'select_albums')
            key(28, 'open_album')
            wheel(1)
            key(158, 'back_album')
            key(158, 'back_albums')
            wheel(1)
            key(28, 'select_artists')
            key(28, 'open_artist')
            key(158, 'back_artist')
            key(158, 'back_artists')
            wheel(-1)
        wheel(1)
        wheel(1)
        key(28, 'select_tracks')
        for i in range(1000):
            wheel(1 if i % 2 == 0 else -1, 'wheel_1000', delay=.012)
        key(158, 'back_tracks')
        key(158, 'back_library')
        wheel(1)
        key(28, 'open_now_playing')
        receipt['idle'].append(idle(process.pid, 'now_playing_paused'))
        for _ in range(16):
            key(158, 'back_now_playing')
            key(28, 'open_now_playing')
        key(164, 'play_pause')
        active = call('status')['playback']
        if active['state'] not in ('playing','buffering'): raise RuntimeError('playback fixture failed: '+str(active))
        receipt['playing_status'] = active
        for code, value in [(115,1),(115,0),(114,1),(114,0)]*16:
            at = record('mt6582-keypad', code, value)
            if value: receipt['injected'].append({'label':'volume_up' if code==115 else 'volume_down','monotonic_s':at})
            time.sleep(.18)
        receipt['idle'].append(idle(process.pid, 'playing', seconds=5))
        if call('status')['playback']['state'] != 'playing': raise RuntimeError('playback ended before idle sample completed')
        receipt['audio_during_playback'] = call('audio')
        if args.profile_audio:
            receipt['metrics_before_audio_profile'] = call('metrics')
            tid = next(p.name for p in Path(f'/proc/{process.pid}/task').iterdir() if (p/'comm').read_text().strip() == 'audio')
            with (args.output/'audio-profile.txt').open('wb') as output:
                observer = subprocess.Popen(['strace','-c','-p',tid],stdout=output,stderr=output)
                time.sleep(1)
                observer.send_signal(signal.SIGINT)
                observer.wait(timeout=5)
            receipt['audio_profile_untimed_seconds'] = 1
        key(164, 'play_pause')
        key(158, 'back_now_playing')
        wheel(1)
        key(28, 'open_queue')
        wheel(1)
        wheel(-1)
        key(158, 'back_queue')
        wheel(1)
        key(28, 'open_settings')
        receipt['idle'].append(idle(process.pid, 'settings_menu_idle'))
        # Screen-off uses the real power input; wake is restored before cleanup.
        record('mtk-pmic-keys', 116, 1); time.sleep(.025)
        at=record('mtk-pmic-keys', 116, 0)
        receipt['injected'].append({'label':'screen_sleep','monotonic_s':at})
        receipt['idle'].append(idle(process.pid, 'screen_off_paused'))
        record('mtk-pmic-keys', 116, 1); time.sleep(.025)
        at=record('mtk-pmic-keys', 116, 0)
        receipt['injected'].append({'label':'screen_wake','monotonic_s':at})
        time.sleep(.3)
        for file in input_files.values(): file.close()
        receipt['startup'] = call('logs', last=30, subsystem='startup', level=None, since_ms=None)
        receipt['metrics'] = call('metrics')
        receipt['final_playback'] = call('status')['playback']
        process.terminate(); process.wait(timeout=10); process = None
        receipt['startup_runs'] = [{'exec_started_s':receipt['exec_started_s'], 'events':receipt['startup'], 'trace':'trace.json'}]
        for n in range(1,4):
            trace_name = f'startup-{n}.json'
            started = time.monotonic()
            with (args.output/f'startup-{n}-stderr.txt').open('wb') as stderr:
                process = subprocess.Popen([str(args.binary), '--performance-trace', str(args.output/trace_name)], stderr=stderr)
            wait_ready(process)
            events = call('logs', last=30, subsystem='startup', level=None, since_ms=None)
            receipt['startup_runs'].append({'exec_started_s':started,'events':events,'trace':trace_name})
            process.terminate(); process.wait(timeout=10); process = None
    finally:
        if process:
            process.terminate()
            try: process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill(); process.wait()
                receipt['forced_kill'] = True
        if bound:
            subprocess.run(['umount', '/data/reborn'], check=True)
        if stopped:
            subprocess.run(['/etc/init.d/S90reborn','start'], check=True)
            restored = wait_ready()
            receipt['restored_build'] = restored['build_id']
            # Restore original screen-off preference without changing owner settings.
            if original['screen_off'] and not restored['screen_off']:
                power_path = next(Path('/dev/input')/p.name for p in Path('/sys/class/input').glob('event*')
                                  if p.joinpath('device/name').read_text().strip()=='mtk-pmic-keys')
                with power_path.open('wb', buffering=0) as f:
                    f.write(struct.pack('llHHi',0,0,1,116,1)+struct.pack('llHHi',0,0,0,0,0))
                    time.sleep(.025)
                    f.write(struct.pack('llHHi',0,0,1,116,0)+struct.pack('llHHi',0,0,0,0,0))
                time.sleep(.2)
        receipt['same_boot'] = boot == Path('/proc/sys/kernel/random/boot_id').read_text().strip()
        receipt['taint'] = Path('/proc/sys/kernel/tainted').read_text().strip()
        (args.output/'receipt.json').write_text(json.dumps(receipt, indent=2)+'\n')
        shutil.rmtree(scratch)
    print(json.dumps({'output':str(args.output),'same_boot':receipt['same_boot'],'taint':receipt['taint']}))


if __name__ == '__main__':
    main()
