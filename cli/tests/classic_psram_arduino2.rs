//! External qualification of stock Arduino-ESP32 2.0.17, not a passing CI golden.
//! Build the public P0 recipe with A2; do not substitute a unicore SDK.
use emu_core::Bus;
use esp_soc::SocBus;
use std::{env, fs, path::PathBuf};

fn boot(present: bool) -> String {
    let firmware = PathBuf::from(env::var_os("P5_FIRMWARE_DIR")
        .expect("set P5_FIRMWARE_DIR to the stock A2 fw-A2 directory"));
    let rom = PathBuf::from(env::var_os("ESP32SIM_ROM_DIR")
        .expect("set ESP32SIM_ROM_DIR to the fetched mask ROM directory"));
    let mut m = esp32::machine([0; 6], 4 << 20);
    m.bus.set_psram_size(if present { 4 << 20 } else { 0 }).unwrap();
    m.console.capture = true;
    m.load_rom(&fs::read(rom.join("esp32_rev300_rom.elf")).unwrap()).unwrap();
    m.add_symbols(&fs::read(firmware.join("p0_probe.ino.elf")).unwrap()).unwrap();
    for prefix in ["psram", "esp_spiram", "cache_sram_mmu", "esp_himem"] { m.trace_fns(prefix); }
    for (offset, suffix) in [(0x1000, ".bootloader.bin"), (0x8000, ".partitions.bin"), (0x10000, ".bin")] {
        m.write_flash(offset, &fs::read(firmware.join(format!("p0_probe.ino{suffix}"))).unwrap()).unwrap();
    }
    if present {
        let pc = m.sym_addr("esp_spiram_test").expect("real SDK memory test symbol");
        m.add_observer(Box::new(esp_soc::observers::Breakpoints { pcs: vec![pc] }));
    }
    m.boot_rom();
    let mut stop = esp_soc::Stop::MaxInsns;
    for _ in 0..1500 {
        stop = m.run(100_000);
        let serial = String::from_utf8_lossy(&m.console.uart0);
        if matches!(stop, esp_soc::Stop::Breakpoint(_)) {
            println!("P5 before guest memory test: {stop:?}");
            for addr in [0x3ff00040, 0x3ff00044, 0x3ff00058, 0x3ff0005c] {
                println!("P5 pretest register {addr:08x}={:08x}", m.bus.read32(addr).unwrap());
            }
            for page in 0..128 {
                assert_eq!(m.bus.read32(0x3ff11200 + page * 4).unwrap(), page, "guest PRO map");
                assert_eq!(m.bus.read32(0x3ff13200 + page * 4).unwrap(), page, "guest APP map");
            }
            println!("P5 pretest matched-mappings=128 external-read={:?}", m.bus.read32(0x3f800000));
            m.observers.clear();
            continue;
        }
        if serial.contains("P0 result=") || serial.contains("Rebooting...")
            || !matches!(stop, esp_soc::Stop::MaxInsns) { break; }
    }
    let serial = String::from_utf8_lossy(&m.console.uart0).into_owned();
    println!("{serial}");
    println!("P5 host present={present} stop={stop:?} insns={} exceptions={} interrupts={} irq={:?}",
        m.insns(), m.exceptions, m.interrupts, m.irq_hist);
    // IDF 4.4.7 dport_reg.h: PRO/APP cache controls and SRAM MMU tables.
    for addr in [0x3ff00040, 0x3ff00044, 0x3ff00058, 0x3ff0005c,
        0x3ff11200, 0x3ff113fc, 0x3ff13200, 0x3ff133fc] {
        println!("P5 register {addr:08x}={:08x}", m.bus.read32(addr).unwrap());
    }
    println!("P5 external-read={:?}", m.bus.read32(0x3f800000));
    assert!(m.stubs.is_empty());
    assert_eq!(m.stub_hits, 0);
    assert_eq!(m.reboots, 0, "do not mask startup failure with reboot");
    serial
}

#[test]
#[ignore = "needs P5_FIRMWARE_DIR stock A2 public P0 build and ESP32SIM_ROM_DIR; currently BLOCKED"]
fn external_arduino2_psram_startup_and_readback() {
    let serial = boot(true);
    assert!(serial.contains("P0 found=1"), "stock Arduino startup must detect PSRAM");
    assert!(serial.contains("P0 allocation=1048576 errors=0 fnv1a=678f9dc5"),
        "real guest allocator and bytewise readback must succeed");
    assert!(serial.contains("P0 result=readback-complete"));
    // This prerequisite does not yet qualify ESP.getPsramSize or free/reallocate.
}

#[test]
#[ignore = "needs P5_FIRMWARE_DIR stock A2 public P0 build and ESP32SIM_ROM_DIR"]
fn external_arduino2_no_device_is_rejected() {
    let serial = boot(false);
    assert!(serial.contains("PSRAM ID read error: 0xffffffff"));
    assert!(serial.contains("P0 found=0 heap=0 free=0"));
    assert!(serial.contains("P0 allocation=unavailable"));
    assert!(serial.contains("P0 result=no-psram"));
    assert!(!serial.contains("P0 found=1"));
}
