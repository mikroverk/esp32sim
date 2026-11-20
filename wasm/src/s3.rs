//! ESP32-S3 capabilities exposed by the browser ABI.
use super::{Emu, log, bytes};

// Keep user-supplied per-event prices bounded well below the u32 timing accumulator.
const MAX_TIMING_PRICE: u32 = 1_000_000;

/// Attach the virtual access point and subnet: `ssid=NAME,psk=PASS,chan=N`. No NAT — the browser
/// has no sockets — so DHCP, DNS, SNTP and ICMP answer, and connections past the gateway are refused,
/// until `esp32sim_nat_relay` hands them to the page. Each further line is another joinable access
/// point on the same network, as the CLI's repeated `--wifi` (at most 8; a later one without a
/// `bssid` gets one derived from its SSID; no two share an SSID or a BSSID). Returns 0, or 1 with
/// the reason on `host_log` and nothing attached when the text is not UTF-8 or the CLI would refuse
/// the list.
///
/// # Safety
/// `e` must point to a live emulator to which the caller has exclusive access. For nonzero `len`,
/// `spec` must be non-null and readable for `len` bytes throughout this call.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_wifi(e: *mut Emu, spec: *const u8, len: usize) -> u32 {
    // SAFETY: The caller provides exclusive access to a live emulator.
    let e = unsafe { &mut *e };
    // SAFETY: The caller provides a readable setup string for this call.
    let spec = match std::str::from_utf8(unsafe { bytes(spec, len) }) {
        Ok(spec) => spec,
        Err(_) => { log("[emu] wifi: configuration is not UTF-8"); return 1; }
    };
    // one spec per line; no lines is the one default access point, as an empty spec always was
    let mut specs: Vec<String> = spec.lines().map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
    if specs.is_empty() { specs.push(String::new()); }
    let cfgs = match esp_soc::wifi::ApConfig::parse_all(&specs) {
        Ok(cfgs) => cfgs,
        Err(reason) => { log(&format!("[emu] wifi: {reason}")); return 1; }
    };
    let mut cfgs = cfgs.into_iter();
    let Some(first) = cfgs.next() else { return 1; };
    log(&format!("[emu] virtual AP '{}' ({}), subnet 10.0.2.0/24, no NAT in the browser", first.ssid, if first.psk.is_some() { "WPA2-PSK" } else { "open" }));
    // the same access point and network behind every chip's MAC; no NAT in the browser
    super::nat::end_relay(e);   // a new network has no relay until esp32sim_nat_relay
    if let Err(reason) = e.m.attach_wifi(first) { log(&format!("[emu] wifi: {reason}")); return 1; }
    for cfg in cfgs {
        log(&format!("[emu] virtual AP '{}' bssid {} channel {} ({}), the same network", cfg.ssid, esp_soc::wifi::mac_str(&cfg.bssid), cfg.channel, if cfg.psk.is_some() { "WPA2-PSK" } else { "open" }));
        if let Err(reason) = e.m.add_wifi_ap(cfg) { log(&format!("[emu] wifi: {reason}")); return 1; }
    }
    0
}

/// The page's Network panel (`web/netmon.js`), as the CLI's `--web` gives it with `--wifi`: from
/// now on every Ethernet frame the WiFi station sends and receives goes to the page as a binary
/// message of type 5 (`[5][dir: 0 from the station, 1 to it][emulated µs, u64 le][frame]`), after
/// one `{"t":"netmon","linktype":1}` text message. On the P4 the station is its co-processor's C6.
/// Before or after boot, after `esp32sim_wifi`; a second call changes nothing. Returns 0, or 1 for a
/// chip without WiFi, with the reason on `host_log`.
///
/// # Safety
/// `e` must point to a live emulator to which the caller has exclusive access.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_netmon(e: *mut Emu) -> u32 {
    // SAFETY: The caller provides exclusive access to a live emulator.
    let e = unsafe { &mut *e };
    if e.netmon { return 0; }
    let mut capture = esp_soc::netcap::NetCapture::default();
    capture.enable_live();
    let tap = std::sync::Arc::new(std::sync::Mutex::new(capture));
    if let Err(reason) = e.m.net_monitor(tap) { log(&format!("[emu] network monitor: {reason}")); return 1; }
    e.netmon = true;
    if let Some(w) = e.m.web() { w.send_text("{\"t\":\"netmon\",\"linktype\":1}"); }
    0
}

/// Set the networks around the station (the CLI's `--wifi-nearby`), replacing the ones there
/// were: one spec per line, as the option takes it (`ssid=`, `chan=`, `rssi=`, `secure=`,
/// `bssid=`), at most 32; no lines for none. A scan lists them and nothing can join them. Needs
/// the access point of `esp32sim_wifi`, which a new `esp32sim_wifi` replaces with none around;
/// before or after boot. A network with the access point's SSID or BSSID is left out, with the
/// reason on `host_log`, as the option leaves it out. Returns 0, or 1 for text that is not UTF-8,
/// a spec or list the option refuses, or no access point, with the reason on `host_log` and the
/// networks unchanged.
///
/// # Safety
/// `e` must point to a live emulator to which the caller has exclusive access. For nonzero `len`,
/// `specs` must be non-null and readable for `len` bytes throughout this call.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_wifi_nearby(e: *mut Emu, specs: *const u8, len: usize) -> u32 {
    // SAFETY: The caller provides exclusive access to a live emulator.
    let e = unsafe { &mut *e };
    // SAFETY: The caller provides a readable spec buffer for this call.
    let Ok(specs) = std::str::from_utf8(unsafe { bytes(specs, len) }) else {
        log("[emu] wifi nearby: the specs are not UTF-8");
        return 1;
    };
    let specs: Vec<String> = specs.lines().map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
    let list = match esp_soc::wifi::BeaconAp::parse_all(&specs) {
        Ok(list) => list,
        Err(reason) => { log(&format!("[emu] wifi nearby: {reason}")); return 1; }
    };
    match e.m.wifi_nearby(list) {
        Ok(left_out) => {
            for (b, why) in left_out { log(&format!("[emu] wifi nearby: '{}' left out: it has {why}", b.ssid)); }
            0
        }
        Err(reason) => { log(&format!("[emu] wifi nearby: {reason}")); 1 }
    }
}

/// Enable provisional per-instruction timing before ROM boot. Returns 1 on rejection.
/// This selects the modeled interpreter, not the production browser JIT.
/// # Safety
/// `e` must point to a live emulator to which the caller has exclusive access.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_set_approximate_timing(e: *mut Emu) -> u32 {
    let e = unsafe { &mut *e };
    let Some(m) = e.m.s3_mut() else { return 1; };
    match m.set_cost_model(Box::new(esp32s3::ApproximateCostModel::default())) {
        Ok(()) => { log("[emu] APPROXIMATE timing enabled; accuracy unvalidated; ROM boot required"); 0 }
        Err(reason) => { log(&format!("[emu] approximate timing: {reason}")); 1 }
    }
}

/// Configure experimental SPI2 wire timing before boot. Returns 1 for unsupported chips or after boot.
///
/// # Safety
/// `e` must be a live exclusively borrowed emulator.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_set_spi2_timing(e: *mut Emu, enabled: u32) -> u32 {
    let e = unsafe { &mut *e };
    if e.booted { return 1; }
    let Some(m) = e.m.s3_mut() else { return 1 };
    m.bus.spi2_timing = enabled != 0;
    0
}

/// Select the measured CO5300 TE waveform before boot. Returns 1 for other boards or after boot.
///
/// # Safety
/// `e` must be a live exclusively borrowed emulator.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_set_measured_te(e: *mut Emu, enabled: u32) -> u32 {
    let e = unsafe { &mut *e };
    if e.booted { return 1; }
    let Some(m) = e.m.s3_mut() else { return 1 };
    if m.bus.board.name() != "waveshare-amoled18-v2" { return 1; }
    // A new board must keep the IMU motion selected before it.
    let motion = m.bus.board.imu_motion();
    m.bus.board = Box::new(if enabled != 0 {
        esp32s3::board::WaveshareAmoled18V2::with_measured_te()
    } else {
        esp32s3::board::WaveshareAmoled18V2::new()
    });
    m.bus.board.set_imu_motion(motion);
    m.bus.attach_board_devices();
    0
}

/// Select independent per-core completion times: 1 batches blocks, 2 also exits on first cache miss.
/// # Safety
/// `e` must be a live exclusively borrowed emulator.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_set_approximate_jit_frontiers(e: *mut Emu, enabled: u32) -> u32 {
    if enabled > 2 { return 1; }
    let e = unsafe { &mut *e };
    e.m.s3_mut()
        .map(|m| {
            if m.set_approximate_jit_frontiers(enabled != 0).is_err() { return 1; }
            m.bus.set_approximate_cache_yield_miss(enabled >= 2);
            0
        }).unwrap_or(1)
}

/// Configure the rough shared data cache after approximate JIT timing, before execution.
/// `fast_internal`: 0 uses helpers, 1 retains direct SRAM, 2 selects feature-gated inline
/// cache hits (32 KB), 3 selects inline hits with 64 KB. Other values are rejected.
/// Reconfiguration clears cache contents/counters and resets common service to `fill`
/// and flash overrides to their defaults. Set custom service/flash timing afterward.
/// # Safety
/// `e` must be a live exclusively borrowed emulator.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_set_approximate_jit_cache(e: *mut Emu, fill: u32, writeback: u32, fast_internal: u32) -> u32 {
    let e = unsafe { &mut *e };
    let Some(m) = e.m.s3_mut() else { return 1 };
    if m.insns() != 0 || !m.has_approximate_jit_timing() || fast_internal > 3 || fill > MAX_TIMING_PRICE || writeback > MAX_TIMING_PRICE { return 1; }
    if fast_internal >= 2 && !cfg!(all(target_arch = "wasm32", feature = "cache-inline")) { return 1; }
    // `fast_internal` values 2 and 3 both select the inline probe; 3 also selects the 64 KB data
    // cache some firmware configures (EXTMEM_DCACHE_CTRL size mode 1; pocket-tank does).
    let capacity_bytes = if fast_internal == 3 { 65536 } else { 32768 };
    let fast_internal = fast_internal.min(2);
    m.bus.enable_approximate_cache(esp32s3::approximate_cache::CacheConfig { fill_cycles: fill, writeback_cycles: writeback, capacity_bytes, ..Default::default() });
    #[cfg(all(target_arch = "wasm32", feature = "cache-inline"))]
    xtensa_lx7::jit::CACHE_SET_MASK.store(capacity_bytes as u32 / (64 * 8) - 1, std::sync::atomic::Ordering::Relaxed);
    m.bus.set_approximate_cache_fast_internal(fast_internal != 0);
    #[cfg(all(target_arch = "wasm32", feature = "cache-inline"))]
    xtensa_lx7::jit::CACHE_PROBES.store(false, std::sync::atomic::Ordering::Relaxed);
    if fast_internal == 2 {
        if !m.bus.set_approximate_cache_inline() { return 1; }
        #[cfg(all(target_arch = "wasm32", feature = "cache-inline"))]
        xtensa_lx7::jit::CACHE_PROBES.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    0
}

/// Select an uncalibrated PIE instruction-cost hypothesis before execution.
/// Nonzero modes require approximate JIT timing; zero disables PIE prices.
/// # Safety
/// `e` must be a live exclusively borrowed emulator, before execution.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_set_approximate_pie_timing(e: *mut Emu, mode: u32) -> u32 {
    let e = unsafe { &mut *e };
    let Some(m) = e.m.s3_mut() else { return 1 };
    if m.insns() != 0 || mode > 2 || (mode != 0 && !m.has_approximate_jit_timing()) { return 1; }
    for cpu in &mut m.cores { cpu.approximate_pie_mode = mode; }
    0
}

/// EX138: charge the measured control-flow prices (taken branch 3, J 3, JX 6, LOOP 5, QUO 4,
/// REM 5; calls and returns derived) on top of one cycle per instruction. Before execution only.
/// Enabling prices requires approximate JIT timing.
/// # Safety
/// `e` must be a live exclusively borrowed emulator.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_set_control_prices(e: *mut Emu, on: u32) -> u32 {
    let e = unsafe { &mut *e };
    let Some(m) = e.m.s3_mut() else { return 1 };
    if m.insns() != 0 || (on != 0 && !m.has_approximate_jit_timing()) { return 1; }
    for cpu in &mut m.cores { cpu.price_control = on != 0; }
    #[cfg(target_arch = "wasm32")]
    xtensa_lx7::jit::PRICED.store(on != 0, std::sync::atomic::Ordering::Relaxed);
    0
}

/// Scheduling quantum of the exact-clock scheduler (default 256 on wasm32, 64 native). Other values interleave two
/// busy cores differently: deterministic, but not bit-identical with the default.
/// Rejected while either timing model owns scheduling; approximate timing uses its own quantum.
/// # Safety
/// `e` must be a live exclusively borrowed emulator.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_set_quantum(e: *mut Emu, instructions: u32) -> u32 {
    let e = unsafe { &mut *e };
    let Some(m) = e.m.s3_mut() else { return 1 };
    if m.insns() != 0 || m.has_timing_model() || !(64..=4096).contains(&instructions) || !instructions.is_multiple_of(64) { return 1; }
    m.quantum = u64::from(instructions);
    0
}

/// EX177 (diagnostic): most whole scheduling rounds one both-busy batch may run (1 = off).
/// The batch keeps the per-core quanta, their order and their budgets, so it is bit-exact with
/// the per-round schedule; `ESP32SIM_BB_BUILD=<n>` pins a build's default.
/// # Safety
/// `e` must be a live exclusively borrowed emulator.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_set_round_batch(e: *mut Emu, rounds: u32) -> u32 {
    let e = unsafe { &mut *e };
    let Some(m) = e.m.s3_mut() else { return 1 };
    if rounds == 0 || rounds > 4096 { return 1; }
    m.bb_max = u64::from(rounds);
    0
}

/// EX177 counters: batches, whole rounds covered, device-register deferrals (batch-s1: the batch
/// continues past them), cuts at waiti, batches that ran the whole cap, granted rounds, batches the bound refused,
/// (lane-s2b) memo'd starts the batch ran without `step_blocks`.
/// # Safety
/// `e` must be a live exclusively borrowed emulator.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_round_batch_stat(e: *mut Emu, i: u32) -> f64 {
    let e = unsafe { &mut *e };
    e.m.s3_mut().and_then(|m| m.bb_stats.get(i as usize).copied()).unwrap_or(0) as f64
}

/// EX147 (diagnostic): instruction-fetch cache fill price for flash-mapped code, per 32-byte line.
/// Nonzero prices require approximate JIT timing; zero disables fetch prices.
/// # Safety
/// `e` must be a live exclusively borrowed emulator.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_set_icache_fill(e: *mut Emu, cycles: u32) -> u32 {
    let e = unsafe { &mut *e };
    let Some(m) = e.m.s3_mut() else { return 1 };
    if m.insns() != 0 || cycles > MAX_TIMING_PRICE || (cycles != 0 && !m.has_approximate_jit_timing()) { return 1; }
    m.cores[0].fetch_cache.reset();
    for cpu in &mut m.cores { cpu.icache_fill = cycles; }
    #[cfg(target_arch = "wasm32")]
    xtensa_lx7::jit::FETCH_RING.store(cycles != 0, std::sync::atomic::Ordering::Relaxed);
    0
}

/// Provisional PIE counts: charged events (0), additional cycles (1).
/// # Safety
/// `e` must be a live exclusively borrowed emulator.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_approximate_pie_counter(e: *mut Emu, counter: u32) -> f64 {
    let e = unsafe { &mut *e };
    e.m.s3_mut()
        .map(|m| m.cores.iter().map(|c| if counter == 0 { c.approximate_pie_events } else { c.approximate_pie_cycles }).sum::<u64>() as f64).unwrap_or(0.0)
}

/// Configure shared memory contention after configuring cache, before execution.
/// # Safety
/// `e` must be a live exclusively borrowed emulator, before execution begins.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_set_approximate_cache_contention(e: *mut Emu, enabled: u32) -> u32 {
    let e = unsafe { &mut *e };
    let Some(m) = e.m.s3_mut() else { return 1 };
    if m.insns() != 0 || m.bus.approximate_cache_stats().is_none() { return 1; }
    m.bus.set_approximate_cache_contention(enabled != 0);
    0
}

/// Set full refill service separately from requested-data readiness, after configuring cache.
/// For example, readiness 96 and service 160 still charge 96 for an uncontended first miss;
/// the remaining service time affects later requests, not that miss's readiness.
/// # Safety
/// `e` must be a live exclusively borrowed emulator, configured before execution.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_set_approximate_cache_fill_service(e: *mut Emu, cycles: u32) -> u32 {
    let e = unsafe { &mut *e };
    let Some(m) = e.m.s3_mut() else { return 1 };
    if m.insns() != 0 || cycles > MAX_TIMING_PRICE || m.bus.approximate_cache_stats().is_none() { return 1; }
    u32::from(!m.bus.set_approximate_cache_fill_service(cycles))
}

/// Override flash demand readiness and service; common cache timing still applies to PSRAM.
/// Configure the cache first. This optional probe does not select prices by default.
/// # Safety
/// `e` must be a live exclusively borrowed emulator, configured before execution.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_set_approximate_flash_timing(e: *mut Emu, ready: u32, service: u32) -> u32 {
    let e = unsafe { &mut *e };
    let Some(m) = e.m.s3_mut() else { return 1 };
    if m.insns() != 0 || ready > MAX_TIMING_PRICE || service > MAX_TIMING_PRICE { return 1; }
    u32::from(!m.bus.set_approximate_flash_timing(ready, service))
}

/// Per-core queued wait in provisional shared memory service.
/// Returns NaN for a core other than 0/1 or an unsupported chip.
/// # Safety
/// `e` must be a live exclusively borrowed emulator.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_approximate_cache_wait(e: *mut Emu, core: u32) -> f64 {
    let e = unsafe { &mut *e };
    if core > 1 { return f64::NAN; }
    e.m.s3_mut()
        .map(|m| m.bus.approximate_cache_wait_cycles()[core as usize] as f64).unwrap_or(f64::NAN)
}

/// Cache experiment counters: hits, line fills, writebacks and extra cycles.
/// # Safety
/// `e` must be a live exclusively borrowed emulator.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_approximate_cache_counter(e: *mut Emu, counter: u32) -> f64 {
    let e = unsafe { &mut *e };
    e.m.s3_mut().and_then(|m| m.bus.approximate_cache_stats())
        .map(|s| match counter { 0 => s.hits, 1 => s.line_fills, 2 => s.dirty_writebacks, _ => s.extra_cycles } as f64).unwrap_or(0.0)
}

/// Guest instructions retired by compiled blocks, including interpreter helpers.
///
/// # Safety
/// `e` must be a live exclusively borrowed emulator.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_block_jit_insns(e: *mut Emu) -> f64 {
    // SAFETY: The ABI caller guarantees a live exclusive handle.
    let e = unsafe { &mut *e };
    e.m.s3_mut()
        .map(|m| m.cores.iter().map(|c| c.blocks.jit_instructions).sum::<u64>() as f64)
        .unwrap_or(0.0)
}

/// Emit the optional statistical block profile through host_log.
///
/// # Safety
/// `e` must be a live exclusively borrowed emulator.
#[cfg(all(target_arch = "wasm32", feature = "jit-profile"))]
#[no_mangle]
pub unsafe extern "C" fn esp32sim_profile_report(e: *mut Emu) {
    // SAFETY: the ABI caller guarantees a live exclusive handle.
    if let Some(m) = unsafe { &mut *e }.m.s3_mut() {
        for (i, core) in m.cores.iter().enumerate() {
            log(&format!("core={i}\n{}", core.blocks.profile.report()));
            if let Some(r) = core.blocks.region_report() { log(&format!("core={i} {r}")); }
            log(&format!("[census] core={i} insn_count={} jit_instructions={} builds={} compiled={}", core.insn_count, core.blocks.jit_instructions, core.blocks.builds, core.blocks.compiled));
        }
        log(&xtensa_lx7::census::report());
    }
}


/// Opt in to interactive host display publication for a supporting S3 board, before execution.
/// This changes host snapshots only, not guest display timing. Returns 1 if unsupported.
/// # Safety
/// The pointer must reference a live exclusively borrowed emulator.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_set_smooth_display(e: *mut Emu, on: u32) -> u32 {
    let e = unsafe { &mut *e };
    let Some(m) = e.m.s3_mut() else { return 1 };
    if m.insns() != 0 || on > 1 { return 1; }
    if m.bus.board.set_smooth_display(on != 0) { 0 } else { 1 }
}

#[cfg(test)]
mod imu_motion_tests {
    use super::*;
    use crate::{esp32sim_delete, esp32sim_new, esp32sim_set_imu_motion};

    fn status0(e: *mut Emu) -> u8 {
        // SAFETY: the test owns the live emulator.
        let m = unsafe { &mut *e }.m.s3_mut().unwrap();
        let mut devices = m.bus.board.i2c_devices();
        let (_, _, imu) = devices.iter_mut().find(|(_, addr, _)| *addr == 0x6b).unwrap();
        imu.start(false); imu.write(0x2e); imu.start(true);
        imu.read()
    }

    /// Both setter orders keep the scripted motion (the TE setter replaces the board).
    #[test]
    fn motion_survives_measured_te_in_either_order() {
        let board = "waveshare-amoled18-v2";
        for motion_first in [true, false] {
            // SAFETY: a fresh emulator, exclusively owned and deleted here.
            unsafe {
                let e = esp32sim_new(board.as_ptr(), board.len(), 4, 2);
                if motion_first { assert_eq!(esp32sim_set_imu_motion(e, 1), 0); }
                assert_eq!(esp32sim_set_measured_te(e, 1), 0);
                if !motion_first { assert_eq!(esp32sim_set_imu_motion(e, 1), 0); }
                assert_eq!(status0(e), 3, "motion first: {motion_first}");
                esp32sim_delete(e);
            }
        }
    }
}

#[cfg(test)]
mod wifi_nearby_tests {
    use super::*;
    use crate::machine::MachineKind;
    use crate::{esp32sim_delete, esp32sim_new};

    /// The SSIDs around the access point of the chip with the radio (the C6 beside a P4).
    fn around(e: *mut Emu) -> Vec<String> {
        // SAFETY: the test owns the live emulator.
        let e = unsafe { &mut *e };
        let ssids = |link: &esp_soc::wifi::StationLink| link.ap().map(|ap| ap.beacons.iter().map(|b| b.ssid.clone()).collect()).unwrap_or_default();
        match &mut e.m {
            MachineKind::S3(m) => ssids(&m.bus.periph.wifi.link),
            MachineKind::C3(m) => ssids(&m.bus.periph.wifi.link),
            MachineKind::Esp32(m) => ssids(&m.bus.periph.wifi.link),
            MachineKind::C6(m) => ssids(&m.bus.periph.wifi_mac.link),
            MachineKind::P4C6(m) => ssids(&m.c6.borrow().m.bus.periph.wifi_mac.link),
            _ => panic!("no radio"),
        }
    }

    /// `esp32sim_wifi_nearby` on every chip with a radio: the option's specs, one per line, reach
    /// that chip's access point; its own SSID is left out; a refused list leaves the networks; a
    /// new `esp32sim_wifi` starts with none; the P4 alone has no access point.
    #[test]
    fn wifi_nearby_reaches_the_access_point_of_every_chip_with_a_radio() {
        for board in ["none", "c3", "esp32c6", "esp32", "esp32p4+c6", "esp32p4"] {
            // SAFETY: the board name is readable for the call; the emulator is deleted below.
            let e = unsafe { esp32sim_new(board.as_ptr(), board.len(), 4, 0) };
            assert!(!e.is_null(), "{board}");
            // SAFETY: the test owns the live emulator; the text is readable for the call.
            let nearby = |s: &[u8]| unsafe { esp32sim_wifi_nearby(e, s.as_ptr(), s.len()) };
            // SAFETY: as above.
            let wifi = |s: &str| unsafe { esp32sim_wifi(e, s.as_ptr(), s.len()) };
            assert_eq!(nearby(b"ssid=Home"), 1, "{board}: no access point yet");
            if board == "esp32p4" {
                assert_eq!(wifi("ssid=Jumperly"), 1, "the P4 has no radio");
                assert_eq!(nearby(b"ssid=Home"), 1);
                // SAFETY: the emulator is not used again.
                unsafe { esp32sim_delete(e) };
                continue;
            }
            assert_eq!(wifi("ssid=Jumperly"), 0);
            assert_eq!(nearby(b"ssid=Home,chan=1,rssi=-60\r\nssid=Jumperly\n\n  ssid=Cafe,chan=11,rssi=-75,secure=0\n"), 0);
            assert_eq!(around(e), ["Home", "Cafe"], "{board}: the access point's own SSID is left out");
            assert_eq!(nearby(b"ssid=Home\nssid=Home,chan=6"), 1, "{board}: one BSSID twice");
            assert_eq!(nearby(b"colour=red"), 1, "{board}: an unknown key");
            assert_eq!(nearby(b"ssid=Office\n\xff"), 1, "{board}: not UTF-8");
            let many: String = (0..33).map(|i| format!("ssid=n{i}\n")).collect();
            assert_eq!(nearby(many.as_bytes()), 1, "{board}: more than 32");
            assert_eq!(around(e), ["Home", "Cafe"], "{board}: a refused list leaves the networks");
            assert_eq!(nearby(many.lines().skip(1).collect::<Vec<_>>().join("\n").as_bytes()), 0, "{board}: 32");
            assert_eq!(around(e).len(), 32);
            assert_eq!(nearby(b""), 0);
            assert!(around(e).is_empty(), "{board}: no lines, none");
            assert_eq!(nearby(b"ssid=Home"), 0);
            assert_eq!(wifi("ssid=Jumperly"), 0);
            assert!(around(e).is_empty(), "{board}: a new access point has none around");
            // SAFETY: the emulator is not used again.
            unsafe { esp32sim_delete(e) };
        }
    }
}
