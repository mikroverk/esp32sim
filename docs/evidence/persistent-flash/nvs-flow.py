#!/usr/bin/env python3
"""Two runs and one control for the persistent flash: configure a soft-AP guest through its own web UI,
then boot a second run from the flash file alone and check that the guest comes up with the settings.

  run-a    : the guest comes up in access-point mode with no saved configuration; its page's own
             protocol (POST /_kfg_ empty = read, POST /_kfg_ = save) stores SSID/PSK; the run is ended
             by SIGTERM, which must still write the flash file
  run-b    : the same flash file, no images, nothing but an access point this script asks the emulator
             to offer: the guest must join it, take a lease, and answer its page through --hostfwd
  control  : the same images and access point, a flash file that was never configured: the guest must
             stay in access-point mode

Paths come from the environment so no bench is baked in:

  APP=app.bin BOOTLOADER=bootloader.bin PTABLE=ptable.bin BOOT_APP0=boot_app0.bin \
  ROM=esp32s3_rev0_rom.elf SSID=bench-ssid PASS=bench-password \
  python3 nvs-flow.py run-a --join-bin <emulator with --wifi-join> --flash-file flash.bin
  python3 nvs-flow.py run-b --nvs-bin <the emulator under test> --flash-file flash.bin

The guest in the receipt runs at about 0.1x real time, so a wait is budgeted in minutes, not seconds.
"""

import argparse
import atexit
import gzip
import os
import pty
import signal
import subprocess
import threading
import time
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))


def env(name):
    value = os.environ.get(name)
    if not value:
        raise SystemExit(f"set {name} (see the usage in this file's docstring)")
    return value


def start(binary, extra, log_path, chip, rom):
    cmd = [binary, "--chip", chip, "--boot", "rom", "--rom", rom,
           "--flash-mb", os.environ.get("FLASH_MB", "16"), "--psram-mb", os.environ.get("PSRAM_MB", "8"),
           "--board", os.environ.get("BOARD", "none"), "--console", "both", "--no-dump"] + extra
    log = open(log_path, "w", encoding="utf-8", errors="replace")
    master, slave = pty.openpty()
    process = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=slave, stderr=slave, start_new_session=True)
    os.close(slave)

    def pump():
        buf = b""
        while True:
            try:
                chunk = os.read(master, 65536)
            except OSError:
                break
            if not chunk:
                break
            buf += chunk
            while b"\n" in buf:
                line, buf = buf.split(b"\n", 1)
                log.write(line.decode("utf-8", "replace").rstrip("\r") + "\n")
                log.flush()

    threading.Thread(target=pump, daemon=True).start()
    # A probe that gives up must not leave the guest running: it would hold the host port and a core.
    atexit.register(stop, process, log)
    print(" ".join(cmd), flush=True)
    return process, log


def stop(process, log):
    """SIGTERM: the run has to write its flash file before it leaves."""
    if process.poll() is None:
        try:
            process.send_signal(signal.SIGTERM)
        except ProcessLookupError:
            pass
    try:
        process.wait(timeout=600)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
    time.sleep(0.2)
    if not log.closed:
        log.close()
    return process.returncode


def http(port, path, body=None, timeout=300):
    data = body.encode() if body is not None else None
    request = urllib.request.Request(f"http://127.0.0.1:{port}{path}", data=data,
                                     headers={"Content-Type": "application/x-www-form-urlencoded"} if data else {})
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def wait_web(port, path="/config", limit=1500, step=3):
    started = time.time()
    while time.time() - started < limit:
        try:
            status, body = http(port, path, timeout=60)
            if status:
                print(f"{path} answered after {time.time() - started:.0f}s wall", flush=True)
                return status, body
        except OSError:
            pass
        time.sleep(step)
    raise SystemExit(f"127.0.0.1:{port}: the guest's web server never answered")


def decoded(body):
    return gzip.decompress(body).decode("utf-8", "replace") if body[:2] == b"\x1f\x8b" else body.decode("utf-8", "replace")


def pairs(text):
    """`NAME=value&NAME=value`, the format the guest's page reads and writes."""
    out = {}
    for pair in text.strip().split("&"):
        if pair and "=" in pair:
            name, value = pair.split("=", 1)
            out[name.strip()] = value
    return out


def body(values):
    return "&".join(f"{k}={v}" for k, v in values.items())


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["run-a", "run-b", "save", "control"])
    parser.add_argument("--join-bin", help="emulator that can join the guest's own access point (run-a)")
    parser.add_argument("--nvs-bin", help="emulator under test (run-b, control)")
    parser.add_argument("--flash-file", required=True, help="the persistent image")
    parser.add_argument("--port", type=int, default=18080)
    parser.add_argument("--max-seconds", type=int, default=60)
    parser.add_argument("--seconds", type=int, default=200, help="control: wall-clock seconds to run")
    args = parser.parse_args()

    chip = os.environ.get("CHIP", "s3")
    rom = env("ROM")
    ssid, password = os.environ.get("SSID", "bench-ssid"), os.environ.get("PASS", "bench-password")

    if args.mode == "run-a":
        process, log = start(args.join_bin or env("JOIN_BIN"), [
            "--bootloader", env("BOOTLOADER"), "--ptable", env("PTABLE"), "--app", env("APP"),
            "--flash-at", f"0xe000={env('BOOT_APP0')}", "--flash-file", args.flash_file,
            "--wifi-join", "", "--hostfwd", f"tcp:127.0.0.1:{args.port}-192.168.4.1:80",
            "--max-seconds", str(args.max_seconds)], "/tmp/nvs-run-a.log", chip, rom)
        status, page = wait_web(args.port)
        print(f"/config: {status}, {len(page)} B on the wire", flush=True)
        status, page = http(args.port, "/_kfg_", "")
        current = pairs(decoded(page))
        print(f"POST /_kfg_ (empty): {status}: {current}", flush=True)
        if "STANAME" not in current:
            raise SystemExit("the guest did not report STANAME")
        current["STANAME"], current["STAPASS"] = ssid, password
        status, page = http(args.port, "/_kfg_", body(current))
        print(f"POST /_kfg_ (save): {status}: {decoded(page)[:300]!r}", flush=True)
        status, page = http(args.port, "/_kfg_", "")
        assert pairs(decoded(page)).get("STANAME") == ssid, "the guest did not keep the saved name"
        print(f"read back: STANAME={ssid!r} STAPASS={password!r}", flush=True)
        try:
            http(args.port, "/config?restart=yes", timeout=20)
        except OSError as e:
            print(f"restart: {e}", flush=True)
        time.sleep(10)
        print("exit:", stop(process, log), flush=True)
        return

    common = ["--wifi", f"ssid={ssid},chan=6,psk={password}",
              "--hostfwd", f"tcp:127.0.0.1:{args.port}-:80", "--max-seconds", str(args.max_seconds)]
    if args.mode == "run-b":
        # no images: everything the guest has is what run A left in the flash file
        extra = ["--flash-file", args.flash_file] + common
    elif args.mode == "save":
        # The file alone again, but the guest's page writes a new name: its own save path has to
        # reach the file, so the next run can come up under the new name.
        extra = ["--flash-file", args.flash_file] + common
        process, log = start(args.nvs_bin or env("NVS_BIN"), extra, "/tmp/nvs-save.log", chip, rom)
        status, page = wait_web(args.port)
        print(f"station web UI: /config {status}, {len(page)} B on the wire", flush=True)
        status, page = http(args.port, "/_kfg_", "")
        current = pairs(decoded(page))
        print(f"POST /_kfg_ (before): {status}: STANAME={current.get('STANAME')!r}", flush=True)
        assert current.get("STANAME") == ssid, "the name run A saved is not the one the guest booted with"
        new_name = os.environ.get("NEW_SSID", "bench-two")
        current["STANAME"], current["STAPASS"] = new_name, password
        status, page = http(args.port, "/_kfg_", body(current))
        print(f"POST /_kfg_ (save {new_name!r}): {status}", flush=True)
        status, page = http(args.port, "/_kfg_", "")
        assert pairs(decoded(page)).get("STANAME") == new_name, "the guest did not keep the new name"
        print(f"read back: STANAME={new_name!r}", flush=True)
        print("exit:", stop(process, log), flush=True)
        return
    else:
        extra = ["--bootloader", env("BOOTLOADER"), "--ptable", env("PTABLE"), "--app", env("APP"),
                 "--flash-at", f"0xe000={env('BOOT_APP0')}", "--flash-file", args.flash_file] + common
    process, log = start(args.nvs_bin or env("NVS_BIN"), extra,
                         f"/tmp/nvs-{args.mode}.log", chip, rom)
    if args.mode == "control":
        time.sleep(args.seconds)
        print("exit:", stop(process, log), flush=True)
        return
    status, page = wait_web(args.port)
    print(f"station web UI: /config {status}, {len(page)} B on the wire", flush=True)
    status, page = http(args.port, "/_kfg_", "")
    settings = pairs(decoded(page))
    print(f"POST /_kfg_ (empty): {status}: STANAME={settings.get('STANAME')!r} "
          f"STAPASS={settings.get('STAPASS')!r}", flush=True)
    assert settings.get("STANAME") == ssid, "the saved name did not survive the run boundary"
    status, page = http(args.port, "/")
    print(f"GET /: {status}, {len(page)} B", flush=True)
    print("exit:", stop(process, log), flush=True)


if __name__ == "__main__":
    main()
