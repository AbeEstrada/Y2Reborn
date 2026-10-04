#!/usr/bin/env python3
"""Summarize product frames and evdev-to-KMS traces; retain coalesced input counts."""
import argparse
import collections
import hashlib
import json
import math
from pathlib import Path


def stats(values):
    values = sorted(values)
    def percentile(p):
        return values[math.ceil(len(values)*p)-1] if values else None
    return {'samples':len(values), 'p50_ms':percentile(.5), 'p95_ms':percentile(.95),
            'p99_ms':percentile(.99), 'max_ms':percentile(1)}


def summarize(directory):
    receipt = json.loads((directory/'receipt.json').read_text())
    trace = json.loads((directory/'trace.json').read_text())
    library_path = directory/'library-10000.json'
    library = json.loads(library_path.read_text()) if library_path.exists() else {'frames':[],'ui':[],'database':{'measurements':[]}}
    samples = trace['samples']
    injected = receipt['injected']
    groups = collections.defaultdict(lambda:collections.defaultdict(list))
    actions = collections.Counter()
    wheel_invariants = {'injected_steps':sum(v['label']=='wheel_1000' for v in injected),
                        'routed_steps':0, 'selection_mismatches':0, 'non_single_steps':0}
    frames = [s for s in samples if s['kind']=='frame']
    for i, event in enumerate(samples):
        if event['kind'] != 'input': continue
        data = event['data']
        actions[str(data['action'])] += 1
        # Only true kernel timestamps in this monotonic domain are latency
        # evidence. Timer-generated repeats carry no evdev timestamp.
        stamp = data.get('kernel_mono_us', data.get('evdev_us'))
        if stamp is None: continue
        at = stamp/1e6
        if not 0 <= event['monotonic_s']-at <= 5: continue
        # Injected key records can carry a kernel stamp a few milliseconds
        # before write(), so a preceding-only join mislabels adjacent controls.
        # Match the nearest injected record, then check the actual routed action.
        origin = min(injected, key=lambda v:abs(v['monotonic_s']-at))
        if abs(origin['monotonic_s']-at) > .075: continue
        label = origin['label']
        action = data['action']
        expected = ('select' if label.startswith(('select_', 'open_')) else
                    'back' if label.startswith('back_') else
                    'play_pause' if label.startswith('play_pause_') else label)
        if label.startswith('wheel'):
            if not isinstance(action, dict) or not any(k.startswith('wheel_') for k in action): continue
        elif action != expected: continue
        state = next((v for v in samples[i+1:] if v['kind']=='state'), None)
        if state is None or state['data']['action'] != data['action']: continue
        if label == 'wheel_1000':
            direction = 1 if 'wheel_clockwise' in action else -1
            steps = next(iter(action.values()))
            wheel_invariants['routed_steps'] += 1
            wheel_invariants['non_single_steps'] += steps != 1
            wheel_invariants['selection_mismatches'] += state['data']['focus']-data['focus'] != direction*steps
        groups[label]['input_to_handler_complete_ms'].append((state['monotonic_s']-at)*1000)
        groups[label]['handler_ms'].append(state['data']['action_ms'])
        groups[label]['evdev_to_dispatch_ms'].append((event['monotonic_s']-at)*1000)
        frame = next((v for v in samples[i+1:] if v['kind']=='frame'),None)
        next_state = next((v for v in samples[samples.index(state)+1:] if v['kind']=='state'),None)
        if frame and (not next_state or frame['monotonic_s'] < next_state['monotonic_s']):
            if all(frame['data'][key] == state['data'][key] for key in ('screen','focus')):
                groups[label]['input_to_KMS_completion_ms'].append((frame['monotonic_s']-at)*1000)
                groups[label]['state_to_KMS_completion_ms'].append((frame['monotonic_s']-state['monotonic_s'])*1000)
        else:
            groups[label]['coalesced'].append(1)
    startup = collections.defaultdict(list)
    for run in receipt.get('startup_runs', []):
        startup_trace = json.loads((directory/run['trace']).read_text())['samples']
        first = next((v for v in startup_trace if v['kind']=='frame'),None)
        if first: startup['exec_to_first_KMS_completion'].append((first['monotonic_s']-run['exec_started_s'])*1000)
        for event in run['events']:
            if 'elapsed_ms' in event['fields']:
                startup[event['fields'].get('phase',event['event'])].append(event['fields']['elapsed_ms'])
    idle = []
    for row in receipt['idle']:
        idle.append({**row,'process_cpu_percent':sum(v['cpu_percent'] for v in row['threads']),
                     'process_switches_per_s':sum(v['switches_per_s'] for v in row['threads'])})
    return {'binary_sha256':receipt['binary_sha256'],'benchmark_sha256':receipt['benchmark_sha256'],
            'boot':receipt['boot'],'versions':receipt['versions'],'same_boot':receipt['same_boot'],'taint':receipt['taint'],
            'restored_build':receipt['restored_build'],'forced_kill':receipt.get('forced_kill',False),
            'trace_dropped':trace['dropped'],'input_origin':receipt['input_origin'],
            'startup':[{ 'event':v['event'], **v['fields']} for v in receipt['startup']],
            'startup_statistics':{k:stats(v) for k,v in startup.items()},
            'latency':{k:{n:len(v) if n=='coalesced' else stats(v) for n,v in d.items()} for k,d in groups.items()},
            'actions':dict(actions),'idle':idle,'frames':library['frames'],'ui':library['ui'],
            'wheel_invariants':wheel_invariants,
            'database':library['database']['measurements'],'artwork':library.get('artwork',[]),
            'allocations':library.get('allocations'),
            'large_collections':library.get('large_collections'),
            'metrics':receipt['metrics'],'playing_status':receipt.get('playing_status'),
            'audio_during_playback':receipt.get('audio_during_playback'),
            'metrics_before_audio_profile':receipt.get('metrics_before_audio_profile'),
            'audio_controls':receipt.get('audio_controls'),
            'audio_control_metrics':receipt.get('audio_control_metrics'),
            'active_shutdown_ms':receipt.get('active_shutdown_ms'),
            'background_observation':stats([s['data']['elapsed_ms'] for s in samples if s['kind']=='background_observation']),
            'live_frames':{k:stats([s['data'][k] for s in frames]) for k in ('layout_ms','frame_ms')},
            'screens':dict(collections.Counter(v['data']['screen'] for v in frames))}


def pixels(before, after):
    results=[]
    for file in sorted((before/'pixels').glob('*.rgba')):
        a=file.read_bytes();b=(after/'pixels'/file.name).read_bytes()
        if len(a)!=len(b):raise ValueError('capture dimensions differ')
        differences=[abs(x-y) for x,y in zip(a,b)]
        results.append({'screen':file.stem,'bytes':len(a),'before_sha256':hashlib.sha256(a).hexdigest(),
                        'after_sha256':hashlib.sha256(b).hexdigest(),'different_channels':sum(v!=0 for v in differences),
                        'max_channel_difference':max(differences),'channels_above_one':sum(v>1 for v in differences)})
    return results


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before',required=True,type=Path)
    parser.add_argument('--after',required=True,type=Path)
    parser.add_argument('--output',required=True,type=Path)
    args=parser.parse_args()
    result={'schema':1,'before':summarize(args.before),'after':summarize(args.after),'pixel_comparison':pixels(args.before,args.after),
            'limits':['Synthetic evdev timing excludes mechanical, wheel IRQ and GPIO debounce latency.',
                      'KMS completion is not a photodiode measurement.',
                      'Application restart is not a new cold-boot qualification.',
                      'Context switches include I/O waits and scheduling; they are not exact timer wake counts.',
                      'No electrical power or human audible-response measurement.']}
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps({'output':str(args.output),'pixels':result['pixel_comparison']}))


if __name__=='__main__':main()
