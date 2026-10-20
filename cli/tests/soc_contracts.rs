//! Host contracts that must agree across chips, independent of firmware or ROM assets.
use esp_soc::{Soc, SocBus};

const FLASH_SIZE: usize = 0x20000;

fn buses() -> [Box<dyn SocBus>; 3] {
    [
        Box::new(esp32c3::bus::SocBus::new(FLASH_SIZE, [0; 6])),
        Box::new(esp32c6::bus::SocBus::new(FLASH_SIZE, [0; 6])),
        Box::new(esp32s3::bus::SocBus::new(FLASH_SIZE, 0, [0; 6])),
    ]
}

fn image(addr: u32, len: usize) -> Vec<u8> {
    let mut bytes = vec![0xa5; 32 + len];
    bytes[0] = 0xe9;
    bytes[1] = 1;
    bytes[4..8].copy_from_slice(&addr.to_le_bytes());
    bytes[24..28].copy_from_slice(&addr.to_le_bytes());
    bytes[28..32].copy_from_slice(&(len as u32).to_le_bytes());
    bytes
}

#[test]
fn flash_and_app_offsets_are_checked_on_every_chip() {
    for mut bus in buses() {
        assert!(bus.write_flash(FLASH_SIZE, &[]).is_ok());
        assert!(bus.write_flash(FLASH_SIZE - 1, &[0x55]).is_ok());
        assert!(bus.write_flash(FLASH_SIZE, &[0x55]).is_err());
        assert!(bus.write_flash(FLASH_SIZE + 1, &[]).is_err());
        assert!(bus.write_flash(usize::MAX, &[1, 2]).is_err());
        assert!(bus.boot_app(FLASH_SIZE + 1).is_err());
        assert!(bus.boot_app(usize::MAX).is_err());
    }
}

fn check_last_flash_page(bus: &mut dyn SocBus, window_end: u32, page_size: usize) {
    let addr = window_end - page_size as u32 + 32;
    bus.write_flash(0, &image(addr, page_size)).unwrap();
    assert!(bus.boot_app(0).unwrap_err().contains("flash window"));
    // A rejected segment must not leave its first page mapped.
    assert!(bus.read8(addr).is_err());
    bus.write_flash(0, &image(addr, page_size - 32)).unwrap();
    assert_eq!(bus.boot_app(0), Ok(addr));
    assert_eq!(bus.read8(addr), Ok(0xa5));
    assert_eq!(bus.read8(window_end - 1), Ok(0xa5));
}

#[test]
fn mapped_segments_must_fit_the_whole_virtual_window() {
    let ends = [esp32c3::bus::IBUS_HIGH, esp32c6::bus::FLASH_HIGH, esp32s3::bus::IBUS_HIGH];
    for (mut bus, end) in buses().into_iter().zip(ends) {
        check_last_flash_page(&mut *bus, end, 0x10000);
    }
    for size_bits in 1..=3 {
        let mut bus = esp32c6::bus::SocBus::new(FLASH_SIZE, [0; 6]);
        bus.mmu_power_ctrl = size_bits << 3;
        let page_size = 1 << bus.page_shift();
        let end = esp32c6::bus::FLASH_LOW + esp32c6::bus::MMU_ENTRIES as u32 * page_size;
        check_last_flash_page(&mut bus, end, page_size as usize);
    }
}

#[test]
fn c3_console_preserves_uart1_output() {
    let mut bus = esp32c3::bus::SocBus::new(1, [0; 6]);
    bus.periph.uart[1].tx_out.extend_from_slice(b"uart1");
    assert_eq!(bus.console_take()[2], b"uart1");
    assert!(bus.console_take()[2].is_empty());
}

#[test]
fn c3_host_input_reaches_the_shared_interrupt_arbiter() {
    let mut bus = esp32c3::bus::SocBus::new(1, [0; 6]);
    let source = esp32c3::periph::src::USB_SERIAL_JTAG as u32;
    bus.periph.intc.write(source * 4, 3);
    bus.periph.intc.write(0x104, 1 << 3);
    bus.periph.intc.write(0x114 + 3 * 4, 1);
    bus.periph.intc.write(0x194, 1);
    bus.periph.usb.int_ena = 1 << 2;
    bus.irq_dirty = false;
    bus.serial_input(b"a");
    assert!(bus.irq_dirty);
    bus.refresh_irq();
    let mut pending = [None];
    esp32c3::C3::irqs(&bus, &mut pending);
    assert_eq!(pending, [Some(3)]);
    bus.irq_dirty = false;
    bus.gpio_set_input(0, false);
    assert!(bus.irq_dirty);
}

#[test]
fn c6_plic_and_intpri_share_host_input_interrupt_state() {
    let mut bus = esp32c6::bus::SocBus::new(1, [0; 6]);
    let source = esp32c6::periph::src::USB_SERIAL_JTAG as u32;
    bus.periph.intmtx.write(source * 4, 3);
    bus.periph.intc.plic_write(0, 1 << 3);
    bus.periph.intc.intpri_write(0x0c + 3 * 4, 1);
    bus.periph.intc.plic_write(0x90, 1);
    bus.periph.usb.int_ena = 1 << 2;
    bus.irq_dirty = false;
    bus.serial_input(b"a");
    assert!(bus.irq_dirty);
    bus.refresh_irq();
    let mut pending = [None];
    esp32c6::C6::irqs(&bus, &mut pending);
    assert_eq!(pending, [Some(3)]);
    assert_eq!(bus.periph.intc.plic_read(0x94), 3);
    assert_eq!(bus.periph.intc.intpri_read(0x08), 1 << 3);
    bus.irq_dirty = false;
    bus.gpio_set_input(0, false);
    assert!(bus.irq_dirty);
}

/// Reading the array back is what `--flash-file` and `--flash-out` save, so every chip must report the
/// bytes it accepted, its full size, and nothing past the end.
#[test]
fn flash_reads_back_what_was_written_on_every_chip() {
    let buses: [Box<dyn SocBus>; 4] = [
        Box::new(esp32::bus::SocBus::new(FLASH_SIZE, [0; 6])),
        Box::new(esp32c3::bus::SocBus::new(FLASH_SIZE, [0; 6])),
        Box::new(esp32c6::bus::SocBus::new(FLASH_SIZE, [0; 6])),
        Box::new(esp32s3::bus::SocBus::new(FLASH_SIZE, 0, [0; 6])),
    ];
    for mut bus in buses {
        assert_eq!(bus.flash_len(), FLASH_SIZE);
        assert_eq!(bus.read_flash(0, FLASH_SIZE).map(|f| f.len()), Some(FLASH_SIZE));
        assert!(bus.read_flash(0, FLASH_SIZE + 1).is_none(), "past the end");
        assert!(bus.read_flash(FLASH_SIZE, 1).is_none(), "past the end");
        assert!(bus.read_flash(FLASH_SIZE + 1, 0).is_none(), "past the end, even empty");
        assert_eq!(bus.read_flash(FLASH_SIZE, 0), Some(&[][..]), "an empty range at the end is inside");
        bus.write_flash(0x100, &[0x11, 0x22, 0x33]).unwrap();
        assert_eq!(bus.read_flash(0x100, 3), Some(&[0x11, 0x22, 0x33][..]), "what was written");
        assert_eq!(bus.read_flash(0x101, 2), Some(&[0x22, 0x33][..]), "at any offset");
        assert_eq!(bus.read_flash(0x100, 4), Some(&[0x11, 0x22, 0x33, 0xff][..]), "an unwritten cell reads 0xff");
    }
}
