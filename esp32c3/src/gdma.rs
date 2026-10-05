//! C3 GDMA register layout translated to the shared channel state.
use esp_periph::{Device, WriteEffect};

#[derive(Default)]
pub struct Gdma { pub state: esp_periph::Gdma }

const IN: [u32; 7] = [0, 1, 2, 5, 7, 9, 10];
const OUT: [u32; 6] = [3, 4, 6, 8, 11, 12];
fn pack(value: u32, bits: &[u32]) -> u32 {
    bits.iter().enumerate().fold(0, |out, (i, bit)| out | (((value >> i) & 1) << bit))
}
fn unpack(value: u32, bits: &[u32]) -> u32 {
    bits.iter().enumerate().fold(0, |out, (i, bit)| out | (((value >> bit) & 1) << i))
}
fn map(off: u32) -> u32 {
    const T: [u32; 13] = [0, 4, 0x18, 0x1c, 0x20, 0x24, 0x28, 0x2c, 0x30, 0x34, 0x38, 0x44, 0x48];
    if off == 0x44 { return 0x3c8; }
    if (0x70..0x2b0).contains(&off) {
        let rel = off - 0x70; let n = rel / 0xc0; let k = rel % 0xc0;
        if k <= 0x30 { return n * 0xc0 + T[(k / 4) as usize]; }
        if (0x60..=0x90).contains(&k) { return n * 0xc0 + 0x60 + T[((k - 0x60) / 4) as usize]; }
    }
    0x1000 + off
}
impl Device for Gdma {
    fn read(&mut self, off: u32) -> u32 {
        let g = &self.state;
        if off < 0x30 {
            let n = (off / 0x10) as usize;
            let raw = pack(g.inp[n].int_raw, &IN) | pack(g.out[n].int_raw, &OUT);
            let ena = pack(g.inp[n].int_ena, &IN) | pack(g.out[n].int_ena, &OUT);
            return match off % 0x10 { 0 => raw, 4 => raw & ena, 8 => ena, _ => 0 };
        }
        g.read(map(off))
    }
    fn write(&mut self, off: u32, value: u32) -> WriteEffect {
        let g = &mut self.state;
        if off < 0x30 {
            let n = (off / 0x10) as usize;
            let input = unpack(value, &IN); let output = unpack(value, &OUT);
            match off % 0x10 {
                8 => { g.inp[n].int_ena = input; g.out[n].int_ena = output; }
                0 | 12 => { g.inp[n].int_raw &= !input; g.out[n].int_raw &= !output; }
                _ => {}
            }
        } else { g.write(map(off), value); }
        WriteEffect::NONE
    }
    fn irq_sources(&self) -> u64 { self.state.irq_sources() }
    fn debug(&mut self, on: bool) { self.state.debug(on); }
}
