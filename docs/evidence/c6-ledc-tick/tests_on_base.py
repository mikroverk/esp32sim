"""Write base production code + candidate tests into the mutation worktree's ledc.rs (argv: worktree, candidate ledc.rs)."""
import sys, subprocess
from pathlib import Path
tree, cand = Path(sys.argv[1]), Path(sys.argv[2]).read_text()
base = subprocess.run(['git', 'show', '104d7bd1:esp-periph/src/ledc.rs'], cwd=tree, capture_output=True, text=True, check=True).stdout
k = '#[cfg(test)]\nmod tests'
assert base.count(k) == 1 and cand.count(k) == 1
(tree / 'esp-periph/src/ledc.rs').write_text(base.split(k)[0] + k + cand.split(k)[1])
print('ok')
