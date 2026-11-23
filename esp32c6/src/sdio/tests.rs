//! The slave against the sequences IDF's host and slave drivers issue (components/sdmmc/sdmmc_io.c,
//! hal/sdio_slave_hal.c) and the SDIO Simplified Specification v3.00 rules they rely on.
use super::*;

const BASE: u32 = 0x4080_0000;

struct Ram(Vec<u8>);
impl Ram {
    fn new() -> Self { Ram(vec![0; 0x10000]) }
    fn mem(&mut self) -> Sram<'_> { Sram { base: BASE, bytes: &mut self.0, ver: &mut [] } }
    fn w32(&mut self, a: u32, v: u32) { let o = (a - BASE) as usize; self.0[o..o + 4].copy_from_slice(&v.to_le_bytes()); }
    fn r32(&self, a: u32) -> u32 { let o = (a - BASE) as usize; u32::from_le_bytes(self.0[o..o + 4].try_into().unwrap()) }
    /// A descriptor: size, length, eof, owner, buffer, next.
    fn desc(&mut self, at: u32, size: u32, len: u32, eof: bool, buf: u32, next: u32) {
        self.w32(at, size | len << 14 | (eof as u32) << 30 | 1 << 31);
        self.w32(at + 4, buf);
        self.w32(at + 8, next);
    }
}

fn cmd52(write: bool, func: u32, addr: u32, data: u8) -> u32 { (write as u32) << 31 | func << 28 | addr << 9 | data as u32 }
fn cmd53(write: bool, func: u32, block: bool, incr: bool, addr: u32, count: u32) -> u32 {
    (write as u32) << 31 | func << 28 | (block as u32) << 27 | (incr as u32) << 26 | addr << 9 | count
}
fn short(r: Result<SdResponse, SdCmdError>) -> u32 { match r { Ok(SdResponse::Short(v)) => v, other => panic!("{other:?}") } }

/// CMD5 (inquiry, then with the host's OCR), CMD3, CMD7: the card ends selected (§3.1.2, §6.14).
fn selected(s: &mut SdioSlave, ram: &mut Ram) {
    assert_eq!(short(s.command(5, 0, &mut ram.mem())), 1 << 31 | 2 << 28 | 0xff8000, "R4: ready, two functions, no memory, 2.7-3.6 V");
    assert_eq!(s.state, CardState::Inactive, "CMD5 with arg 0 does not start initialization");
    short(s.command(5, 0x30_0000, &mut ram.mem()));
    let r6 = short(s.command(3, 0, &mut ram.mem()));
    assert_eq!(r6, 0x0001_0000, "R6: an RCA, the SDIO-only status bits zero (§4.3)");
    short(s.command(7, r6 & 0xffff_0000, &mut ram.mem()));
    assert_eq!(s.state, CardState::Command);
}

fn rd(s: &mut SdioSlave, ram: &mut Ram, func: u32, addr: u32) -> u8 { short(s.command(52, cmd52(false, func, addr, 0), &mut ram.mem())) as u8 }
fn wr(s: &mut SdioSlave, ram: &mut Ram, func: u32, addr: u32, v: u8) -> u32 { short(s.command(52, cmd52(true, func, addr, v), &mut ram.mem())) }

#[test]
fn nothing_but_cmd5_is_answered_before_cmd5() {
    let (mut s, mut ram) = (SdioSlave::new(), Ram::new());
    for (cmd, arg) in [(52, cmd52(true, 0, 6, 8)), (0, 0), (8, 0x1aa), (3, 0), (7, 0x1_0000), (55, 0), (41, 0x30_0000)] {
        assert_eq!(s.command(cmd, arg, &mut ram.mem()), Err(SdCmdError::NoResponse), "CMD{cmd}");
    }
    selected(&mut s, &mut ram);
    assert_eq!(s.command(5, 0, &mut ram.mem()), Err(SdCmdError::NoResponse), "no CMD5 once an RCA is assigned");
    assert_eq!(s.command(7, 0x2_0000, &mut ram.mem()), Err(SdCmdError::NoResponse), "CMD7 with another RCA deselects");
    assert_eq!(s.state, CardState::Standby);
    assert_eq!(s.command(52, cmd52(false, 0, 0, 0), &mut ram.mem()), Err(SdCmdError::NoResponse), "CMD52 needs the card selected");
}

#[test]
fn the_cccr_fbr_and_cis_read_back() {
    let (mut s, mut ram) = (SdioSlave::new(), Ram::new());
    selected(&mut s, &mut ram);
    assert_eq!((rd(&mut s, &mut ram, 0, 0x00), rd(&mut s, &mut ram, 0, 0x01)), (0x32, 0x02), "CCCR and SD revisions from SDIO_VER");
    assert_eq!(rd(&mut s, &mut ram, 0, 0x08), CARD_CAP);
    assert_eq!(rd(&mut s, &mut ram, 0, 0x08) & 0x40, 0, "a full-speed card: the host may use four lines");
    let cis = (0..3).fold(0, |a, i| a | (rd(&mut s, &mut ram, 0, 0x09 + i) as u32) << (8 * i));
    assert_eq!(cis, CIS_ADDR);
    let tuples: Vec<u8> = (0..39).map(|i| rd(&mut s, &mut ram, 0, CIS_ADDR + i)).collect();
    assert_eq!(&tuples[..11], &[0x01, 0x03, 0xd9, 0x01, 0xff, 0x20, 0x04, 0x92, 0x00, 0x66, 0x66], "DEVICE, MANFID 0092/6666");
    assert_eq!(&tuples[11..], &CIS_TAIL[..], "FUNCID, FUNCE, CONFIG, CFTABLE_ENTRY, END");
    // walk the chain as sdmmc_io_get_cis_data does: code, link, body, until END
    let mut at = 0;
    while tuples[at] != 0xff { at += 2 + tuples[at + 1] as usize; }
    assert_eq!(at, tuples.len() - 1);
    // block sizes: the host writes 512 and reads it back (esp_hosted hosted_sdio_set_blocksize)
    for (lo, v) in [(0x10, 0x00u8), (0x11, 0x02), (0x110, 0x00), (0x111, 0x02)] { wr(&mut s, &mut ram, 0, lo, v); }
    assert_eq!((rd(&mut s, &mut ram, 0, 0x110) as u16) | (rd(&mut s, &mut ram, 0, 0x111) as u16) << 8, 512);
}

#[test]
fn cmd52_writes_echo_or_read_back_and_enable_the_function() {
    let (mut s, mut ram) = (SdioSlave::new(), Ram::new());
    selected(&mut s, &mut ram);
    // R5 (§5.2.1): IO_CURRENT_STATE CMD, the data echoed without RAW
    assert_eq!(wr(&mut s, &mut ram, 0, 0x02, 0xff), 0x10ff);
    assert_eq!(short(s.command(52, cmd52(true, 0, 0x02, 0xff) | 1 << 27, &mut ram.mem())), 0x1006, "RAW: IOE1 and IOE2 only");
    assert_eq!(rd(&mut s, &mut ram, 0, 0x03), 0, "not ready before the driver sets IOREADY1");
    Device::write(&mut s, HINF + 4, CFG_DATA1_RESET | IOREADY1);
    assert_eq!(rd(&mut s, &mut ram, 0, 0x03), 0x02, "IOR1");
    assert_eq!(Device::read(&mut s, HINF + 4) >> 11 & 1, 1, "CFG_DATA1.IOENABLE1 shows the host's IOE1");
    wr(&mut s, &mut ram, 0, 0x07, 0x02);
    assert_eq!(s.bus_width(), 4, "bus width 10b");
    // high speed: EHS sticks only where HINF's HIGHSPEED_ENABLE offers SHS (sdmmc_io_enable_hs_mode)
    assert_eq!(short(s.command(52, cmd52(true, 0, 0x13, 0x02) | 1 << 27, &mut ram.mem())) & 3, 0);
    Device::write(&mut s, HINF + 4, CFG_DATA1_RESET | IOREADY1 | HIGHSPEED_ENABLE);
    assert_eq!(short(s.command(52, cmd52(true, 0, 0x13, 0x02) | 1 << 27, &mut ram.mem())) & 3, 3);
    assert_eq!(wr(&mut s, &mut ram, 3, 0, 0) >> 8, 0x12, "an absent function: FUNCTION_NUMBER");
    // function 1's scratch registers are SLCHOST's CONF_W0..
    wr(&mut s, &mut ram, 1, 0x6d, 0xa5);
    assert_eq!(Device::read(&mut s, HOST + 0x6c), 0xa500, "the C6 reads what the host wrote");
    Device::write(&mut s, HOST + 0x9c, 0x1234_5678);
    assert_eq!(rd(&mut s, &mut ram, 1, 0x9e), 0x34);
    // the I/O reset puts the card back to needing CMD5 (§4.4)
    wr(&mut s, &mut ram, 0, 0x06, 0x08);
    assert_eq!((s.state, s.bus_width()), (CardState::Inactive, 1));
    assert_eq!(s.command(52, cmd52(false, 0, 0, 0), &mut ram.mem()), Err(SdCmdError::NoResponse));
}

#[test]
fn cmd53_bytes_read_the_register_window_as_esp_hosted_does() {
    let (mut s, mut ram) = (SdioSlave::new(), Ram::new());
    selected(&mut s, &mut ram);
    Device::write(&mut s, SLC + slc::TOKEN1, 1 << 12);              // WR 0
    for _ in 0..3 { Device::write(&mut s, SLC + slc::TOKEN1, 1 | 1 << 14); }   // INC_MORE 1, as recv_size_inc writes it
    Device::write(&mut s, SLC + slc::LEN_CONF, 1500 | 1 << 20);
    Device::write(&mut s, SLC + slc::INTVEC_TOHOST, 0x40);
    Device::write(&mut s, HOST + host::FUNC1_INT_ENA, 1 << 23 | 0xff);
    // TOKEN_RDATA: the buffer count in 27:16
    assert_eq!(short(s.command(53, cmd53(false, 1, false, true, host::TOKEN_RDATA, 4), &mut ram.mem())), 0x2000, "R5 in TRN");
    let mut b = [0u8; 4];
    s.read_data(&mut b, &mut ram.mem()).unwrap();
    assert_eq!(u32::from_le_bytes(b) >> 16 & 0xfff, 3);
    // INT_RAW through PKT_LEN in one transfer (sdio_drv.c DO_COMBINED_REG_READ)
    short(s.command(53, cmd53(false, 1, false, true, host::INT_RAW, 20), &mut ram.mem()));
    let mut regs = [0u8; 20];
    s.read_data(&mut regs, &mut ram.mem()).unwrap();
    let w = |i: usize| u32::from_le_bytes(regs[i..i + 4].try_into().unwrap());
    assert_eq!((w(0), w(8), w(16)), (0x40, 0x40, 1500));
    assert_eq!(s.read_data(&mut b, &mut ram.mem()), Err(SdDataError::Timeout), "the transfer is over");
}

#[test]
fn interrupts_both_ways_assert_and_clear() {
    let (mut s, mut ram) = (SdioSlave::new(), Ram::new());
    selected(&mut s, &mut ram);
    Device::write(&mut s, HOST + host::FUNC1_INT_ENA, 0xff | 1 << 23);
    Device::write(&mut s, SLC + slc::INTVEC_TOHOST, 1 << 7);         // sdio_slave_send_host_int(7)
    assert!(!s.host_interrupt(), "not on DAT1 until the host sets IENM and IEN1 (§6.3)");
    assert_eq!(rd(&mut s, &mut ram, 0, 0x05), 0x02, "but pending (INT1)");
    wr(&mut s, &mut ram, 0, 0x04, 0x03);
    assert!(s.host_interrupt());
    Device::write(&mut s, HINF + 4, CFG_DATA1_RESET | SDIO_INT_MASK);
    assert!(!s.host_interrupt(), "HINF SDIO_INT_MASK");
    Device::write(&mut s, HINF + 4, CFG_DATA1_RESET);
    // the host clears it with a 4-byte CMD53 to INT_CLR
    short(s.command(53, cmd53(true, 1, false, true, host::INT_CLR, 4), &mut ram.mem()));
    s.write_data(&(1u32 << 7).to_le_bytes(), &mut ram.mem()).unwrap();
    assert!(!s.host_interrupt());
    // host to slave: CONF_W7's low byte raises SLC_FRHOST_BITn
    assert!(!s.irq());
    Device::write(&mut s, SLC + slc::INT_ENA, 0xff);
    wr(&mut s, &mut ram, 1, host::CONF_W7, 1 << 2);
    assert!(s.irq());
    assert_eq!(Device::read(&mut s, SLC + slc::INT_ST), 1 << 2);
    assert_eq!(Device::read(&mut s, HOST + host::CONF_W7) & 0xff, 0, "the interrupt byte clears itself");
    Device::write(&mut s, SLC + slc::INT_CLR, 1 << 2);
    assert!(!s.irq());
}

/// The driver's send: the ISR invoker's one-byte descriptor, then a chain of two descriptors with
/// EOF on the last (stream mode, `send_new_packet`), read by the host in one block-mode CMD53 to
/// 0x1F800 - len (ESP-Hosted's block-only reads), zero filled past the packet.
#[test]
fn slave_to_host_descriptors_go_back_at_eof() {
    let (mut s, mut ram) = (SdioSlave::new(), Ram::new());
    selected(&mut s, &mut ram);
    Device::write(&mut s, SLC + slc::CONF0, slc::RX_AUTO_WRBACK);
    ram.desc(BASE + 0x100, 1, 1, true, BASE + 0x800, 0);
    Device::write(&mut s, SLC + slc::RX_LINK_ADDR, BASE + 0x100);
    Device::write(&mut s, SLC + slc::RX_LINK, slc::LINK_START);
    assert!(s.dma_pending());
    s.run_dma(&mut ram.mem());
    assert_eq!(Device::read(&mut s, SLC + slc::INT_RAW) & sint::RX_DONE, sint::RX_DONE, "the invoker's RX_DONE");
    assert_eq!(Device::read(&mut s, HOST + host::INT_RAW), 0, "no length follows the invoker: no RX_NEW_PACKET");
    Device::write(&mut s, SLC + slc::RX_LINK, slc::LINK_STOP);
    Device::write(&mut s, HOST + host::INT_CLR, hint::NEW_PACKET);
    Device::write(&mut s, SLC + slc::INT_CLR, sint::RX_DONE);
    // a 700-byte packet in two buffers
    for i in 0..700u32 { ram.0[0x1000 + i as usize] = i as u8; }
    ram.desc(BASE + 0x200, 600, 600, false, BASE + 0x1000, BASE + 0x210);
    ram.desc(BASE + 0x210, 100, 100, true, BASE + 0x1000 + 600, 0);
    Device::write(&mut s, SLC + slc::CONF0, slc::RX_AUTO_WRBACK | slc::RX_RST);
    Device::write(&mut s, SLC + slc::CONF0, slc::RX_AUTO_WRBACK);
    Device::write(&mut s, SLC + slc::RX_LINK_ADDR, BASE + 0x200);
    Device::write(&mut s, SLC + slc::RX_LINK, slc::LINK_START);
    s.run_dma(&mut ram.mem());
    Device::write(&mut s, SLC + slc::LEN_CONF, 700 | 1 << 20);
    assert_eq!(Device::read(&mut s, HOST + host::PKT_LEN), 700);
    assert_eq!(Device::read(&mut s, HOST + host::INT_RAW), hint::NEW_PACKET);
    short(s.command(53, cmd53(false, 1, true, true, FIFO_END - 700, 2), &mut ram.mem()));
    let mut got = vec![0u8; 1024];
    for chunk in got.chunks_mut(512) { s.read_data(chunk, &mut ram.mem()).unwrap(); }
    assert!(got[..700].iter().enumerate().all(|(i, &b)| b == i as u8), "the packet");
    assert!(got[700..].iter().all(|&b| b == 0), "zero filled past it");
    assert_eq!(Device::read(&mut s, SLC + slc::INT_RAW) & (sint::RX_DONE | sint::RX_EOF), sint::RX_DONE | sint::RX_EOF);
    assert_eq!(ram.r32(BASE + 0x200) >> 31, 0, "first descriptor written back");
    assert_eq!(ram.r32(BASE + 0x210) >> 31, 0, "second too");
    assert_eq!(s.stats.to_host, 700);
    assert_eq!(Device::read(&mut s, HOST + host::INT_RAW) & hint::RX_UDF, 0);
    // reading past what the link holds is an underflow
    short(s.command(53, cmd53(false, 1, false, true, FIFO_END - 8, 8), &mut ram.mem()));
    s.read_data(&mut got[..8], &mut ram.mem()).unwrap();
    assert_eq!(Device::read(&mut s, HOST + host::INT_RAW) & hint::RX_UDF, hint::RX_UDF);
}

/// The driver's receive: buffers loaded one after another (`sdio_slave_hal_load_buf`: START for
/// the first, the tail's next pointer and RESTART for the rest), TOKEN1 counting them; the host
/// writes packets with CMD53 to 0x1F800 - len, a packet longer than a buffer spills into the next,
/// and the block padding is dropped.
#[test]
fn host_to_slave_fills_buffers_and_hands_them_back() {
    let (mut s, mut ram) = (SdioSlave::new(), Ram::new());
    selected(&mut s, &mut ram);
    let (d0, d1, d2) = (BASE + 0x100, BASE + 0x110, BASE + 0x120);
    ram.desc(d0, 64, 0, false, BASE + 0x1000, 0);
    Device::write(&mut s, SLC + slc::TX_LINK_ADDR, d0);
    Device::write(&mut s, SLC + slc::TX_LINK, slc::LINK_START);
    ram.desc(d1, 64, 0, false, BASE + 0x1100, 0);
    ram.w32(d0 + 8, d1);
    Device::write(&mut s, SLC + slc::TX_LINK, slc::LINK_RESTART);
    // 80 bytes in one block of 512: the first buffer full, the second holds 16 with EOF
    let packet: Vec<u8> = (0..80u8).collect();
    let mut block = packet.clone(); block.resize(512, 0xee);
    short(s.command(53, cmd53(true, 1, true, true, FIFO_END - 80, 1), &mut ram.mem()));
    s.write_data(&block, &mut ram.mem()).unwrap();
    assert_eq!(ram.r32(d0), 64 | 64 << 14, "full, no EOF, owner cleared");
    assert_eq!(ram.r32(d1), 64 | 16 << 14 | 1 << 30, "16 bytes with EOF");
    assert_eq!(&ram.0[0x1000..0x1040], &packet[..64]);
    assert_eq!(&ram.0[0x1100..0x1110], &packet[64..]);
    assert_eq!(ram.0[0x1110], 0, "the padding went nowhere");
    assert_eq!(Device::read(&mut s, SLC + slc::INT_RAW) & (sint::TX_DONE | sint::TX_SUC_EOF), sint::TX_DONE | sint::TX_SUC_EOF);
    // the list ran out: the next packet waits for a buffer the driver appends and RESTARTs
    short(s.command(53, cmd53(true, 1, false, true, FIFO_END - 4, 4), &mut ram.mem()));
    s.write_data(&[1, 2, 3, 4], &mut ram.mem()).unwrap();
    assert_eq!((s.stats.overflows, Device::read(&mut s, HOST + host::INT_RAW) & hint::TX_OVF), (1, hint::TX_OVF), "no buffer: TX_OVF");
    ram.desc(d2, 64, 0, false, BASE + 0x1200, 0);
    ram.w32(d1 + 8, d2);
    Device::write(&mut s, SLC + slc::TX_LINK, slc::LINK_RESTART);
    // one packet in two CMD53s: a block, then the remainder at the incremented address
    let long: Vec<u8> = (0..40u8).collect();
    short(s.command(53, cmd53(true, 1, false, true, FIFO_END - 40, 32), &mut ram.mem()));
    s.write_data(&long[..32], &mut ram.mem()).unwrap();
    assert_eq!(ram.r32(d2) >> 31, 1, "still open");
    short(s.command(53, cmd53(true, 1, false, true, FIFO_END - 8, 8), &mut ram.mem()));
    s.write_data(&long[32..], &mut ram.mem()).unwrap();
    assert_eq!(ram.r32(d2), 64 | 40 << 14 | 1 << 30);
    assert_eq!(&ram.0[0x1200..0x1228], &long[..]);
}

/// Through the C6's bus: the register write that starts the rx link is the moment the DMA reads
/// the descriptor, so the driver's poll of RX_DONE right after it sees the bit; the interrupt
/// source follows SLC0INT_ST.
#[test]
fn a_link_start_through_the_bus_reads_the_descriptor_at_once() {
    use riscv_rv32::bus::Bus;
    let mut b = crate::bus::SocBus::new(0x10_0000, [0; 6]);
    let at = 0x4081_0000;
    for (o, v) in [(0, 1 | 1 << 14 | 1 << 30 | 1 << 31), (4, at + 0x100), (8, 0)] { b.write32(at + o, v).unwrap(); }
    b.write32(0x6001_7040, at).unwrap();
    b.write32(0x6001_703c, slc::LINK_START).unwrap();
    assert_eq!(b.read32(0x6001_7004).unwrap() & sint::RX_DONE, sint::RX_DONE);
    assert_eq!(b.read32(0x6001_8050).unwrap(), 0, "SLCHOST INT_RAW: RX_NEW_PACKET waits for the length");
    b.write32(0x6001_70f4, slc::LEN_WR | 1).unwrap();
    assert_eq!(b.read32(0x6001_8050).unwrap(), hint::NEW_PACKET);
    b.periph.refresh_lines();
    assert_eq!(b.periph.source_status()[2] & 1, 0, "SLC0 masked");
    b.write32(0x6001_700c, sint::RX_DONE).unwrap();
    b.periph.refresh_lines();
    assert_eq!(b.periph.source_status()[2] & 1, 1, "source 64 asserted");
    assert_eq!(b.sdio_command(5, 0), Ok(SdResponse::Short(1 << 31 | 2 << 28 | 0xff8000)));
}

/// `send_new_packet` stops, resets and starts the link, then writes PKT_LEN. The host's combined
/// register read (INT_RAW to PKT_LEN in one CMD53, esp_hosted sdio_drv.c `sdio_read_regs`) between
/// the START and the length must not announce a packet whose length it cannot see yet.
#[test]
fn a_packet_is_announced_with_its_length() {
    let (mut s, mut ram) = (SdioSlave::new(), Ram::new());
    selected(&mut s, &mut ram);
    let regs = |s: &mut SdioSlave, ram: &mut Ram| {
        short(s.command(53, cmd53(false, 1, false, true, host::INT_RAW, 20), &mut ram.mem()));
        let mut b = [0u8; 20];
        s.read_data(&mut b, &mut ram.mem()).unwrap();
        (u32::from_le_bytes(b[0..4].try_into().unwrap()), u32::from_le_bytes(b[16..20].try_into().unwrap()))
    };
    ram.desc(BASE + 0x200, 64, 64, true, BASE + 0x1000, 0);
    Device::write(&mut s, SLC + slc::RX_LINK_ADDR, BASE + 0x200);
    Device::write(&mut s, SLC + slc::RX_LINK, slc::LINK_START);
    s.run_dma(&mut ram.mem());
    assert_eq!(regs(&mut s, &mut ram), (0, 0), "between START and the length: nothing announced");
    Device::write(&mut s, SLC + slc::LEN_CONF, slc::LEN_WR | 64);
    assert_eq!(regs(&mut s, &mut ram), (hint::NEW_PACKET, 64), "announced with its length");
    assert_eq!(s.stats.stale_new_packets, 0);
    Device::write(&mut s, HOST + host::INT_CLR, hint::NEW_PACKET);
    // a STOP between them takes the announcement back
    Device::write(&mut s, SLC + slc::RX_LINK, slc::LINK_START);
    s.run_dma(&mut ram.mem());
    Device::write(&mut s, SLC + slc::RX_LINK, slc::LINK_STOP);
    Device::write(&mut s, SLC + slc::LEN_CONF, slc::LEN_WR | 128);
    assert_eq!(regs(&mut s, &mut ram).0, 0);
    // the counter: a length write that leaves PKT_LEN where the host last read it
    Device::write(&mut s, SLC + slc::RX_LINK, slc::LINK_START);
    s.run_dma(&mut ram.mem());
    Device::write(&mut s, SLC + slc::LEN_CONF, slc::LEN_WR | 128);
    assert_eq!(regs(&mut s, &mut ram), (hint::NEW_PACKET, 128));
    assert_eq!(s.stats.stale_new_packets, 1, "RX_NEW_PACKET with the length the host already had");
}

/// A data phase that ends later on the bus (a timed bus, or a host DMA resumed while the C6 is
/// behind): the bytes cross at once, the descriptors and RX_DONE/RX_EOF wait for its end.
#[test]
fn the_hand_back_waits_for_the_end_of_the_data_phase() {
    let (mut s, mut ram) = (SdioSlave::new(), Ram::new());
    selected(&mut s, &mut ram);
    Device::write(&mut s, SLC + slc::CONF0, slc::RX_AUTO_WRBACK);
    for i in 0..64 { ram.0[0x1000 + i] = i as u8; }
    ram.desc(BASE + 0x200, 64, 64, true, BASE + 0x1000, 0);
    Device::write(&mut s, SLC + slc::RX_LINK_ADDR, BASE + 0x200);
    Device::write(&mut s, SLC + slc::RX_LINK, slc::LINK_START);
    s.run_dma(&mut ram.mem());
    Device::write(&mut s, SLC + slc::INT_CLR, sint::RX_DONE);
    short(s.command(53, cmd53(false, 1, false, true, FIFO_END - 64, 64), &mut ram.mem()));
    let mut b = [0u8; 64];
    s.defer_to(Some(1000));
    s.read_data(&mut b, &mut ram.mem()).unwrap();
    s.defer_to(None);
    assert!(b.iter().enumerate().all(|(i, &v)| v == i as u8), "the bytes crossed");
    assert_eq!((ram.r32(BASE + 0x200) >> 31, Device::read(&mut s, SLC + slc::INT_RAW) & sint::RX_EOF), (1, 0), "nothing handed back yet");
    assert_eq!(s.next_due(), Some(1000));
    assert!(!s.apply_due(999, &mut ram.mem()));
    assert!(s.apply_due(1000, &mut ram.mem()));
    assert_eq!((ram.r32(BASE + 0x200) >> 31, Device::read(&mut s, SLC + slc::INT_RAW) & sint::RX_EOF), (0, sint::RX_EOF));
    assert_eq!(s.next_due(), None);
}

/// CMD52 into function 1's FIFO is a one-byte transfer to address A, so it follows the same rule
/// as CMD53: a packet of 0x1F800 - A bytes, closed by its last byte; a read takes the next byte of
/// the packet on the send link.
#[test]
fn cmd52_into_the_fifo_moves_packet_bytes() {
    let (mut s, mut ram) = (SdioSlave::new(), Ram::new());
    selected(&mut s, &mut ram);
    ram.desc(BASE + 0x100, 64, 0, false, BASE + 0x1000, 0);
    Device::write(&mut s, SLC + slc::TX_LINK_ADDR, BASE + 0x100);
    Device::write(&mut s, SLC + slc::TX_LINK, slc::LINK_START);
    wr(&mut s, &mut ram, 1, FIFO_END - 2, 0xa1);
    assert_eq!(ram.r32(BASE + 0x100) >> 31, 1, "a two-byte packet: still open after its first byte");
    wr(&mut s, &mut ram, 1, FIFO_END - 1, 0xa2);
    assert_eq!(ram.r32(BASE + 0x100), 64 | 2 << 14 | 1 << 30, "closed by its last byte");
    assert_eq!(&ram.0[0x1000..0x1002], &[0xa1, 0xa2]);
    ram.desc(BASE + 0x200, 4, 2, true, BASE + 0x1800, 0);
    ram.0[0x1800..0x1802].copy_from_slice(&[0x5a, 0x5b]);
    Device::write(&mut s, SLC + slc::RX_LINK_ADDR, BASE + 0x200);
    Device::write(&mut s, SLC + slc::RX_LINK, slc::LINK_START);
    s.run_dma(&mut ram.mem());
    assert_eq!((rd(&mut s, &mut ram, 1, FIFO_END - 2), rd(&mut s, &mut ram, 1, FIFO_END - 1)), (0x5a, 0x5b));
    assert_eq!(Device::read(&mut s, SLC + slc::INT_RAW) & sint::RX_EOF, sint::RX_EOF);
    assert_eq!(rd(&mut s, &mut ram, 1, FIFO_END - 1), 0, "nothing left: an underflow");
    assert_eq!(s.stats.underflows, 1);
}

/// Stream mode sends every queued packet in one chain, each ending with EOF, and PKT_LEN counts
/// bytes cumulatively in 20 bits: the host takes the difference from the length it last saw,
/// across the wrap at 2^20 (esp_hosted sdio_drv.c `sdio_get_len_from_slave`), and reads both
/// packets in one CMD53.
#[test]
fn a_chain_of_packets_and_pkt_len_across_its_wrap() {
    let (mut s, mut ram) = (SdioSlave::new(), Ram::new());
    selected(&mut s, &mut ram);
    Device::write(&mut s, SLC + slc::CONF0, slc::RX_AUTO_WRBACK);
    Device::write(&mut s, SLC + slc::LEN_CONF, slc::LEN_WR | 0xf_ff00);
    let seen = Device::read(&mut s, HOST + host::PKT_LEN);
    for i in 0..500 { ram.0[0x1000 + i] = (i % 199) as u8; }
    // packet 1: 300 bytes in two descriptors; packet 2: 200 bytes in one
    ram.desc(BASE + 0x200, 256, 256, false, BASE + 0x1000, BASE + 0x210);
    ram.desc(BASE + 0x210, 64, 44, true, BASE + 0x1000 + 256, BASE + 0x220);
    ram.desc(BASE + 0x220, 256, 200, true, BASE + 0x1000 + 300, 0);
    Device::write(&mut s, SLC + slc::RX_LINK_ADDR, BASE + 0x200);
    Device::write(&mut s, SLC + slc::RX_LINK, slc::LINK_START);
    s.run_dma(&mut ram.mem());
    Device::write(&mut s, SLC + slc::LEN_CONF, slc::LEN_INC_MORE | 500);
    let now = Device::read(&mut s, HOST + host::PKT_LEN);
    assert_eq!(now, 0xf4, "0xfff00 + 500 wraps in 20 bits");
    let len = if now >= seen { now - seen } else { (1 << 20) - seen + now };
    assert_eq!(len, 500, "the host's difference across the wrap");
    short(s.command(53, cmd53(false, 1, true, true, FIFO_END - len, 1), &mut ram.mem()));
    let mut got = [0u8; 512];
    s.read_data(&mut got, &mut ram.mem()).unwrap();
    assert!(got[..500].iter().enumerate().all(|(i, &b)| b == (i % 199) as u8), "both packets, back to back");
    assert!(got[500..].iter().all(|&b| b == 0));
    for d in [0x200, 0x210, 0x220] { assert_eq!(ram.r32(BASE + d) >> 31, 0, "descriptor {d:#x} handed back"); }
    assert_eq!(Device::read(&mut s, SLC + slc::INT_RAW) & sint::RX_EOF, sint::RX_EOF);
    assert_eq!(s.stats.underflows, 0);
}
