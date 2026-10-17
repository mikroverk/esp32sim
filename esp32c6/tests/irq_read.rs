//! Read-side source changes must reach the CPU even when the following tick is quiet.
use emu_core::Bus;
use esp_soc::SocBus as _;
use esp32c6::periph::src;

const FIFO: u32 = 0x6000_f000;
const RECV_PKT: u32 = 1 << 2;

fn queued_packet() -> esp32c6::Machine {
    let mut m = esp32c6::machine([0; 6], 1024);
    m.bus.periph.intmtx.map[src::USB_SERIAL_JTAG] = 3;
    m.bus.periph.intc.lines.enable = 1 << 3;
    m.bus.periph.intc.lines.pri[3] = 1;
    // IDF v5.5.4 components/soc/esp32c6/register/soc/usb_serial_jtag_reg.h:
    // EP1, INT_ENA/CLR and SERIAL_OUT_RECV_PKT (bit 2).
    m.bus.write32(FIFO + 0x10, RECV_PKT).unwrap();
    m.bus.serial_input(&[0x41; 65]);
    for _ in 0..63 { assert_eq!(m.bus.read32(FIFO).unwrap(), 0x41); }
    // Acknowledge packet one before its final byte is drained.
    m.bus.write32(FIFO + 0x14, RECV_PKT).unwrap();
    m.sync_irq();
    assert!(!m.bus.irq_dirty);
    assert_eq!(m.bus.periph.intc.lines.pending(), None);
    m
}

#[test]
fn next_usb_packet_irq_is_presented_after_guest_fifo_read() {
    let mut m = queued_packet();
    let entry = esp32c6::bus::SRAM_LOW;
    // lw t0,0(a0); wfi; jal zero,0; nop; handler: addi s0,s0,1; jal zero,-4
    let words: [u32; 6] = [0x00052283, 0x10500073, 0x0000006f, 0x00000013, 0x00140413, 0xffdff06f];
    let bytes: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
    m.bus.load_bytes(entry, &bytes).unwrap();
    m.cores[0].pc = entry;
    m.cores[0].x[10] = FIFO;
    m.cores[0].mtvec = entry + 16;
    m.cores[0].mstatus |= 8;
    m.max_cycles = 256;
    m.run(256);
    assert!(m.bus.periph.usb.irq(), "final byte must present packet two");
    assert_eq!(m.cores[0].x[5], 0x41, "guest must execute the FIFO load");
    assert_eq!(m.cores[0].mcause, 0x8000_0003, "packet two must reach the CPU");
    assert!(m.cores[0].x[8] > 0, "interrupt handler must run");
}

#[test]
fn fifo_read_widths_preserve_invalidation_until_sync() {
    for (size, offset) in [(4, 0), (2, 0), (2, 2), (1, 0), (1, 1), (1, 2), (1, 3)] {
        let mut m = queued_packet();
        match size {
            4 => { m.bus.read32(FIFO + offset).unwrap(); }
            2 => { m.bus.read16(FIFO + offset).unwrap(); }
            _ => { m.bus.read8(FIFO + offset).unwrap(); }
        }
        assert!(m.bus.periph.usb.irq());
        assert!(m.bus.irq_dirty, "width {size}, offset {offset}");
        m.bus.read32(FIFO + 4).unwrap(); // unrelated read cannot lose an invalidation
        m.bus.tick(1);
        assert!(m.bus.irq_dirty);
        m.sync_irq();
        assert_eq!(m.bus.periph.intc.lines.pending(), Some(3));
        assert!(!m.bus.irq_dirty);
    }
}

#[test]
fn unrelated_reads_do_not_invalidate_a_clean_irq_gate() {
    let mut m = esp32c6::machine([0; 6], 1024);
    m.sync_irq();
    for addr in [FIFO + 4, FIFO + 8, FIFO + 12, 0x6000_001c, 0x6000_a040, 0x2000_1094, 0x6009_103c] {
        m.bus.read32(addr).unwrap();
        assert!(!m.bus.irq_dirty, "unrelated read {addr:#x}");
    }
}
