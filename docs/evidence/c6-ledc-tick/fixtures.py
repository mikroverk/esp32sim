"""S3/C3/C6/P4 fixture command lines shared by the EX252 scripts. ESP32SIM_TREE: the checkout whose
web/wasm/fw holds the fetched assets (default: current directory); the mask ROM comes from there or,
when it is not there, from ESP32SIM_ROM_DIR; C6_STARTER: a directory with the public display
starter's four images and manifest.json (offsets and SHA-256, inputs.json); FIXTURES:
space-separated fixture names (default: the four C6 ones), any web/wasm/fw/<name>.json (`hello` is
the S3 hello; pocket-tank needs tools/fetch-pocket-tank.sh). binary(b, name): b itself, or, when b
is a `cargo build --release --bins` directory, the binary for the fixture's chip in it."""
from pathlib import Path
import json, hashlib, os
TREE = Path(os.environ.get('ESP32SIM_TREE', '.')).resolve()
FW = TREE / 'web/wasm/fw'
EXE = {'esp32s3': 'esp32sim', 'esp32c3': 'esp32sim-c3', 'esp32c6': 'esp32sim-c6', 'esp32p4': 'esp32sim-p4'}
STARTER = Path(os.environ.get('C6_STARTER', 'starter'))

def fixture_args(name):
    if name == 'public-display-starter':
        a = ['--boot', 'rom', '--rom', str(FW / 'esp32c6_rev0_rom.elf'), '--no-dump', '--flash-mb', '8', '--board', 'waveshare-c6-lcd147']
        man = json.loads((STARTER / 'manifest.json').read_text())
        for f in man['artifacts']:
            p = STARTER / f['filename']
            assert hashlib.sha256(p.read_bytes()).hexdigest() == f['sha256']
            a += ['--flash-at', f"{f['offset']:x}={p}"]
        return a
    j = json.loads((FW / f'{name}.json').read_text())
    board = j['board']
    a = ['--boot', 'rom', '--rom', str(rom(j['files'].get('rom', 'esp32c6_rev0_rom.elf'))), '--no-dump']
    a += ['--flash-mb', str(j['flash_mb']), '--board', 'none' if board in ('esp32c6', 'esp32c3', 'esp32p4') else board]
    if j.get('psram_mb'): a += ['--psram-mb', str(j['psram_mb'])]
    for k in ('bootloader', 'ptable', 'app'):
        a += [f'--{k}', str(FW / j['files'][k])]
    for off, f in j.get('flash_at', {}).items(): a += ['--flash-at', f'{off}={FW / f}']
    return a

def rom(f):
    p = FW / f
    return p if p.exists() or not os.environ.get('ESP32SIM_ROM_DIR') else Path(os.environ['ESP32SIM_ROM_DIR']) / f

def binary(b, name):
    b = Path(b)
    if not b.is_dir(): return b
    if name == 'public-display-starter': return b / 'esp32sim-c6'
    r = json.loads((FW / f'{name}.json').read_text())['files'].get('rom', 'esp32c6_rev0_rom.elf')
    return b / EXE[r.split('_')[0]]

FIXTURES = os.environ.get('FIXTURES', 'c6-hello c6-contiki c6-energy-scan public-display-starter').split()
