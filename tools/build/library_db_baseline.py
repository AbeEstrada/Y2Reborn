#!/usr/bin/env python3
"""Build the current fixture harness against an archived pre-pass library.

Only benchmark adapters change: OFFSET catalog paging and the legacy seen prune.
The archived production schema, scanner and worker are untouched.
"""
import argparse
from pathlib import Path
import shutil
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--revision', default='b71b468860233faa0a42b8448ec5777fa952b8e3')
args = parser.parse_args()
repo = Path(__file__).resolve().parents[2]
output = args.output.resolve()
if not output.is_relative_to(repo / 'out'):
    raise SystemExit('baseline archive must live under this worktree/out')
output.mkdir(parents=True, exist_ok=False)
archive = subprocess.Popen(['git', 'archive', args.revision], cwd=repo, stdout=subprocess.PIPE)
try:
    subprocess.run(['tar', '-x', '-C', str(output)], stdin=archive.stdout, check=True)
finally:
    archive.stdout.close()
assert archive.wait() == 0
for name in ['crates/reborn-library/src/benchmark.rs', 'app/reborn/src/bin/reborn-bench.rs']:
    shutil.copy2(repo / name, output / name)
path = output / 'crates/reborn-library/src/benchmark.rs'
source = path.read_text()
assert source.count('catalog(&mut c)') == 2
source = source.replace('catalog(&mut c)', 'baseline_catalog(&c, count)')
assert source.count('finish(c, source, true).map(|_| ())') == 1
source = source.replace('finish(c, source, true).map(|_| ())',
                        'c.execute("UPDATE tracks SET deleted=1 WHERE source_id=?1 AND seen<>2", [&source.id]).map(|_|()).map_err(err)')
source += '''
fn baseline_catalog(c: &Connection, count: usize) -> Result<Vec<Track>, String> {
    let mut result = Vec::new();
    for offset in (0..count).step_by(512) {
        result.extend(list(c,&Filter{offset,limit:512,..Default::default()})?);
    }
    Ok(result)
}
'''
# Keep helpers before the test module for Clippy on the archived workspace too.
start = source.index('#[cfg(test)]\nmod tests')
end = source.index('\nfn baseline_catalog', start)
tests = source[start:end]
source = source[:start] + source[end:] + '\n' + tests
path.write_text(source)
subprocess.run(['cargo', 'fmt', '--all', '--manifest-path', str(output / 'Cargo.toml')], cwd=repo, check=True)
print(output)
