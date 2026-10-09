use esp32c6::bus::{SocBus, SRAM_LOW as RAM};
const SHA: u32 = 0x6008_9000;
const SELECT: u32 = 0x6008_0100;
const LINK: u32 = 0x6008_00e0;
const CONF1: u32 = 0x6008_00d4;
fn channel(bus: &mut SocBus) -> &mut esp_periph::GdmaOutCh { &mut bus.periph.gdma.gdma.out[0] }
include!("../../tests/sha_dma.rs");
