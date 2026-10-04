#!/usr/bin/env python3
"""Run an isolated library benchmark. Never opens a user's existing database."""
import argparse
import hashlib
import platform
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import subprocess
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--cpu', type=int)
parser.add_argument('--sizes', type=int, nargs='+', default=[100, 1000, 10000, 50000])
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
context = {'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(),
           'utc': datetime.now(timezone.utc).isoformat(), 'platform': platform.platform(),
           'cpu_affinity': args.cpu, 'load_before': os.getloadavg()}
(args.output / 'context.json').write_text(json.dumps(context, indent=2) + '\n')
for count in args.sizes:
    # Real host filesystem, not tmpfs. The child inherits only this empty FD.
    with tempfile.TemporaryDirectory(prefix='fixture-', dir=args.output) as scratch:
        fd = os.open(scratch, os.O_RDONLY | os.O_DIRECTORY)
        try:
            run = subprocess.run([str(args.binary.resolve()), '--scratch-fd', str(fd),
                                  '--tracks', str(count), '--platform-db'],
                                 pass_fds=(fd,), text=True, capture_output=True, timeout=1800,
                                 preexec_fn=(lambda: os.sched_setaffinity(0, {args.cpu})) if args.cpu is not None else None)
        finally:
            os.close(fd)
        if run.returncode:
            raise RuntimeError(run.stdout + run.stderr)
        result = json.loads(run.stdout)
        (args.output / f'{count}.json').write_text(json.dumps(result, indent=2) + '\n')
        print(f'{count}: complete', flush=True)
