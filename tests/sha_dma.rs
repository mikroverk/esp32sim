use emu_core::Bus;

fn fixture() -> SocBus {
    let mut bus = SocBus::new(4 << 20, [0; 6]);
    let mut block = [0u8; 64];
    block[..4].copy_from_slice(&[b'a', b'b', b'c', 0x80]);
    block[63] = 24;
    for (i, byte) in block.into_iter().enumerate() { bus.write8(RAM + 0x100 + i as u32, byte).unwrap(); }
    for (desc, len, buf, next, eof) in [(RAM, 16, RAM + 0x100, RAM + 16, 0), (RAM + 16, 48, RAM + 0x110, 0, 1)] {
        bus.write32(desc, (1 << 31) | (eof << 30) | (len << 12) | len).unwrap();
        bus.write32(desc + 4, buf).unwrap();
        bus.write32(desc + 8, next).unwrap();
    }
    bus.write32(SELECT, 7).unwrap();
    bus.write32(CONF1, 1 << 12).unwrap();
    bus.write32(SHA, 2).unwrap();
    bus.write32(SHA + 0xc, 1).unwrap();
    bus
}
fn start(bus: &mut SocBus) {
    bus.write32(LINK, (1 << 21) | (RAM & 0xfffff)).unwrap();
    bus.write32(SHA + 0x1c, 1).unwrap();
}
fn digest(bus: &mut SocBus) -> String {
    (0..8).flat_map(|n| bus.read32(SHA + 0x40 + n * 4).unwrap().to_le_bytes())
        .map(|b| format!("{b:02x}")).collect()
}
#[test]
fn sha256_split_descriptor_completes_on_start_write() {
    let mut bus = fixture();
    start(&mut bus);
    assert_eq!(digest(&mut bus), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(bus.read32(SHA + 0x18).unwrap(), 0);
    assert_eq!(bus.read32(RAM).unwrap() >> 31, 0);
    assert_eq!(bus.read32(RAM + 16).unwrap() >> 31, 0);
    assert_eq!(channel(&mut bus).eof_desc, RAM + 16);
    assert_eq!(channel(&mut bus).int_raw & 3, 3);
}
#[test]
fn sha_before_gdma_waits_for_channel_and_never_ticks() {
    let mut bus = fixture();
    bus.write32(SHA + 0x1c, 1).unwrap();
    bus.tick(1000);
    assert_eq!(bus.read32(SHA + 0x18).unwrap(), 1);
    assert_eq!(bus.periph.sha.blocks, 0);
    bus.write32(LINK, (1 << 21) | (RAM & 0xfffff)).unwrap();
    assert_eq!(digest(&mut bus), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}
#[test]
fn dma_continue_preserves_the_previous_hash_state() {
    let mut bus = fixture();
    let mut padded = [0u8; 128];
    padded[..80].fill(b'a'); padded[80] = 0x80;
    padded[120..].copy_from_slice(&640u64.to_be_bytes());
    for (n, block) in padded.as_chunks::<64>().0.iter().enumerate() {
        for (i, byte) in block.iter().enumerate() { bus.write8(RAM + 0x100 + i as u32, *byte).unwrap(); }
        bus.write32(RAM, (3 << 30) | (64 << 12) | 64).unwrap();
        bus.write32(RAM + 8, 0).unwrap();
        bus.write32(LINK, (1 << 21) | (RAM & 0xfffff)).unwrap();
        bus.write32(SHA + if n == 0 { 0x1c } else { 0x20 }, 1).unwrap();
        assert!(!bus.periph.sha.busy);
    }
    assert_eq!(digest(&mut bus), "0f45e858fbc4176cdf4e411f88281edefc390ae5afe7df0f44cd9297f0a64580");
    assert_eq!(bus.periph.sha.blocks, 2);
}
#[test]
fn invalid_chains_raise_descriptor_error_without_hashing() {
    for invalid in 0..6 {
        let mut bus = fixture();
        match invalid {
            0 => bus.write32(RAM + 8, RAM).unwrap(),
            1 => bus.write32(RAM + 16, (1 << 30) | (48 << 12) | 48).unwrap(),
            2 => bus.write32(RAM + 8, 0).unwrap(),
            3 => bus.write32(RAM + 4, SHA).unwrap(),
            4 => bus.write32(RAM + 4, u32::MAX - 3).unwrap(),
            _ => bus.write32(RAM + 8, u32::MAX - 3).unwrap(),
        }
        start(&mut bus);
        assert_eq!(bus.periph.sha.blocks, 0, "invalid case {invalid}");
        assert!(!bus.periph.sha.busy);
        assert_eq!(channel(&mut bus).int_raw & 4, 4);
        assert!(!channel(&mut bus).running);
    }
}
#[test]
fn owner_check_is_optional_and_requested_length_bounds_input() {
    let mut bus = fixture();
    bus.write32(CONF1, 0).unwrap();
    bus.write32(RAM, (1 << 30) | (128 << 12) | 128).unwrap();
    start(&mut bus);
    assert_eq!(digest(&mut bus), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(bus.periph.sha.blocks, 1);
}

#[test]
fn cycle_detection_is_independent_of_owner_checking() {
    let mut bus = fixture();
    bus.write32(CONF1, 0).unwrap();
    bus.write32(RAM + 8, RAM).unwrap();
    start(&mut bus);
    assert_eq!(bus.periph.sha.blocks, 0);
    assert_eq!(channel(&mut bus).int_raw & 4, 4);
}

#[test]
fn one_dma_request_hashes_every_block_without_reinitializing() {
    let mut bus = fixture();
    let mut padded = [0u8; 128];
    padded[..80].fill(b'a'); padded[80] = 0x80;
    padded[120..].copy_from_slice(&640u64.to_be_bytes());
    for (i, byte) in padded.iter().enumerate() { bus.write8(RAM + 0x100 + i as u32, *byte).unwrap(); }
    bus.write32(RAM, (3 << 30) | (128 << 12) | 128).unwrap();
    bus.write32(SHA + 0xc, 2).unwrap();
    start(&mut bus);
    assert_eq!(digest(&mut bus), "0f45e858fbc4176cdf4e411f88281edefc390ae5afe7df0f44cd9297f0a64580");
    assert_eq!(bus.periph.sha.blocks, 2);
    assert!(!bus.periph.sha.dma_pending);
}
