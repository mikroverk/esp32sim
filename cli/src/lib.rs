//! esp32sim — the command line, one front end for every chip (`--chip esp32|s3|c3|c6`; the `esp32sim-c3`
//! and `esp32sim-c6` binaries are `--chip c3` / `--chip c6`). Parsing and everything a run does are chip-agnostic over
//! `Machine<S>`; the few flags a chip owns (board, WiFi, camera, PSRAM, register presets) live in
//! its setup function.
use esp_soc::observers::{BlockProfile, Breakpoints, Coverage, IrqLatency, MmioHeat, PcHist, RegTrace, Trace, Vcd, Watch};
use esp_soc::{Machine, Soc, SocBus, Stop};
use emu_core::{Bus, Core};
use std::path::PathBuf;

pub mod cooja;
mod camera;

fn usage(chip: &str) -> ! {
    eprintln!("usage: esp32sim [--chip esp32|s3|c3|c6] --boot rom|app --bootloader B.bin --ptable P.bin --app A.bin [--elf X.elf]... [options]");
    eprintln!("       see docs/cli.md for every flag (default chip here: {})", chip);
    std::process::exit(2)
}

fn usage_error(message: &str) -> ! { eprintln!("{message}"); std::process::exit(2) }

fn timing_cycles(value: &str, name: &str) -> Result<u32, String> {
    value.parse().map_err(|_| format!("{name}: expected a nonnegative u32 cycle count, got {value:?}"))
}

fn validate_timing(o: &Opts) -> Result<(), String> {
    if o.approximate_timing && !matches!(o.chip.as_str(), "s3" | "esp32s3") {
        return Err("--approximate-timing, --approximate-memory and --approximate-cache require --chip s3".into());
    }
    if o.memory_contention && o.approximate_memory.is_none() {
        return Err("--memory-contention requires --approximate-memory".into());
    }
    if o.approximate_memory.is_some() && o.boot.as_deref() != Some("rom") {
        return Err("--approximate-memory requires --boot rom for its MMU shadow".into());
    }
    Ok(())
}

fn cache_config() -> Result<esp32s3::approximate_cache::CacheConfig, String> {
    let mut cache = esp32s3::approximate_cache::CacheConfig::default();
    for (name, cycles) in [("ESP32SIM_CACHE_FILL", &mut cache.fill_cycles), ("ESP32SIM_CACHE_WRITEBACK", &mut cache.writeback_cycles)] {
        match std::env::var(name) {
            Ok(value) => *cycles = timing_cycles(&value, name)?,
            Err(std::env::VarError::NotPresent) => {},
            Err(_) => return Err(format!("{name}: expected a UTF-8 cycle count")),
        }
    }
    Ok(cache)
}

fn hex(s: &str, what: &str) -> u32 { u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or_else(|_| { eprintln!("--{}: bad hex {}", what, s); std::process::exit(2) }) }
fn pair(s: &str, dflt: usize) -> (u32, usize) { match s.split_once(',') { Some((a, n)) => (hex(a, "addr"), n.parse().unwrap_or(dflt)), None => (hex(s, "addr"), dflt) } }

use esp_soc::load::stub_spec;

fn console_mask(name: &str) -> u32 {
    esp_soc::Console::parse_mask(name).unwrap_or_else(|| {
        eprintln!("--console: unknown source {name:?}; expected usb, uart0, both, all or none");
        std::process::exit(2)
    })
}

/// Everything the command line can say, chip-agnostic; `None` means "the chip's default".
#[derive(Default)]
pub struct Opts {
    pub approximate_timing: bool,
    pub approximate_memory: Option<u32>,
    pub memory_contention: bool,
    pub approximate_cache: bool,
    pub chip: String,
    pub rom: Option<PathBuf>, pub bootloader: Option<String>, pub ptable: Option<String>, pub app: Option<String>, pub elfs: Vec<String>,
    pub flash_image: Option<String>, pub flash_at: Vec<String>, pub boot: Option<String>, pub flash_mb: Option<usize>, pub psram_mb: Option<usize>,
    pub mac: Option<[u8; 6]>, pub strap: Option<u32>, pub reset_cause: Option<u32>, pub efuse_regs: Option<String>, pub regs_init: Option<String>,
    pub board: String, pub wifi: Option<String>, pub ble: bool, pub ble_full: bool, pub ble_observe: bool, pub net: String, pub cam_image: Option<String>, pub cam_stream: Option<String>, pub cam_size: Option<String>, pub cam_fps: f64,
    pub spi2_timing: bool, pub measured_te: bool,
    pub max_insns: u64, pub max_seconds: Option<f64>, pub script: Option<String>, pub serial: Option<String>,
    pub console: Option<String>, pub console_prefix: bool, pub realtime: bool, pub web_port: Option<u16>, pub web_dir: Option<String>, pub no_reboot: bool,
    pub wav: Option<String>, pub tft_png: Option<String>, pub gram_png: Option<String>, pub dump: bool,
    pub trace: bool, pub trace_from: u64, pub breaks: Vec<u32>, pub watch: Option<u32>, pub peeks: Vec<(u32, usize)>, pub pwm_pins: Vec<u8>, pub disasms: Vec<(u32, usize)>,
    pub profile: bool, pub profile_blocks: bool, pub coverage: Option<Option<String>>, pub irq_latency: bool, pub vcd: Option<String>,
    pub regstat: Option<String>, pub regtrace: Option<String>, pub regtrace_max: u64, pub regtrace_from_pc: Option<u32>,
    pub stubs: Vec<String>, pub trace_fns: Vec<String>, pub stop_exc: u64, pub log_periph: bool, pub no_jit: bool, pub debug: Vec<String>,
    /// `--cooja`: run as a Cooja-NG external mote over NDJSON on stdin/stdout (ESP32-C6 only)
    pub cooja: bool, pub cooja_slice_us: u64, pub cooja_verbose: bool, pub cooja_rx_at_end: bool,
}

pub fn parse(args: &[String], default_chip: &str) -> Opts {
    let mut o = Opts { chip: default_chip.to_string(), board: "atech14".into(), net: "nat".into(), cam_fps: 10.0, max_insns: u64::MAX, dump: true, regtrace_max: u64::MAX, stop_exc: u64::MAX, cooja_slice_us: 100, ..Default::default() };
    let mut i = 1;
    while i < args.len() {
        let a = args[i].as_str();
        let mut next = || { i += 1; args.get(i).cloned().unwrap_or_else(|| usage(default_chip)) };
        match a {
            "--approximate-timing" => o.approximate_timing = true,
            "--approximate-memory" => { o.approximate_timing = true; o.approximate_memory = Some(timing_cycles(&next(), "--approximate-memory").unwrap_or_else(|e| usage_error(&e))); }
            "--memory-contention" => o.memory_contention = true,
            "--approximate-cache" => { o.approximate_timing = true; o.approximate_cache = true; },
            "--chip" => o.chip = next().to_ascii_lowercase(),
            "--rom" => o.rom = Some(PathBuf::from(next())),
            "--bootloader" => o.bootloader = Some(next()),
            "--ptable" => o.ptable = Some(next()),
            "--app" => o.app = Some(next()),
            "--elf" => o.elfs.push(next()),
            "--flash-image" => o.flash_image = Some(next()),
            "--flash-at" => o.flash_at.push(next()),
            "--boot" => o.boot = Some(next()),
            "--flash-mb" => o.flash_mb = Some(next().parse().expect("mb")),
            "--psram-mb" => o.psram_mb = Some(next().parse().expect("mb")),
            "--mac" => { let v = next(); let b: Vec<u8> = v.split(':').filter_map(|x| u8::from_str_radix(x, 16).ok()).collect(); if b.len() != 6 { eprintln!("--mac wants xx:xx:xx:xx:xx:xx"); std::process::exit(2); } let mut m = [0u8; 6]; m.copy_from_slice(&b); o.mac = Some(m); }
            "--strap" => o.strap = Some(hex(&next(), "strap")),
            "--reset-cause" => o.reset_cause = Some(hex(&next(), "reset-cause")),
            "--efuse-regs" => o.efuse_regs = Some(next()),
            "--regs-init" => o.regs_init = Some(next()),
            "--board" => o.board = next(),
            "--spi2-timing" => o.spi2_timing = true,
            "--measured-te" => o.measured_te = true,
            "--wifi" => o.wifi = Some(next()),
            "--ble-observe" => o.ble_observe = true,
            "--ble" => {
                let full = args.get(i + 1).is_some_and(|s| s == "full");
                if full { i += 1; o.ble_full = true; } else { o.ble = true; }
            },
            "--net" => o.net = next(),
            "--cam-image" => o.cam_image = Some(next()),
            "--cam-stream" => o.cam_stream = Some(next()),
            "--cam-size" => o.cam_size = Some(next()),
            "--cam-fps" => o.cam_fps = next().parse().expect("fps"),
            "--max-insns" => o.max_insns = next().replace('_', "").parse().expect("max-insns"),
            "--max-seconds" => o.max_seconds = Some(next().parse().expect("seconds")),
            "--script" => o.script = Some(next()),
            "--serial" => o.serial = Some(next()),
            "--console" => o.console = Some(next()),
            "--console-prefix" => o.console_prefix = true,
            "--realtime" => o.realtime = true,
            "--web" => o.web_port = Some(next().parse().expect("port")),
            "--web-dir" => o.web_dir = Some(next()),
            "--no-reboot" => o.no_reboot = true,
            "--wav" => o.wav = Some(next()),
            "--tft-png" => o.tft_png = Some(next()),
            "--gram-png" => o.gram_png = Some(next()),
            "--no-dump" => o.dump = false,
            "--trace" => o.trace = true,
            "--trace-from" => { o.trace = true; o.trace_from = next().replace('_', "").parse().expect("trace-from") }
            "--break" => o.breaks.push(hex(&next(), "break")),
            "--watch" => o.watch = Some(hex(&next(), "watch")),
            "--peek" => o.peeks.push(pair(&next(), 8)),
            "--pwm" => o.pwm_pins.push(next().parse().unwrap_or_else(|_| usage_error("--pwm: expected a pin number in 0..255"))),
            "--disasm" => o.disasms.push(pair(&next(), 16)),
            "--profile" => o.profile = true,
            "--profile-blocks" => o.profile_blocks = true,
            "--coverage" => o.coverage = Some(None),
            "--coverage-file" => o.coverage = Some(Some(next())),
            "--irq-latency" => o.irq_latency = true,
            "--vcd" => o.vcd = Some(next()),
            "--regstat" => o.regstat = Some(next()),
            "--regtrace" => o.regtrace = Some(next()),
            "--regtrace-max" => o.regtrace_max = next().parse().expect("n"),
            "--regtrace-from-pc" => o.regtrace_from_pc = Some(hex(&next(), "regtrace-from-pc")),
            "--stub" => o.stubs.push(next()),
            "--trace-fn" => o.trace_fns.push(next()),
            "--stop-after-exceptions" => o.stop_exc = next().parse().expect("count"),
            "--log-periph" => o.log_periph = true,
            "--no-jit" => o.no_jit = true,
            "--debug" => o.debug.push(next()),
            "--cooja" => o.cooja = true,
            "--cooja-slice-us" => o.cooja_slice_us = next().parse().expect("slice-us"),
            "--cooja-verbose" => o.cooja_verbose = true,
            "--cooja-rx-timing" => o.cooja_rx_at_end = match next().as_str() { "start" => false, "end" => true, x => { eprintln!("--cooja-rx-timing {}: start or end", x); std::process::exit(2) } },
            "-h" | "--help" => usage(default_chip),
            _ => { eprintln!("unknown arg {}", a); usage(default_chip) }
        }
        i += 1;
    }
    if o.ble_observe && !o.ble_full { usage_error("--ble-observe requires --ble full"); }
    if o.ble_full && o.ble { usage_error("--ble full and --ble are mutually exclusive"); }
    if o.ble_full && !matches!(o.chip.as_str(), "c3" | "esp32c3") { usage_error("--ble full requires C3"); }
    if let Some(path) = &o.script {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| usage_error(&format!("script: {e}")));
        for (ln, line) in text.lines().enumerate() {
            let mut words = line.split_whitespace();
            if words.next().is_none_or(|s| s.starts_with('#')) || words.next() != Some("ble") { continue }
            words.collect::<Vec<_>>().join(" ").parse::<esp_soc::ble::peer::Command>()
                .unwrap_or_else(|e| usage_error(&format!("script: line {}: {e}", ln + 1)));
            if !o.ble { usage_error(&format!("script: line {}: BLE requires --ble and the application ELF", ln + 1)); }
        }
    }
    if matches!(o.chip.as_str(), "esp32" | "classic") && !args.iter().any(|a| a == "--board") { o.board = "esp32dev".into(); }
    o
}

/// ESP32SIM_ROM_DIR, then `~/.espressif/tools/esp-rom-elfs/*/<name>` (newest first).
fn find_rom(name: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ESP32SIM_ROM_DIR").map(|d| PathBuf::from(d).join(name)).filter(|p| p.is_file()) { return Some(p) }
    let home = std::env::var("HOME").ok()?;
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(format!("{}/.espressif/tools/esp-rom-elfs", home)).ok()?.flatten().map(|e| e.path()).collect();
    dirs.sort();
    dirs.into_iter().rev().map(|d| d.join(name)).find(|p| p.exists())
}

pub fn run_cli(default_chip: &str) {
    let args: Vec<String> = std::env::args().collect();
    let mut o = parse(&args, default_chip);
    if o.cam_stream.is_some() != o.cam_size.is_some() { usage_error("--cam-stream and --cam-size must be supplied together"); }
    validate_timing(&o).unwrap_or_else(|e| usage_error(&e));
    if o.approximate_cache { cache_config().unwrap_or_else(|e| usage_error(&e)); }
    if o.cooja { return run_cooja(&mut o); }
    match o.chip.as_str() {
        "esp32" | "classic" => { let m = setup_esp32(&o); run(m, &o) }
        "s3" | "esp32s3" => { let m = setup_s3(&o); run(m, &o) }
        "c3" | "esp32c3" => { let m = setup_c3(&o); run(m, &o) }
        "c6" | "esp32c6" => { let m = setup_c6(&o); run(m, &o) }
        c => { eprintln!("--chip {}: s3, c3 or c6", c); std::process::exit(2) }
    }
}

/// `--cooja`: the C6 as a Cooja-NG external mote. csim's `hello` comes first — its node id names
/// the MAC unless `--mac` did — then the machine is set up and booted exactly as for a normal
/// run, and the NDJSON exchange takes the place of the run loop. The guest console never
/// reaches stdout: it goes to csim as `log` events. The usual report goes to stderr at the end.
fn run_cooja(o: &mut Opts) {
    if !matches!(o.chip.as_str(), "c6" | "esp32c6") { eprintln!("--cooja: only the ESP32-C6 speaks the Cooja-NG lock-step protocol"); std::process::exit(2); }
    if o.max_insns != u64::MAX { eprintln!("--cooja: --max-insns is unsupported; use --max-seconds"); std::process::exit(2); }
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let hello = match cooja::read_hello(&mut input) { Ok(h) => h, Err(e) => { eprintln!("[cooja] {}", e); std::process::exit(2) } };
    if o.mac.is_none() { o.mac = Some(cooja::mac_for_node(hello.id)); }
    if o.console.is_none() { o.console = Some("uart0".into()); }
    let mut m = setup_c6(o);
    let boot = prepare(&mut m, o);
    let cfg = cooja::Config {
        slice_ns: o.cooja_slice_us.max(1).saturating_mul(1000),
        console_mask: console_mask(o.console.as_deref().unwrap_or("uart0")),
        rx_on_air: !o.cooja_rx_at_end,
        verbose: o.cooja_verbose,
        reboot: !o.no_reboot && boot == "rom",
    };
    eprintln!("[cooja] node {} ({}), mac {}, slice {} µs", hello.id, boot, o.mac.map(|m| m.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(":")).unwrap_or_default(), cfg.slice_ns / 1000);
    let t0 = std::time::Instant::now();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let summary = match cooja::run(&mut m, cfg, &hello, &mut input, &mut output) { Ok(s) => s, Err(e) => { eprintln!("[cooja] {}", e); std::process::exit(1) } };
    let dt = t0.elapsed().as_secs_f64();
    eprintln!("[cooja] {} steps, {} early yields, {} tx, {} rx ({} dropped), {} log lines; {:.3} s simulated in {:.1} s wall ({} cycles, {:.1} Mcycle/s)",
              summary.steps, summary.yields, summary.tx, summary.rx, summary.rx_dropped, summary.logs, summary.sim_ns as f64 / 1e9, dt, m.bus.cycles(), m.bus.cycles() as f64 / dt / 1e6);
    report(&mut m, o, match summary.stopped { Some(_) => Stop::Halted, None => Stop::MaxInsns }, dt);
}

fn setup_s3(o: &Opts) -> esp32s3::Machine {
    let size_error = o.cam_size.as_deref().and_then(|s| camera::size(s).err());
    for (message, invalid) in [
        ("--cam-stream requires --board waveshare-cam", o.cam_stream.is_some() && o.board != "waveshare-cam"),
        (size_error.as_deref().unwrap_or(""), size_error.is_some()),
    ] {
        if invalid { usage_error(message); }
    }
    let mut m = esp32s3::machine(o.mac.unwrap_or([0x44, 0x1b, 0xf6, 0x75, 0xdc, 0xe0]));
    m.bus.board = esp32s3::board::make_board(&o.board).unwrap_or_else(|| { eprintln!("unknown board '{}' (atech14, waveshare-cam, waveshare-lcd4b, waveshare-amoled18-v2, none)", o.board); std::process::exit(2) });
    if o.measured_te {
        assert_eq!(m.bus.board.name(), "waveshare-amoled18-v2", "--measured-te requires the AMOLED V2 board");
        m.bus.board = Box::new(esp32s3::board::WaveshareAmoled18V2::with_measured_te());
    }
    m.bus.spi2_timing = o.spi2_timing;
    m.bus.attach_board_devices();
    if !o.debug.is_empty() { let mut f = esp_soc::DebugFlags::from_env(); for d in &o.debug { f.parse(d); } m.set_debug(&f); }
    if let Some(spec) = &o.wifi {
        let (cfg, nat) = wifi_config(spec, o.net == "nat" || o.net == "user", m.bus.debug.has("net"));
        m.bus.attach_wifi(cfg, nat).unwrap_or_else(|e| { eprintln!("--wifi: {e}"); std::process::exit(2) });
    }
    if let Some(p) = &o.cam_image { match esp_soc::picture::load(p) { Ok(pic) => { eprintln!("[emu] camera picture {} ({}x{})", p, pic.w, pic.h); m.bus.board.set_camera_picture(pic).unwrap_or_else(|e| usage_error(e)); } Err(e) => { eprintln!("[emu] {}", e); std::process::exit(2); } } }
    m.bus.periph.lcd_cam.frame_cycles = (esp32s3::periph::CPU_HZ as f64 / o.cam_fps) as u64;
    if let Some(mb) = o.flash_mb { if mb != 8 { m.bus.set_flash_size(mb << 20); } }
    if let Some(mb) = o.psram_mb { if mb != 2 { m.bus.set_psram_size(mb << 20).unwrap(); } }
    if let Some(p) = &o.efuse_regs {
        let txt = std::fs::read_to_string(p).expect("efuse file");
        let mut n = 0;
        for line in txt.lines() {
            let line = line.trim(); if line.is_empty() { continue; }
            let (addr_s, rest) = match line.split_once(':') { Some(x) => x, None => continue };
            let Ok(mut a) = u32::from_str_radix(addr_s.trim().trim_start_matches("0x"), 16) else { continue };
            for w in rest.split_whitespace() { if let Ok(v) = u32::from_str_radix(w, 16) { let off = if a >= 0x6000_7000 { a - 0x6000_7000 } else { a }; m.bus.periph.efuse.ram.write(off, v); a += 4; n += 1; } }
        }
        eprintln!("[emu] loaded {} efuse words from {}", n, p);
    }
    if let Some(p) = &o.regs_init {
        let txt = std::fs::read_to_string(p).expect("regs-init file");
        let mut n = 0;
        for line in txt.lines() {
            let (addr_s, rest) = match line.trim().split_once(':') { Some(x) => x, None => continue };
            let Ok(mut a) = u32::from_str_radix(addr_s.trim().trim_start_matches("0x"), 16) else { continue };
            for w in rest.split_whitespace() { if let Ok(v) = u32::from_str_radix(w, 16) { if m.bus.periph.init_regs(a, v) { n += 1; } a += 4; } }
        }
        eprintln!("[emu] applied {} reset-state register words from {}", n, p);
    }
    if let Some(p) = &o.regstat { m.add_observer(Box::new(MmioHeat::new(p, |a| { let b = a.wrapping_sub(esp32s3::periph::PERIPH_BASE) >> 12; format!("{}+0x{:03x}", esp32s3::periph::Peripherals::block_name_pub(b), a & 0xfff) }))); }
    m
}

/// `--wifi SPEC` with `--net`: the virtual access point's configuration and, for `--net nat`,
/// the NAT to the host, the same for every chip.
fn wifi_config(spec: &str, nat: bool, net_log: bool) -> (esp_soc::wifi::ApConfig, Option<esp_soc::nat::Nat>) {
    let cfg = esp_soc::wifi::ApConfig::parse(spec).unwrap_or_else(|e| { eprintln!("--wifi: {e}"); std::process::exit(2) });
    eprintln!("[emu] virtual AP '{}' bssid {} channel {} ({})", cfg.ssid, esp_soc::wifi::mac_str(&cfg.bssid), cfg.channel, if cfg.psk.is_some() { "WPA2-PSK" } else { "open" });
    let nat = nat.then(|| esp_soc::nat::Nat::new(net_log));
    if let Some(nat) = &nat { eprintln!("[emu] NAT to the host network enabled (DNS via {}.{}.{}.{})", nat.resolver[0], nat.resolver[1], nat.resolver[2], nat.resolver[3]); }
    let net = esp_soc::net::VirtualNet::new(false);   // its fixed addresses, for the line below
    eprintln!("[emu] virtual network: station {}.{}.{}.{}, gateway {}.{}.{}.{} (DHCP, ARP, ICMP, DNS, NTP)", net.sta_ip[0], net.sta_ip[1], net.sta_ip[2], net.sta_ip[3], net.gw_ip[0], net.gw_ip[1], net.gw_ip[2], net.gw_ip[3]);
    (cfg, nat)
}

fn setup_c3(o: &Opts) -> esp32c3::Machine {
    let mut m = esp32c3::machine(o.mac.unwrap_or([0x60, 0x55, 0xf9, 0x00, 0x11, 0x22]), o.flash_mb.unwrap_or(4) << 20);
    m.bus.set_flash_size(o.flash_mb.unwrap_or(4) << 20);   // the JEDEC capacity follows the size
    if !o.debug.is_empty() { let mut f = esp_soc::DebugFlags::from_env(); for d in &o.debug { f.parse(d); } m.set_debug(&f); }
    if o.ble_full { m.bus.enable_ble_full(o.ble_observe).unwrap_or_else(|e| usage_error(&e)); }
    if let Some(spec) = &o.wifi {
        let (cfg, nat) = wifi_config(spec, o.net == "nat" || o.net == "user", m.bus.debug.has("net"));
        m.bus.attach_wifi(cfg, nat).unwrap_or_else(|e| { eprintln!("--wifi: {e}"); std::process::exit(2) });
    }
    for (flag, on) in [("--board", o.board != "atech14" && o.board != "none"), ("--cam-image", o.cam_image.is_some()), ("--cam-stream", o.cam_stream.is_some()), ("--cam-size", o.cam_size.is_some()), ("--psram-mb", o.psram_mb.is_some()), ("--efuse-regs", o.efuse_regs.is_some()), ("--regs-init", o.regs_init.is_some()), ("--regstat", o.regstat.is_some())] {
        if on { eprintln!("{} is not available on the C3", flag); std::process::exit(2); }
    }
    m
}

fn setup_esp32(o: &Opts) -> esp32::Machine {
    let mut m = esp32::machine(o.mac.unwrap_or([0x24, 0x6f, 0x28, 0x00, 0x11, 0x22]), o.flash_mb.unwrap_or(4) << 20);
    m.bus.set_flash_size(o.flash_mb.unwrap_or(4) << 20);
    if !o.debug.is_empty() { let mut f = esp_soc::DebugFlags::from_env(); for d in &o.debug { f.parse(d); } m.set_debug(&f); }
    let board = &o.board;
    m.bus.board = esp32::board::make_board(board).unwrap_or_else(|| usage_error(&format!("unknown classic ESP32 board '{board}' (none, bare, esp32dev)")));
    for (flag, on) in [("--wifi", o.wifi.is_some()), ("--cam-image", o.cam_image.is_some()), ("--psram-mb", o.psram_mb.is_some()), ("--efuse-regs", o.efuse_regs.is_some()), ("--regs-init", o.regs_init.is_some()), ("--regstat", o.regstat.is_some())] {
        if on { eprintln!("{} is not available on the classic ESP32", flag); std::process::exit(2); }
    }
    m
}

fn setup_c6(o: &Opts) -> esp32c6::Machine {
    let mut m = esp32c6::machine(o.mac.unwrap_or([0xdc, 0x1e, 0xd5, 0x6e, 0x8c, 0xdc]), o.flash_mb.unwrap_or(4) << 20);
    m.bus.set_flash_size(o.flash_mb.unwrap_or(4) << 20);   // the JEDEC capacity follows the size
    if !o.debug.is_empty() { let mut f = esp_soc::DebugFlags::from_env(); for d in &o.debug { f.parse(d); } m.set_debug(&f); }
    let name = if o.board == "atech14" { "none" } else { o.board.as_str() };   // the S3 default means "bare module" here
    match esp32c6::board::make_board(name) { Some(b) => m.bus.board = b, None => { eprintln!("--board {}: none or waveshare-c6-lcd147 on the C6", name); std::process::exit(2) } }
    m.bus.attach_board_devices();
    if let Some(spec) = &o.wifi {
        let (cfg, nat) = wifi_config(spec, o.net == "nat" || o.net == "user", m.bus.debug.has("net"));
        m.bus.attach_wifi(cfg, nat).unwrap_or_else(|e| { eprintln!("--wifi: {e}"); std::process::exit(2) });
    }
    for (flag, on) in [("--cam-image", o.cam_image.is_some()), ("--cam-stream", o.cam_stream.is_some()), ("--cam-size", o.cam_size.is_some()), ("--psram-mb", o.psram_mb.is_some()), ("--efuse-regs", o.efuse_regs.is_some()), ("--regs-init", o.regs_init.is_some()), ("--regstat", o.regstat.is_some())] {
        if on { eprintln!("{} is not available on the C6", flag); std::process::exit(2); }
    }
    m
}

/// Everything after the chip is set up: images, boot, observers, the run, the reports.
fn run<S: Soc>(mut m: Machine<S>, o: &Opts) {
    let approximate = o.approximate_timing.then(|| {
        let mut config = esp32s3::ApproximateTimingConfig::default();
        if o.approximate_cache && o.approximate_memory.is_none() {
            config.data_cache = Some(cache_config().unwrap_or_else(|e| usage_error(&e)));
        }
        esp32s3::ApproximateCostModel::new(config)
    });
    let memory_model = o.approximate_memory.map(|extra| {
        use esp32s3::rough_memory::{MemoryConfig, MemoryPrice};
        let external = MemoryPrice { latency: extra, ..MemoryPrice::FREE };
        let model = esp32s3::memory_cost_model::MemoryCostModel::new(approximate.as_ref().unwrap().clone(),
            MemoryConfig { flash: external, psram: external, contention: o.memory_contention, ..Default::default() });
        if o.approximate_cache { model.with_cache(cache_config().unwrap_or_else(|e| usage_error(&e))) } else { model }
    });
    if let Some(model) = &approximate {
        let cost: Box<dyn emu_core::CostModel> = match &memory_model {
            Some(memory) => Box::new(memory.clone()), None => Box::new(model.clone()),
        };
        m.set_cost_model(cost).unwrap_or_else(|e| usage_error(&format!("--approximate-timing: {e}")));
        eprintln!("[emu] APPROXIMATE timing: {:?}; use --boot rom; accuracy unvalidated", model.config);
    }
    let boot = prepare(&mut m, o);
    let t0 = std::time::Instant::now();
    m.web_restart = !o.no_reboot;
    let stop = run_with_reboots(&mut m, o.max_insns, !o.no_reboot && boot == "rom", boot == "app");
    let dt = t0.elapsed().as_secs_f64();
    report(&mut m, o, stop, dt);
    let pending = m.bus.ble_pending_commands();
    if pending != 0 { eprintln!("[ble] {pending} command(s) still pending at end of run"); }
    if let Some(model) = approximate { eprintln!("[emu] approximate timing totals: {:?}", model.stats()); }
    if let Some(model) = memory_model {
        eprintln!("[emu] approximate memory totals [internal, ROM, flash, PSRAM, MMIO]: {:?}", model.memory.borrow().stats);
        if let Some(cache) = &model.cache { eprintln!("[emu] approximate physical data cache: {:?}", cache.borrow().stats()); }
    }
}

/// Run, coming back up after a chip reset where that is possible: through the ROM when it booted
/// the run (`reboot`), and for an app-mode run (`app`) when the reset was the page's Restart — the
/// chip is reset and the app mapped and entered again, as at startup. A reset the firmware asked
/// for still ends an app-mode run: there is no ROM to take it through.
fn run_with_reboots<S: Soc>(m: &mut Machine<S>, mut remaining: u64, reboot: bool, app: bool) -> Stop {
    loop {
        let before = m.run_steps();
        let stop = m.run(remaining);
        remaining = remaining.saturating_sub(m.run_steps().saturating_sub(before));
        if let Stop::SwReset = stop {
            let cause = m.bus.reset_cause();
            eprintln!("[emu] chip reset at t={:.3}s: cause {:#x} ({})", m.seconds(), cause, esp_periph::reset_cause_name(cause));
            let button = m.take_button_reset();
            if !reboot && !(button && app) { return stop; }
            if remaining == 0 { return Stop::MaxInsns; }
            m.reboot();
            if !reboot {
                match m.boot_app(0x10000) {
                    Ok(entry) => eprintln!("[emu] restart: app entry {:#010x} {}", entry, m.sym(entry)),
                    Err(e) => { eprintln!("[emu] restart: {}", e); return stop; }
                }
            }
        } else { return stop; }
    }
}

/// Images, boot, observers, scripts: everything before the first instruction. Returns the boot mode.
fn prepare<S: Soc>(m: &mut Machine<S>, o: &Opts) -> String {
    let c3 = S::CORES == 1;
    let boot = o.boot.clone().unwrap_or_else(|| if c3 { "rom".into() } else { "app".into() });
    let console = o.console.clone().unwrap_or_else(|| if c3 { "uart0".into() } else { "both".into() });
    m.bus.misc().log_unknown = o.log_periph;
    if !o.breaks.is_empty() { m.add_observer(Box::new(Breakpoints { pcs: o.breaks.clone() })); }
    if o.trace { m.add_observer(Box::new(Trace { from: o.trace_from })); }
    let rom = o.rom.clone().or_else(|| find_rom(S::ROM_ELF));
    match &rom {
        Some(r) => match std::fs::read(r) { Ok(d) => { m.load_rom(&d).expect("rom"); eprintln!("[emu] ROM loaded from {}", r.display()); } Err(e) => eprintln!("[emu] no ROM ({}): {}", r.display(), e) },
        None if boot == "rom" => { eprintln!("[emu] no {} mask ROM ELF found (pass --rom, or use --boot app)", S::NAME); std::process::exit(2) }
        None => {}
    }
    if let Some(p) = &o.flash_image { m.write_flash(0, &std::fs::read(p).expect("flash image")).unwrap(); }
    if let Some(p) = &o.bootloader { m.write_flash(S::BOOTLOADER_OFFSET, &std::fs::read(p).expect("bootloader")).unwrap(); }
    if let Some(p) = &o.ptable { m.write_flash(0x8000, &std::fs::read(p).expect("ptable")).unwrap(); }
    if let Some(p) = &o.app { m.write_flash(0x10000, &std::fs::read(p).expect("app")).unwrap(); }
    for spec in &o.flash_at {
        let (off, path) = spec.split_once('=').unwrap_or_else(|| { eprintln!("--flash-at needs OFFSET=FILE"); std::process::exit(2) });
        let off = usize::from_str_radix(off.trim_start_matches("0x"), 16).unwrap_or_else(|_| { eprintln!("--flash-at: bad offset {}", off); std::process::exit(2) });
        let data = std::fs::read(path).unwrap_or_else(|e| { eprintln!("--flash-at: {}: {}", path, e); std::process::exit(2) });
        m.write_flash(off, &data).unwrap_or_else(|e| { eprintln!("--flash-at: {}", e); std::process::exit(2) });
        eprintln!("[emu] flash {:#x}: {} ({} bytes)", off, path, data.len());
    }
    let mut ble_elf = esp_soc::elf::Elf::default();
    for p in &o.elfs {
        let data = std::fs::read(p).expect("elf");
        m.add_symbols(&data).expect("elf symbols");
        if o.ble { let elf = esp_soc::elf::parse(&data).expect("BLE ELF symbols"); if elf.by_name.contains_key("esp_bt_controller_init") { ble_elf = elf; } }
    }
    if o.ble { m.bus.enable_ble(&ble_elf).unwrap_or_else(|e| usage_error(&format!("--ble: {e}"))); }
    if let Some(p) = &o.script { m.load_script(&std::fs::read_to_string(p).unwrap_or_else(|e| usage_error(&format!("script: {e}")))).unwrap_or_else(|e| usage_error(&format!("script: {e}"))); }
    if let Some(s) = &o.serial { m.bus.serial_input(s.as_bytes()); }
    for pre in &o.trace_fns {
        let n = m.trace_fns(pre);
        eprintln!("[emu] --trace-fn {}: {} functions", pre, n);
    }
    for st in &o.stubs {
        let (name, val) = stub_spec(st).unwrap_or_else(|e| { eprintln!("--stub: {e}"); std::process::exit(2) });
        let addr = m.resolve_stub(name).unwrap_or_else(|| { eprintln!("--stub: unknown symbol {}", name); std::process::exit(2) });
        eprintln!("[emu] stub {} @ {:#010x} -> returns {:#x}", name, addr, val);
        m.stubs.insert(addr, val);
    }
    if o.no_jit { for c in &mut m.cores { c.set_jit(false); } }
    match boot.as_str() {
        "app" => match m.boot_app(0x10000) { Ok(entry) => eprintln!("[emu] app boot: entry {:#010x} {}", entry, m.sym(entry)), Err(e) => { eprintln!("[emu] {}", e); std::process::exit(2) } },
        "rom" => { m.boot_rom(); eprintln!("[emu] ROM boot from reset vector {:#010x}", m.cores[0].pc()); }
        _ => { eprintln!("--boot app|rom"); std::process::exit(2); }
    }
    // Match a real board's boot conditions: the ROM prints the reset cause and the strapping-derived boot mode
    if let Some(c) = o.reset_cause { m.bus.set_reset_cause(c); }
    if let Some(v) = o.strap { m.bus.set_strap(v); }
    for &(a, n) in &o.peeks { eprintln!("[peek before run]\n{}", m.peek(a, n)); }
    m.dbg.stop_after_exceptions = o.stop_exc;
    m.console.mask = console_mask(&console);
    m.console.prefix = o.console_prefix;
    if let Some(p) = &o.regtrace { m.add_observer(Box::new(RegTrace::new(std::fs::File::create(p).expect("regtrace file"), o.regtrace_max, o.regtrace_from_pc))); }
    if let Some(port) = o.web_port {
        let dir = o.web_dir.clone().unwrap_or_else(|| { let exe = std::env::current_exe().unwrap(); let mut d = exe.parent().unwrap().to_path_buf(); for _ in 0..3 { if d.join("web").exists() { break; } d = d.parent().unwrap().to_path_buf(); } d.join("web").to_string_lossy().to_string() });
        let w = esp_soc::web::WebServer::start(port, dir.clone()).expect("web server");
        eprintln!("[emu] board UI: http://127.0.0.1:{}/  (serving {})", port, dir);
        m.web = Some(w); m.rt.enabled = true;
    }
    if let Some(path) = &o.cam_stream {
        let (width, height) = camera::size(o.cam_size.as_deref().unwrap()).unwrap();
        let input = std::sync::Arc::new(std::sync::Mutex::new(None));
        m.camera_input = Some(input.clone());
        let path = path.clone();
        std::thread::spawn(move || {
            let reader: std::io::Result<Box<dyn std::io::Read>> = if path == "-" { Ok(Box::new(std::io::stdin())) }
                else { std::fs::File::open(path).map(|file| Box::new(file) as Box<dyn std::io::Read>) };
            let result = reader.and_then(|reader| camera::read_frames(reader, width, height, |frame| *input.lock().unwrap() = Some(frame)));
            if let Err(e) = result { eprintln!("[emu] camera stream: {e}"); }
        });
        m.rt.enabled = true;
    }
    if o.realtime { m.rt.enabled = true; }
    if o.profile { m.add_observer(Box::new(PcHist::new(12))); }
    if let Some(wa) = o.watch { let v = m.bus.read32_unpriced(wa).unwrap_or(0); m.add_observer(Box::new(Watch { addr: wa, value: v })); }
    if o.profile_blocks { m.add_observer(Box::new(BlockProfile::new(20))); }
    if let Some(path) = &o.coverage { m.add_observer(Box::new(Coverage::new(path.clone()))); }
    if o.irq_latency { m.add_observer(Box::new(IrqLatency::new(S::CORES))); }
    if let Some(p) = &o.vcd { m.add_observer(Box::new(Vcd::new(p, S::CPU_HZ))); }
    if let Some(sec) = o.max_seconds { m.max_cycles = (sec * S::CPU_HZ as f64) as u64; }
    boot
}

/// The end-of-run report on stderr: counts, faults, peeks, observer reports, captures, registers.
fn report<S: Soc>(m: &mut Machine<S>, o: &Opts, stop: Stop, dt: f64) {
    let total = m.insns();
    let per: String = if S::CORES == 1 { format!("{}", total) } else { m.cores.iter().enumerate().map(|(i, c)| format!("core{} {}", i, c.insn_count())).collect::<Vec<_>>().join(" + ") };
    eprintln!("\n[emu] stop: {:?} — {} insns in {:.1}s wall = {:.1} Minsn/s; emulated {:.3}s ({} cycles); {} exceptions, {} interrupts",
              stop, per, dt, total as f64 / dt / 1e6, m.seconds(), m.bus.cycles(), m.exceptions, m.interrupts);
    if let Stop::Unimplemented(pc, raw) = stop {
        if let Ok(b) = m.bus.fetch(pc) { eprintln!("[emu] unimplemented at {:08x} {}: {} (raw {:#x})", pc, m.sym(pc), m.cores[0].disasm(pc, b), raw); }
    }
    if let Some((a, w)) = m.bus.last_fault() { eprintln!("[emu] last bus fault: {} {:#010x}", if w { "write" } else { "read" }, a); }
    for &(a, n) in &o.peeks { eprintln!("[peek after run]\n{}", m.peek(a, n)); }
    for &pin in &o.pwm_pins {
        match m.bus.pwm_output(pin) {
            Some((hz, duty)) => eprintln!("[pwm] GPIO{}: {:.3} Hz, {:.2}% duty", pin, hz, duty as f64 * 100.0 / 65535.0),
            None => eprintln!("[pwm] GPIO{}: no supported running PWM", pin),
        }
    }
    for &(a, n) in &o.disasms { eprintln!("[disasm {:#010x}]\n{}", a, m.disasm(a, n)); }
    { let r = m.reports(); if !r.is_empty() { eprintln!("{}", r); } }
    eprintln!("{}", m.irq_report());
    if let Some(w) = &o.wav { match m.write_wav(w) { Ok(n) => eprintln!("[emu] wrote {} samples ({:.2} s) to {}", n, n as f64 / m.bus.audio().1 as f64, w), Err(e) => eprintln!("[emu] wav: {}", e) } }
    { let r = m.bus.report(); if !r.is_empty() { eprintln!("{}", r); } }
    { let st: Vec<(u64, u64, u64, usize)> = m.cores.iter().filter_map(|c| c.code_cache_stats()).collect();
      if m.vq_stats[0] > 0 { eprintln!("[emu] virtual quanta: runs, quanta, stopped at a device register, at waiti = {:?}", m.vq_stats); }
      if st.iter().any(|s| s.0 > 0) { let b0 = st[0]; let b1 = st.get(1).copied().unwrap_or((0, 0, 0, 0)); eprintln!("[emu] blocks: {} built ({} cache flushes) core0, {} ({}) core1; jit: {} compiled, {} KB code", b0.0, b0.1, b1.0, b1.1, b0.2 + b1.2, (b0.3 + b1.3) / 1024); } }
    if m.stub_hits > 0 { eprintln!("[emu] stubs hit {} times", m.stub_hits); }
    if let Some(p) = &o.tft_png { match m.write_tft_png(p, 3) { Ok(()) => eprintln!("[emu] wrote {}", p), Err(e) => eprintln!("[emu] png: {}", e) } }
    if let Some(p) = &o.gram_png { match m.write_gram_png(p) { Ok(()) => eprintln!("[emu] wrote {}", p), Err(e) => eprintln!("[emu] png: {}", e) } }
    if o.dump { eprintln!("{}", m.dump_regs()); }
}

#[cfg(test)]
mod tests;
