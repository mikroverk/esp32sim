//! The host-side run controls: `--exit-on`, `--timeout`, `--inject` / `--inject-on`, and `--uart-tcp` carrying
//! the mask ROM's UART download protocol (esptool's SLIP framing) over a socket. No external tools: the test
//! speaks SLIP itself. Stock esptool against the same bridge is checked by hand (docs/cli.md).
#[path = "../../tests/common.rs"]
mod common;
use common::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_esp32sim");
const FW: &str = "web/wasm/fw/public";

fn c3_hello(extra: &[&str]) -> Vec<String> {
    let rom = rom("esp32c3_rev3");
    let mut a: Vec<String> = ["--chip", "c3", "--rom", rom.to_str().unwrap(), "--boot", "rom", "--flash-mb", "4", "--strap", "0xd", "--no-dump",
        "--bootloader", &format!("{FW}/c3-hello-bootloader.bin"), "--ptable", &format!("{FW}/c3-hello-ptable.bin"), "--app", &format!("{FW}/c3-hello_world.bin")]
        .iter().map(|s| s.to_string()).collect();
    a.extend(extra.iter().map(|s| s.to_string()));
    a
}

fn emulated_seconds(stderr: &str) -> f64 {
    let line = stderr.lines().find(|l| l.starts_with("[emu] stop:")).unwrap_or_else(|| panic!("no stop line:\n{stderr}"));
    let i = line.find("emulated ").unwrap() + "emulated ".len();
    line[i..].split('s').next().unwrap().parse().unwrap()
}

/// `--exit-on` ends the run with status 0 as soon as the text is printed, well before `--max-seconds`; a text that
/// never comes ends the run at `--max-seconds` with status 1.
#[test] #[ignore = "needs the ESP32-C3 mask ROM ELF"]
fn exit_on_stops_the_run_when_the_console_prints_the_text() {
    let out = Command::new(BIN).args(c3_hello(&["--exit-on", "Restarting in 8 seconds", "--max-seconds", "30"])).current_dir(root()).output().unwrap();
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stdout.contains("Restarting in 8 seconds") && !stdout.contains("Restarting in 7 seconds"), "{stdout}");
    // hello_world prints on USB-Serial/JTAG and UART0; whichever stream shows the text first ends the run
    assert!(stderr.contains("[emu] --exit-on \"Restarting in 8 seconds\" matched on "), "{stderr}");
    assert!(emulated_seconds(&stderr) < 3.0, "{stderr}");
    let out = Command::new(BIN).args(c3_hello(&["--exit-on", "never printed", "--max-seconds", "1"])).current_dir(root()).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stderr));
}

/// `--timeout` counts wall-clock time and ends the run gracefully (the report is printed) with status 124.
#[test] #[ignore = "needs the ESP32-C3 mask ROM ELF"]
fn timeout_ends_the_run_on_wall_clock_time_with_its_own_status() {
    let t0 = Instant::now();
    let out = Command::new(BIN).args(c3_hello(&["--timeout", "300ms", "--realtime"])).current_dir(root()).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(124), "{stderr}");
    assert!(stderr.contains("[emu] --timeout: the wall-clock time ran out") && stderr.contains("[emu] stop: Halted"), "{stderr}");
    assert!(t0.elapsed() < Duration::from_secs(20));
    // paced to wall time, 300 ms of it is at most a little more of emulated time
    assert!(emulated_seconds(&stderr) < 1.0, "{stderr}");
}

/// The payload of `--inject` reaches UART0's receive FIFO when its `--inject-on` trigger has been printed: the ROM's
/// `uart_rx_one_char` path is not used by hello_world, so the check is the script log of the chain.
#[test] #[ignore = "needs the ESP32-C3 mask ROM ELF"]
fn inject_on_waits_for_its_trigger_then_writes_the_payload() {
    let out = Command::new(BIN).args(c3_hello(&["--inject-on", "Restarting in 9 seconds", "--inject", "ab\\r\\n", "--inject", "first",
        "--exit-on", "Restarting in 7 seconds", "--max-seconds", "30"])).current_dir(root()).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let lines: Vec<&str> = stderr.lines().filter(|l| l.starts_with("[script]")).collect();
    assert_eq!(lines.len(), 3, "{stderr}");
    assert!(lines[0].starts_with("[script] t=0.000s Uart(0, \"first\")"), "{lines:?}");
    assert!(lines[1].contains("wait uart0 matched \"Restarting in 9 seconds\""), "{lines:?}");
    assert!(lines[2].contains("Uart(0, \"ab\\r\\n\")"), "{lines:?}");
}

/// A console REPL reads what `--inject` writes: the P4 USB-MSC example's `esp32p4>` prompt is the trigger, `help`
/// the payload, and the command list it prints ends the run through `--exit-on`.
#[test] #[ignore = "needs the ESP32-P4 mask ROM ELF"]
fn inject_reaches_the_p4_console_repl_after_its_prompt() {
    for name in ["p4-usbmsc-bootloader.bin", "p4-usbmsc-ptable.bin", "p4-usbmsc.bin"] {
        assert!(root().join(FW).join(name).exists(), "{FW}/{name} is missing: tools/fetch-p4-firmware.sh fetches it");
    }
    let card = tmp("inject-card.img");
    std::fs::File::create(&card).unwrap().set_len(64 << 20).unwrap();   // a blank card: the example formats it
    let rom = rom("esp32p4_rev0");
    let out = Command::new(BIN).args(["--chip", "p4", "--rom", rom.to_str().unwrap(), "--flash-mb", "4", "--no-dump",
        "--bootloader", &format!("{FW}/p4-usbmsc-bootloader.bin"), "--ptable", &format!("{FW}/p4-usbmsc-ptable.bin"), "--app", &format!("{FW}/p4-usbmsc.bin"),
        "--sd", card.to_str().unwrap(), "--inject-on", "esp32p4>", "--inject", "help\\n", "--exit-on", "Expose Storage", "--max-seconds", "10"])
        .current_dir(root()).output().unwrap();
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stderr}\n{stdout}");
    assert!(stderr.contains("wait uart0 matched \"esp32p4>\"") && stderr.contains("Uart(0, \"help\\n\")"), "{stderr}");
    assert!(stdout.contains("show storage size and sector size"), "{stdout}");
    assert!(emulated_seconds(&stderr) < 5.0, "{stderr}");
}

fn slip(packet: &[u8]) -> Vec<u8> {
    let mut f = vec![0xc0];
    for &b in packet { match b { 0xc0 => f.extend([0xdb, 0xdc]), 0xdb => f.extend([0xdb, 0xdd]), b => f.push(b) } }
    f.push(0xc0);
    f
}

/// esptool's command packet: direction 0, opcode, length, checksum, data.
fn command(op: u8, data: &[u8]) -> Vec<u8> {
    let mut p = vec![0, op];
    p.extend((data.len() as u16).to_le_bytes());
    p.extend(0u32.to_le_bytes());
    p.extend(data);
    slip(&p)
}

/// SLIP frames out of a byte stream: returns the decoded packets and keeps a partial frame in `buf`.
fn frames(buf: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let Some(start) = buf.iter().position(|&b| b == 0xc0) else { buf.clear(); break };
        let Some(len) = buf[start + 1..].iter().position(|&b| b == 0xc0) else { buf.drain(..start); break };
        let raw: Vec<u8> = buf[start + 1..start + 1 + len].to_vec();
        buf.drain(..start + 1 + len);   // keep the closing 0xc0: it may open the next frame
        if raw.is_empty() { continue; }
        let mut p = Vec::new();
        let mut it = raw.into_iter();
        while let Some(b) = it.next() { p.push(if b == 0xdb { match it.next() { Some(0xdc) => 0xc0, Some(0xdd) => 0xdb, _ => 0 } } else { b }); }
        out.push(p);
    }
    out
}

/// Boot the chip's ROM in UART download mode with `--uart-tcp` on a free port, SYNC as esptool does, and read the
/// chip-detect magic word: the answer must come from the ROM over the socket.
fn rom_download_over_uart_tcp(chip: &str, rom_name: &str, strap: &str, magics: &[u32]) {
    let rom = rom(rom_name);
    let mut child = Command::new(BIN).args(["--chip", chip, "--rom", rom.to_str().unwrap(), "--boot", "rom", "--flash-mb", "4", "--strap", strap,
        "--uart-tcp", "127.0.0.1:0", "--timeout", "60s", "--no-dump"]).current_dir(root())
        .stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    let mut err = BufReader::new(child.stderr.take().unwrap());
    let mut addr = None;
    let mut line = String::new();
    while addr.is_none() && err.read_line(&mut line).unwrap() > 0 {
        if let Some(rest) = line.strip_prefix("[emu] UART0 on tcp ") { addr = rest.split(':').take(2).collect::<Vec<_>>().join(":").into(); }
        line.clear();
    }
    let addr = addr.unwrap_or_else(|| { let _ = child.kill(); panic!("no `[emu] UART0 on tcp` line") });
    // drain the rest of stderr so the emulator never blocks on it
    let log = std::thread::spawn(move || { let mut s = String::new(); let _ = err.read_to_string(&mut s); s });
    let mut s = TcpStream::connect(&addr).unwrap();
    s.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    let mut sync = vec![0x07, 0x07, 0x12, 0x20]; sync.extend([0x55; 32]);
    let (mut buf, mut synced, mut magic) = (Vec::new(), false, None);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut chunk = [0u8; 4096];
    while magic.is_none() && Instant::now() < deadline {
        s.write_all(&command(if synced { 0x0a } else { 0x08 }, if synced { &[0x00, 0x10, 0x00, 0x40] } else { &sync })).unwrap();
        let until = Instant::now() + Duration::from_millis(300);
        while Instant::now() < until {
            match s.read(&mut chunk) { Ok(0) => break, Ok(n) => buf.extend_from_slice(&chunk[..n]), Err(_) => {} }
            for p in frames(&mut buf) {
                if p.len() < 10 || p[0] != 1 { continue; }
                let status = p[8];
                match p[1] { 0x08 if status == 0 => synced = true, 0x0a if status == 0 => magic = Some(u32::from_le_bytes([p[4], p[5], p[6], p[7]])), _ => {} }
            }
            if magic.is_some() || (synced && buf.is_empty()) { break; }
        }
    }
    drop(s);
    let _ = child.kill(); let _ = child.wait();
    let log = log.join().unwrap();
    assert!(synced, "{chip}: no SYNC answer over the socket\n{log}");
    let magic = magic.unwrap_or_else(|| panic!("{chip}: no READ_REG answer\n{log}"));
    assert!(magics.contains(&magic), "{chip}: chip-detect magic {magic:#010x}, expected one of {magics:x?}");
    assert!(log.contains("[uart0-tcp] client"), "{log}");
}

// esptool's CHIP_DETECT_MAGIC_VALUE lists (esptool v4.12 targets/esp32c3.py, esp32s3.py)
#[test] #[ignore = "needs the ESP32-C3 mask ROM ELF"]
fn c3_rom_download_mode_answers_over_uart_tcp() { rom_download_over_uart_tcp("c3", "esp32c3_rev3", "0x2", &[0x6921506f, 0x1b31506f, 0x4881606f, 0x4361606f]); }

#[test] #[ignore = "needs the ESP32-S3 mask ROM ELF"]
fn s3_rom_download_mode_answers_over_uart_tcp() { rom_download_over_uart_tcp("s3", "esp32s3_rev0", "0x2", &[0x9]); }
