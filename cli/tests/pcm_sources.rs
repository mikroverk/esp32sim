use emu_core::Bus;
use esp_periph::i2s::{PcmPins, PcmSource};
use esp_soc::SocBus;

#[derive(Clone, Copy)]
struct Chip {
    i2s: u32,
    dma: u32,
    gpio: u32,
    ram: u32,
    hz: u32,
    compact: bool,
    c6: bool,
    signal: u32,
    select_bit: u32,
    port: usize,
}
const S3: Chip = Chip {
    i2s: 0x6000f000,
    dma: 0x6003f000,
    gpio: 0x60004000,
    ram: 0x3fc90000,
    hz: 240_000_000,
    compact: false,
    c6: false,
    signal: 25,
    select_bit: 7,
    port: 0,
};
const C3: Chip = Chip {
    i2s: 0x6002d000,
    hz: 160_000_000,
    compact: true,
    signal: 15,
    select_bit: 6,
    ..S3
};
const C6: Chip = Chip {
    i2s: 0x6000c000,
    dma: 0x60080000,
    gpio: 0x60091000,
    ram: 0x40810000,
    c6: true,
    select_bit: 7,
    ..C3
};

fn route(bus: &mut impl SocBus, chip: Chip, pins: [u32; 3]) {
    let [bclk, ws, din] = pins;
    bus.write32(
        chip.gpio + 0x154 + chip.signal * 4,
        din | (1 << chip.select_bit),
    )
    .unwrap();
    bus.write32(chip.gpio + 0x554 + bclk * 4, chip.signal + 1)
        .unwrap();
    bus.write32(chip.gpio + 0x554 + ws * 4, chip.signal + 2)
        .unwrap();
}

fn receiver(bus: &mut impl SocBus, chip: Chip, pdm: bool, frames: u32) {
    if chip.c6 {
        bus.write32(0x60096078, (25 << 12) | (2 << 20) | (1 << 22))
            .unwrap();
    } else {
        bus.write32(chip.i2s + 0x30, (1 << 26) | (2 << 27) | 25)
            .unwrap();
    }
    bus.write32(chip.i2s + 0x28, (24 << 7) | (15 << 13) | (15 << 18))
        .unwrap();
    bus.write32(chip.i2s + 0x50, (1 << 16) | 3).unwrap();
    bus.write32(chip.i2s + 0x64, frames * 4).unwrap();
    bus.write32(chip.ram, (frames * 4) | (1 << 31)).unwrap();
    bus.write32(chip.ram + 4, chip.ram + 64).unwrap();
    bus.write32(chip.ram + 8, 0).unwrap();
    let (select, link) = if chip.compact {
        (0xa0, 0x80)
    } else {
        (0x48, 0x20)
    };
    bus.write32(chip.dma + select, 3 + chip.port as u32)
        .unwrap();
    bus.write32(chip.dma + link, (chip.ram & 0xfffff) | (1 << 22))
        .unwrap();
    bus.write32(
        chip.i2s + 0x20,
        4 | if pdm { (1 << 20) | (1 << 21) } else { 0 },
    )
    .unwrap();
}

fn check(bus: &mut impl SocBus, chip: Chip) {
    for (id, rate, pins, sample) in [(0, 8000, [4, 5, 6], 111), (15, 16000, [7, 8, 9], 222)] {
        let mut source = PcmSource::new(
            rate,
            PcmPins::I2s {
                bclk: pins[0],
                ws: pins[1],
                data: pins[2],
            },
        )
        .unwrap();
        source.push(&vec![[sample; 2]; rate as usize * 2 + 100]);
        assert_eq!(source.queued_frames(), rate as usize * 2);
        bus.pcm_sources().unwrap().inputs.resize_with(16, || None);
        bus.pcm_sources().unwrap().inputs[id] = Some(source);
    }
    // All sources advance while RX and DMA are stopped; reset preserves the bank.
    bus.tick(chip.hz / 2);
    bus.flush_ticks();
    assert_eq!(
        bus.pcm_sources().unwrap().inputs[0]
            .as_ref()
            .unwrap()
            .queued_frames(),
        12000
    );
    assert_eq!(
        bus.pcm_sources().unwrap().inputs[15]
            .as_ref()
            .unwrap()
            .queued_frames(),
        24000
    );
    for (pins, sample) in [
        ([4, 5, 6], 111u32),
        ([7, 8, 9], 222),
        ([4, 5, 9], 0),
    ] {
        bus.reboot([0; 6]);
        route(bus, chip, pins);
        receiver(bus, chip, false, 1);
        bus.tick(chip.hz / 8000);
        bus.flush_ticks();
        assert_eq!(bus.read32(chip.ram + 64).unwrap(), sample | (sample << 16));
    }
    // Host time drains the source with the DMA descriptor exhausted.
    for _ in 0..200 {
        bus.tick(chip.hz / 100);
    }
    bus.flush_ticks();
    assert_eq!(
        bus.pcm_sources().unwrap().inputs[0]
            .as_ref()
            .unwrap()
            .queued_frames(),
        0
    );
    assert_eq!(
        bus.pcm_sources().unwrap().inputs[15]
            .as_ref()
            .unwrap()
            .queued_frames(),
        0
    );
}

#[test]
fn routing_timing_dma_stall_and_reset_on_all_chips() {
    check(&mut esp32s3::machine([0; 6]).bus, S3);
    check(&mut esp32c3::machine([0; 6], 4 << 20).bus, C3);
    check(&mut esp32c6::machine([0; 6], 4 << 20).bus, C6);
}

#[test]
fn s3_port_switch_uses_same_source_and_pdm_requires_pdm_wiring() {
    let mut bus = esp32s3::machine([0; 6]).bus;
    let mut source = PcmSource::new(
        8000,
        PcmPins::I2s {
            bclk: 4,
            ws: 5,
            data: 6,
        },
    )
    .unwrap();
    source.push(&[[11; 2], [22; 2], [33; 2], [44; 2]]);
    bus.pcm_sources().unwrap().inputs.resize_with(1, || None);
    bus.pcm_sources().unwrap().inputs[0] = Some(source);
    for (chip, expected) in [
        (S3, 11),
        (
            Chip {
                i2s: 0x6002d000,
                signal: 30,
                port: 1,
                ..S3
            },
            22,
        ),
    ] {
        bus.write32(S3.i2s + 0x20, 0).unwrap();
        route(&mut bus, chip, [4, 5, 6]);
        receiver(&mut bus, chip, false, 1);
        bus.tick(chip.hz / 8000);
        bus.flush_ticks();
        assert_eq!(
            bus.read32(chip.ram + 64).unwrap(),
            expected | (expected << 16)
        );
    }
    assert_eq!(
        bus.pcm_sources().unwrap().inputs[0]
            .as_ref()
            .unwrap()
            .queued_frames(),
        2
    );
    bus.reboot([0; 6]);
    route(&mut bus, S3, [4, 5, 6]);
    bus.write32(S3.gpio + 0x554 + 4 * 4, 27).unwrap();
    receiver(&mut bus, S3, true, 1);
    bus.tick(S3.hz / 4000);
    bus.flush_ticks();
    assert_eq!(bus.read32(S3.ram + 64).unwrap(), 0);
    let mut pdm = PcmSource::new(8000, PcmPins::Pdm { clk: 4, data: 6 }).unwrap();
    pdm.push(&[[123; 2]; 8]);
    bus.pcm_sources().unwrap().inputs[0] = Some(pdm);
    bus.write32(S3.gpio + 0x554 + 4 * 4, 27).unwrap();
    receiver(&mut bus, S3, true, 1);
    bus.tick(S3.hz / 4000);
    bus.flush_ticks();
    assert_eq!(bus.read32(S3.ram + 64).unwrap(), 123 | (123 << 16));
}
