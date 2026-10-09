use esp_soc::DebugFlags;

#[test]
fn reboot_preserves_debug_areas() {
    for area in ["", "spi", "wifi", "mmio", "spi,wifi,mmio"] {
        let mut m = esp32c3::Machine::new([0; 6], esp32c3::bus::SocBus::new(4 << 20, [0; 6]));
        let mut debug = DebugFlags::default();
        debug.parse(area);
        m.set_debug(&debug);
        for boot in 0..3 {
            if boot != 0 { m.reboot(); }
            let p = &m.bus.periph;
            assert_eq!(m.bus.debug, debug, "area={area}, boot={boot}");
            assert_eq!(p.spi1.log, debug.has("spi"), "SPI: area={area}, boot={boot}");
            assert_eq!(p.wifi.log, debug.has("wifi"), "Wi-Fi: area={area}, boot={boot}");
            assert_eq!(p.misc.log_all, debug.has("mmio"), "MMIO: area={area}, boot={boot}");
        }
    }
}
