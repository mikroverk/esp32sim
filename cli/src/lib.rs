//! esp32sim — the command line, one front end for every chip (`--chip s3|c3|c6`; the `esp32sim-c3`
//! and `esp32sim-c6` binaries are `--chip c3` / `--chip c6`). Parsing and everything a run does are chip-agnostic over
//! `Machine<S>`; the few flags a chip owns (board, WiFi, camera, PSRAM, register presets) live in
//! its setup function.
use esp_soc::observers::{BlockProfile, Breakpoints, Coverage, IrqLatency, MmioHeat, PcHist, RegTrace, Trace, Vcd, Watch};
use esp_soc::{Machine, Soc, SocBus, Stop};
use emu_core::{Bus, Core};
use std::path::PathBuf;

pub mod cooja;

fn usage(chip: &str) -> ! {
    eprintln!("usage: esp32sim [--chip s3|c3|c6] --boot rom|app --bootloader B.bin --ptable P.bin [--ptable-offset 0xNNNN] --app A.bin [--app-offset 0xNNNN] [--elf X.elf]... [options]");
    eprintln!("       see docs/cli.md for every flag (default chip here: {})", chip);
    std::process::exit(2)
}

fn hex(s: &str, what: &str) -> u32 { u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or_else(|_| { eprintln!("--{}: bad hex {}", what, s); std::process::exit(2) }) }
fn pair(s: &str, dflt: usize) -> (u32, usize) { match s.split_once(',') { Some((a, n)) => (hex(a, "addr"), n.parse().unwrap_or(dflt)), None => (hex(s, "addr"), dflt) } }

/// Everything the command line can say, chip-agnostic; `None` means "the chip's default".
#[derive(Default)]
pub struct Opts {
    pub chip: String,
    pub rom: Option<PathBuf>, pub bootloader: Option<String>, pub ptable: Option<String>, pub ptable_offset: Option<u32>, pub app: Option<String>, pub app_offset: Option<u32>, pub elfs: Vec<String>,
    pub flash_image: Option<String>, pub flash_at: Vec<String>, pub flash_state: Option<String>, pub efuse_state: Option<String>, pub boot: Option<String>, pub flash_mb: Option<usize>, pub psram_mb: Option<usize>,
    pub mac: Option<[u8; 6]>, pub strap: Option<u32>, pub reset_cause: Option<u32>, pub efuse_regs: Option<String>, pub regs_init: Option<String>,
    pub board: String, pub wifi: Option<String>, pub net: String, pub cam_image: Option<String>, pub cam_fps: f64,
    pub max_insns: u64, pub max_seconds: Option<f64>, pub script: Option<String>, pub serial: Option<String>,
    pub console: Option<String>, pub console_prefix: bool, pub realtime: bool, pub uart_tcp: Option<String>, pub web_port: Option<u16>, pub web_dir: Option<String>, pub no_reboot: bool,
    pub wav: Option<String>, pub tft_png: Option<String>, pub gram_png: Option<String>, pub dump: bool,
    pub trace: bool, pub trace_from: u64, pub breaks: Vec<u32>, pub watch: Option<u32>, pub peeks: Vec<(u32, usize)>, pub disasms: Vec<(u32, usize)>,
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
            "--chip" => o.chip = next().to_ascii_lowercase(),
            "--rom" => o.rom = Some(PathBuf::from(next())),
            "--bootloader" => o.bootloader = Some(next()),
            "--ptable" => o.ptable = Some(next()),
            "--ptable-offset" => o.ptable_offset = Some(hex(&next(), "ptable-offset")),
            "--app-offset" => o.app_offset = Some(hex(&next(), "app-offset")),
            "--app" => o.app = Some(next()),
            "--elf" => o.elfs.push(next()),
            "--flash-image" => o.flash_image = Some(next()),
            "--flash-at" => o.flash_at.push(next()),
            "--flash-state" => o.flash_state = Some(next()),
            "--efuse-state" => o.efuse_state = Some(next()),
            "--boot" => o.boot = Some(next()),
            "--flash-mb" => o.flash_mb = Some(next().parse().expect("mb")),
            "--psram-mb" => o.psram_mb = Some(next().parse().expect("mb")),
            "--mac" => { let v = next(); let b: Vec<u8> = v.split(':').filter_map(|x| u8::from_str_radix(x, 16).ok()).collect(); if b.len() != 6 { eprintln!("--mac wants xx:xx:xx:xx:xx:xx"); std::process::exit(2); } let mut m = [0u8; 6]; m.copy_from_slice(&b); o.mac = Some(m); }
            "--strap" => o.strap = Some(hex(&next(), "strap")),
            "--reset-cause" => o.reset_cause = Some(hex(&next(), "reset-cause")),
            "--efuse-regs" => o.efuse_regs = Some(next()),
            "--regs-init" => o.regs_init = Some(next()),
            "--board" => o.board = next(),
            "--wifi" => o.wifi = Some(next()),
            "--net" => o.net = next(),
            "--cam-image" => o.cam_image = Some(next()),
            "--cam-fps" => o.cam_fps = next().parse().expect("fps"),
            "--max-insns" => o.max_insns = next().replace('_', "").parse().expect("max-insns"),
            "--max-seconds" => o.max_seconds = Some(next().parse().expect("seconds")),
            "--script" => o.script = Some(next()),
            "--serial" => o.serial = Some(next()),
            "--console" => o.console = Some(next()),
            "--console-prefix" => o.console_prefix = true,
            "--realtime" => o.realtime = true,
            "--uart-tcp" => o.uart_tcp = Some(next()),
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
    o
}

/// `~/.espressif/tools/esp-rom-elfs/*/<name>` (the newest release wins).
fn find_rom(name: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(format!("{}/.espressif/tools/esp-rom-elfs", home)).ok()?.flatten().map(|e| e.path()).collect();
    dirs.sort();
    dirs.into_iter().rev().map(|d| d.join(name)).find(|p| p.exists())
}

pub fn run_cli(default_chip: &str) {
    let args: Vec<String> = std::env::args().collect();
    let mut o = parse(&args, default_chip);
    if o.cooja { return run_cooja(&mut o); }
    match o.chip.as_str() {
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
    if o.uart_tcp.is_some() { eprintln!("--uart-tcp cannot be combined with --cooja"); std::process::exit(2); }
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let hello = match cooja::read_hello(&mut input) { Ok(h) => h, Err(e) => { eprintln!("[cooja] {}", e); std::process::exit(2) } };
    if o.mac.is_none() { o.mac = Some(cooja::mac_for_node(hello.id)); }
    if o.console.is_none() { o.console = Some("uart0".into()); }
    let mut m = setup_c6(o);
    let boot = prepare(&mut m, o);
    let cfg = cooja::Config {
        slice_ns: o.cooja_slice_us.max(1) * 1000,
        console_mask: match o.console.as_deref().unwrap_or("uart0") { "usb" => 1, "uart0" | "uart" => 2, "both" => 3, "all" => 7, "none" => 0, _ => 2 },
        rx_on_air: !o.cooja_rx_at_end,
        verbose: o.cooja_verbose,
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
    let mut m = esp32s3::machine(o.mac.unwrap_or([0x44, 0x1b, 0xf6, 0x75, 0xdc, 0xe0]));
    m.bus.board = esp32s3::board::make_board(&o.board).unwrap_or_else(|| { eprintln!("unknown board '{}' (atech14, waveshare-cam, waveshare-lcd4b, waveshare-amoled18-v2, none)", o.board); std::process::exit(2) });
    m.bus.attach_board_devices();
    if !o.debug.is_empty() { let mut f = esp_soc::DebugFlags::from_env(); for d in &o.debug { f.parse(d); } m.set_debug(&f); }
    if let Some(spec) = &o.wifi {
        let mut cfg = esp32s3::wifi::ApConfig { ssid: "esp32sim".into(), bssid: [0x02, 0x53, 0x49, 0x4d, 0x00, 0x01], channel: 6, psk: None };
        for kv in spec.split(',') {
            match kv.split_once('=') {
                Some(("ssid", v)) => cfg.ssid = v.to_string(),
                Some(("chan", v)) | Some(("channel", v)) => cfg.channel = v.parse().unwrap_or(6),
                Some(("psk", v)) | Some(("password", v)) => cfg.psk = Some(v.to_string()),
                Some(("bssid", v)) => { let b: Vec<u8> = v.split(':').filter_map(|x| u8::from_str_radix(x, 16).ok()).collect(); if b.len() == 6 { cfg.bssid.copy_from_slice(&b); } }
                _ => {}
            }
        }
        eprintln!("[emu] virtual AP '{}' bssid {} channel {} ({})", cfg.ssid, esp32s3::wifi::mac_str(&cfg.bssid), cfg.channel, if cfg.psk.is_some() { "WPA2-PSK" } else { "open" });
        m.bus.periph.wifi.ap = Some(esp32s3::wifi::VirtualAp::new(cfg, m.bus.debug.has("wifi-frames")));
        let mut net = esp32s3::net::VirtualNet::new(m.bus.debug.has("net"));
        if o.net == "nat" || o.net == "user" {
            let nat = esp32s3::nat::Nat::new(m.bus.debug.has("net"));
            eprintln!("[emu] NAT to the host network enabled (DNS via {}.{}.{}.{})", nat.resolver[0], nat.resolver[1], nat.resolver[2], nat.resolver[3]);
            net.nat = Some(nat);
        }
        eprintln!("[emu] virtual network: station {}.{}.{}.{}, gateway {}.{}.{}.{} (DHCP, ARP, ICMP, DNS, NTP)", net.sta_ip[0], net.sta_ip[1], net.sta_ip[2], net.sta_ip[3], net.gw_ip[0], net.gw_ip[1], net.gw_ip[2], net.gw_ip[3]);
        m.bus.periph.wifi.net = Some(net);
    }
    if let Some(p) = &o.cam_image { match esp_soc::picture::load(p) { Ok(pic) => { eprintln!("[emu] camera picture {} ({}x{})", p, pic.w, pic.h); m.bus.board.set_camera_picture(pic); } Err(e) => { eprintln!("[emu] {}", e); std::process::exit(2); } } }
    m.bus.periph.lcd_cam.frame_cycles = (esp32s3::periph::CPU_HZ as f64 / o.cam_fps) as u64;
    if let Some(mb) = o.flash_mb { if mb != 8 { m.bus.set_flash_size(mb << 20); } }
    if let Some(mb) = o.psram_mb { if mb != 2 { m.bus.set_psram_size(mb << 20).unwrap(); } }
    if let Some(path) = &o.flash_state {
        let loaded = m.bus.configure_flash_state(path).unwrap_or_else(|e| { eprintln!("--flash-state: {}", e); std::process::exit(2) });
        eprintln!("[emu] flash state: {} ({})", path, if loaded { "loaded" } else { "new" });
    }
    if let Some(path) = &o.efuse_state {
        let loaded = m.bus.configure_efuse_state(path).unwrap_or_else(|e| { eprintln!("--efuse-state: {}", e); std::process::exit(2) });
        eprintln!("[emu] eFuse state: {} ({})", path, if loaded { "loaded" } else { "new" });
    }
    if let Some(p) = &o.efuse_regs {
        if m.bus.persistent_efuse_loaded() { eprintln!("[emu] existing eFuse state takes precedence over --efuse-regs"); }
        let txt = std::fs::read_to_string(p).expect("efuse file");
        let mut n = 0;
        for line in txt.lines() {
            let line = line.trim(); if line.is_empty() { continue; }
            let (addr_s, rest) = match line.split_once(':') { Some(x) => x, None => continue };
            let Ok(mut a) = u32::from_str_radix(addr_s.trim().trim_start_matches("0x"), 16) else { continue };
            for w in rest.split_whitespace() { if let Ok(v) = u32::from_str_radix(w, 16) { let off = if a >= 0x6000_7000 { a - 0x6000_7000 } else { a }; if !m.bus.persistent_efuse_loaded() && !m.bus.periph.efuse.import_shadow(off, v) { m.bus.periph.efuse.ram.write(off, v); } a += 4; n += 1; } }
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

fn setup_c3(o: &Opts) -> esp32c3::Machine {
    let mut m = esp32c3::machine(o.mac.unwrap_or([0x60, 0x55, 0xf9, 0x00, 0x11, 0x22]), o.flash_mb.unwrap_or(4) << 20);
    m.bus.set_flash_size(o.flash_mb.unwrap_or(4) << 20);   // the JEDEC capacity follows the size
    if !o.debug.is_empty() { let mut f = esp_soc::DebugFlags::from_env(); for d in &o.debug { f.parse(d); } m.set_debug(&f); }
    for (flag, on) in [("--board", o.board != "atech14" && o.board != "none"), ("--wifi", o.wifi.is_some()), ("--cam-image", o.cam_image.is_some()), ("--psram-mb", o.psram_mb.is_some()), ("--efuse-regs", o.efuse_regs.is_some()), ("--efuse-state", o.efuse_state.is_some()), ("--flash-state", o.flash_state.is_some()), ("--regs-init", o.regs_init.is_some()), ("--regstat", o.regstat.is_some())] {
        if on { eprintln!("{} is not available on the C3", flag); std::process::exit(2); }
    }
    m
}

fn setup_c6(o: &Opts) -> esp32c6::Machine {
    let mut m = esp32c6::machine(o.mac.unwrap_or([0xdc, 0x1e, 0xd5, 0x6e, 0x8c, 0xdc]), o.flash_mb.unwrap_or(4) << 20);
    m.bus.set_flash_size(o.flash_mb.unwrap_or(4) << 20);   // the JEDEC capacity follows the size
    if !o.debug.is_empty() { let mut f = esp_soc::DebugFlags::from_env(); for d in &o.debug { f.parse(d); } m.set_debug(&f); }
    let name = if o.board == "atech14" { "none" } else { o.board.as_str() };   // the S3 default means "bare module" here
    match esp32c6::board::make_board(name) { Some(b) => m.bus.board = b, None => { eprintln!("--board {}: none or waveshare-c6-lcd147 on the C6", name); std::process::exit(2) } }
    for (flag, on) in [("--wifi", o.wifi.is_some()), ("--cam-image", o.cam_image.is_some()), ("--psram-mb", o.psram_mb.is_some()), ("--efuse-regs", o.efuse_regs.is_some()), ("--efuse-state", o.efuse_state.is_some()), ("--flash-state", o.flash_state.is_some()), ("--regs-init", o.regs_init.is_some()), ("--regstat", o.regstat.is_some())] {
        if on { eprintln!("{} is not available on the C6", flag); std::process::exit(2); }
    }
    m
}

/// Everything after the chip is set up: images, boot, observers, the run, the reports.
fn run<S: Soc>(mut m: Machine<S>, o: &Opts) {
    let boot = prepare(&mut m, o);
    m.bus.initialize_storage().unwrap_or_else(|e| { eprintln!("[emu] storage: {}", e); std::process::exit(2) });
    let t0 = std::time::Instant::now();
    let stop = loop {
        let stop = m.run(o.max_insns);
        if let Stop::SwReset = stop {
            m.bus.flush_storage().unwrap_or_else(|e| { eprintln!("[emu] storage: {}", e); std::process::exit(1) });
            let cause = m.bus.reset_cause();
            eprintln!("[emu] chip reset at t={:.3}s: cause {:#x} ({})", m.seconds(), cause, esp_periph::reset_cause_name(cause));
            if o.no_reboot || boot != "rom" { break stop; }
            m.reboot();
            continue;
        }
        break stop;
    };
    m.bus.flush_storage().unwrap_or_else(|e| { eprintln!("[emu] storage: {}", e); std::process::exit(1) });
    let dt = t0.elapsed().as_secs_f64();
    report(&mut m, o, stop, dt);
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
    if m.bus.persistent_flash_loaded() {
        if o.flash_image.is_some() || o.bootloader.is_some() || o.ptable.is_some() || o.app.is_some() || !o.flash_at.is_empty() { eprintln!("[emu] existing flash state takes precedence over seed images"); }
    } else {
        if let Some(p) = &o.flash_image { m.write_flash(0, &std::fs::read(p).expect("flash image")).unwrap(); }
        if let Some(p) = &o.bootloader { m.write_flash(0x0, &std::fs::read(p).expect("bootloader")).unwrap(); }
        if let Some(p) = &o.ptable { m.write_flash(o.ptable_offset.unwrap_or(0x8000) as usize, &std::fs::read(p).expect("ptable")).unwrap(); }
        if let Some(p) = &o.app { m.write_flash(o.app_offset.unwrap_or(0x10000) as usize, &std::fs::read(p).expect("app")).unwrap(); }
        for spec in &o.flash_at {
            let (off, path) = spec.split_once('=').unwrap_or_else(|| { eprintln!("--flash-at needs OFFSET=FILE"); std::process::exit(2) });
            let off = usize::from_str_radix(off.trim_start_matches("0x"), 16).unwrap_or_else(|_| { eprintln!("--flash-at: bad offset {}", off); std::process::exit(2) });
            let data = std::fs::read(path).unwrap_or_else(|e| { eprintln!("--flash-at: {}: {}", path, e); std::process::exit(2) });
            m.write_flash(off, &data).unwrap_or_else(|e| { eprintln!("--flash-at: {}", e); std::process::exit(2) });
            eprintln!("[emu] flash {:#x}: {} ({} bytes)", off, path, data.len());
        }
    }
    for p in &o.elfs { m.add_symbols(&std::fs::read(p).expect("elf")).expect("elf symbols"); }
    if let Some(s) = &o.serial { m.bus.serial_input(s.as_bytes()); }
    for pre in &o.trace_fns {
        let mut n = 0;
        for (&a, name) in m.symbols.clone().iter() { if name.starts_with(pre.as_str()) || (pre.ends_with('$') && name == &pre[..pre.len() - 1]) { m.fn_probes.insert(a, name.clone()); n += 1; } }
        eprintln!("[emu] --trace-fn {}: {} functions", pre, n);
    }
    for st in &o.stubs {
        let (name, val) = match st.split_once('=') { Some((n, v)) => (n, u32::from_str_radix(v.trim_start_matches("0x"), if v.starts_with("0x") { 16 } else { 10 }).unwrap_or(0)), None => (st.as_str(), 0) };
        let addr = if let Some(a) = m.sym_addr(name) { a } else if let Ok(a) = u32::from_str_radix(name.trim_start_matches("0x"), 16) { a } else { eprintln!("--stub: unknown symbol {}", name); std::process::exit(2) };
        eprintln!("[emu] stub {} @ {:#010x} -> returns {:#x}", name, addr, val);
        m.stubs.insert(addr, val);
    }
    if o.no_jit { for c in &mut m.cores { c.set_jit(false); } }
    match boot.as_str() {
        "app" => match m.boot_app(o.app_offset.unwrap_or(0x10000) as usize) { Ok(entry) => eprintln!("[emu] app boot: entry {:#010x} {}", entry, m.sym(entry)), Err(e) => { eprintln!("[emu] {}", e); std::process::exit(2) } },
        "rom" => { m.boot_rom(); eprintln!("[emu] ROM boot from reset vector {:#010x}", m.cores[0].pc()); }
        _ => { eprintln!("--boot app|rom"); std::process::exit(2); }
    }
    // Match a real board's boot conditions: the ROM prints the reset cause and the strapping-derived boot mode
    if let Some(c) = o.reset_cause { m.bus.set_reset_cause(c); }
    if let Some(v) = o.strap { m.bus.set_strap(v); }
    for &(a, n) in &o.peeks { eprintln!("[peek before run]\n{}", m.peek(a, n)); }
    m.dbg.stop_after_exceptions = o.stop_exc;
    m.console.mask = match console.as_str() { "usb" => 1, "uart0" => 2, "uart" => 2, "both" => 3, "all" => 7, "none" => 0, _ => 3 };
    m.console.prefix = o.console_prefix;
    if let Some(addr) = &o.uart_tcp {
        let uart_tcp = esp_soc::UartTcp::start(addr).unwrap_or_else(|e| { eprintln!("--uart-tcp {}: {}", addr, e); std::process::exit(2) });
        eprintln!("[emu] UART0 TCP: {}", uart_tcp.local_addr());
        m.uart_tcp = Some(uart_tcp);
        m.console.mask &= !2;       // UART0 belongs to the binary socket while the bridge is active
        m.rt.enabled = true;        // keep ROM receive timeouts synchronized with the host
    }
    if let Some(p) = &o.regtrace { m.add_observer(Box::new(RegTrace::new(std::fs::File::create(p).expect("regtrace file"), o.regtrace_max, o.regtrace_from_pc))); }
    if let Some(port) = o.web_port {
        let dir = o.web_dir.clone().unwrap_or_else(|| { let exe = std::env::current_exe().unwrap(); let mut d = exe.parent().unwrap().to_path_buf(); for _ in 0..3 { if d.join("web").exists() { break; } d = d.parent().unwrap().to_path_buf(); } d.join("web").to_string_lossy().to_string() });
        let w = esp_soc::web::WebServer::start(port, dir.clone()).expect("web server");
        eprintln!("[emu] board UI: http://127.0.0.1:{}/  (serving {})", port, dir);
        m.web = Some(w); m.rt.enabled = true;
    }
    if o.realtime { m.rt.enabled = true; }
    if o.profile { m.add_observer(Box::new(PcHist::new(12))); }
    if let Some(wa) = o.watch { let v = m.bus.read32(wa).unwrap_or(0); m.add_observer(Box::new(Watch { addr: wa, value: v })); }
    if o.profile_blocks { m.add_observer(Box::new(BlockProfile::new(20))); }
    if let Some(path) = &o.coverage { m.add_observer(Box::new(Coverage::new(path.clone()))); }
    if o.irq_latency { m.add_observer(Box::new(IrqLatency::new(S::CORES))); }
    if let Some(p) = &o.vcd { m.add_observer(Box::new(Vcd::new(p, S::CPU_HZ))); }
    if let Some(p) = &o.script { m.load_script(&std::fs::read_to_string(p).expect("script")).expect("script"); }
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
    for &(a, n) in &o.disasms { eprintln!("[disasm {:#010x}]\n{}", a, m.disasm(a, n)); }
    { let r = m.reports(); if !r.is_empty() { eprintln!("{}", r); } }
    eprintln!("{}", m.irq_report());
    if let Some(w) = &o.wav { match m.write_wav(w) { Ok(n) => eprintln!("[emu] wrote {} samples ({:.2} s) to {}", n, n as f64 / m.bus.audio().1 as f64, w), Err(e) => eprintln!("[emu] wav: {}", e) } }
    { let r = m.bus.report(); if !r.is_empty() { eprintln!("{}", r); } }
    { let st: Vec<(u64, u64, u64, usize)> = m.cores.iter().filter_map(|c| c.code_cache_stats()).collect();
      if st.iter().any(|s| s.0 > 0) { let b0 = st[0]; let b1 = st.get(1).copied().unwrap_or((0, 0, 0, 0)); eprintln!("[emu] blocks: {} built ({} cache flushes) core0, {} ({}) core1; jit: {} compiled, {} KB code", b0.0, b0.1, b1.0, b1.1, b0.2 + b1.2, (b0.3 + b1.3) / 1024); } }
    if m.stub_hits > 0 { eprintln!("[emu] stubs hit {} times", m.stub_hits); }
    if let Some(p) = &o.tft_png { match m.write_tft_png(p, 3) { Ok(()) => eprintln!("[emu] wrote {}", p), Err(e) => eprintln!("[emu] png: {}", e) } }
    if let Some(p) = &o.gram_png { match m.write_gram_png(p) { Ok(()) => eprintln!("[emu] wrote {}", p), Err(e) => eprintln!("[emu] png: {}", e) } }
    if o.dump { eprintln!("{}", m.dump_regs()); }
}
