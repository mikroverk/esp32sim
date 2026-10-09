//! Classic I2S RX register adapter and native descriptor DMA.
//! ESP-IDF v5.5.4 components/soc/esp32/register/soc/i2s_reg.h:93-98,738-756,1373-1390;
//! include/soc/gpio_sig_map.h:69-72,299,309-312,343 supplies the GPIO matrix signals.
//! dport_reg.h:947,965 defines clock gates; i2s_reg.h:149-178 defines RX interrupt bits.
//! Samples are packed at DMA boundaries; no bit-level clock model is claimed.
use esp_periph::{Device, RegRam, WriteEffect};
use esp_periph::i2s::{I2s, PcmSources};
use crate::periph::CPU_HZ;
use crate::periph::ClassicGpio;

pub struct ClassicI2s {
    pub inner: I2s,
    ram: RegRam,
    dma: esp_periph::gdma::GdmaInCh,
}
impl Default for ClassicI2s { fn default() -> Self { Self::new() } }
impl ClassicI2s {
    pub fn new() -> Self {
        let mut ram = RegRam::new();
        ram.write(0x24, 64);
        Self { inner: I2s::new(CPU_HZ), ram, dma: esp_periph::gdma::GdmaInCh { conf1: 1 << 12, ..Default::default() } }
    }
    /// Writes go through `Device::write`, as the other chips' dispatch does, so the shared
    /// inherent writer keeps its single caller and S3/C3/C6 code generation is unchanged.
    fn sync(&mut self) {
        let conf = self.ram.read(8);
        let clock = self.ram.read(0xac);
        let sample = self.ram.read(0xb0);
        let bits = (sample >> 18) & 63;
        let bck = (sample >> 6) & 63;
        let (a, b) = ((clock >> 14) & 63, (clock >> 8) & 63);
        let div = if b == 0 || a < b { 0 } else { b | ((a % b) << 9) | ((a / b - 1) << 18) };
        let supported = clock & (1 << 21) == 0 && bck != 0 && [16, 24, 32].contains(&bits);
        Device::write(&mut self.inner, 0x30, (clock & 255) | (2 << 27) | if supported { 1 << 26 } else { 0 });
        Device::write(&mut self.inner, 0x38, div);
        Device::write(&mut self.inner, 0x28, (bck.saturating_sub(1) << 7) | (bits.saturating_sub(1) << 13) | ((if bits <= 16 { 15 } else { 31 }) << 18));
        let channel = (self.ram.read(0x2c) >> 3) & 3;
        Device::write(&mut self.inner, 0x50, (1 << 16) | match channel { 1 => 2, 2 => 1, _ => 3 });
        Device::write(&mut self.inner, 0x20, ((conf >> 3) & 4) | ((conf >> 1) & 3) | ((conf & (1 << 7)) >> 4));
    }
    #[inline]
    pub(crate) fn data(&mut self, cycles: u64, now: u64, port: usize, gpio: &ClassicGpio, sources: Option<&mut PcmSources>) -> Vec<u8> {
        let (data, clock) = [(155, [27, 28]), (181, [164, 165])][port];
        let signals = esp_periph::i2s::RxSignals { data, clock, input_select_bit: 7, output_mask: 0x3ff };
        self.inner.receive(cycles, now, false, &gpio.gpio, signals, sources)
    }
    #[inline]
    pub(crate) fn complete(&mut self, descriptor: u32) {
        self.ram.write(0x3c, descriptor);
        self.inner.int_raw |= 1 << 9;
    }
    #[inline]
    pub(crate) fn fault(&mut self) { self.inner.int_raw |= 1 << 13; }
}
impl Device for ClassicI2s {
    fn read(&mut self, off: u32) -> u32 {
        match off {
            0xc | 0x10 | 0x14 => self.inner.read(off),
            8 => self.ram.read(off) & !15,
            0xbc => 1,
            _ => self.ram.read(off),
        }
    }
    fn write(&mut self, off: u32, value: u32) -> WriteEffect {
        self.ram.write(off, value);
        match off {
            0x14 | 0x18 => { Device::write(&mut self.inner, off, value); }
            0x34 => {
                if value & (1 << 28) != 0 { self.dma.running = false; }
                if value & (3 << 29) != 0 { self.dma.desc = 0x3ff0_0000 | (value & 0xfffff); self.dma.buf_pos = 0; self.dma.rx_eof_pos = 0; self.dma.running = true; }
            }
            8 | 0x2c | 0xac | 0xb0 => self.sync(),
            _ => {}
        }
        WriteEffect::NONE
    }
    fn irq_sources(&self) -> u64 { self.inner.irq() as u64 }
}
impl crate::bus::SocBus {
    #[inline]
    pub(crate) fn i2s_step(&mut self, cycles: u64) {
        for port in 0..2 {
            let clock_bit = if port == 0 { 4 } else { 21 };
            if self.periph.dport.ram.read(0xc0) & (1 << clock_bit) == 0 || !self.periph.i2s[port].dma.running { continue; }
            // Pin-routed sources live on controller 0, as on the other chips.
            let mut bank = self.periph.i2s[0].inner.take_pcm_bank();
            let data = self.periph.i2s[port].data(cycles, self.cycles, port, &self.periph.gpio, bank.as_deref_mut());
            self.periph.i2s[0].inner.restore_pcm_bank(bank);
            let mut dma = self.periph.i2s[port].dma;
            let mut irq = false;
            // IDF v5.5.4 hal/esp32/include/hal/i2s_ll.h:639-642 counts RXEOF_NUM in words.
            if let Some(eof_bytes) = self.periph.i2s[port].ram.read(0x24).checked_mul(4) {
                dma.receive(self, &data, Some(eof_bytes), false, Self::is_periph, &mut irq);
            } else {
                dma.fail_receive(&mut irq);
            }
            if dma.int_raw & (1 << 1) != 0 { self.periph.i2s[port].complete(dma.eof_desc); }
            if dma.int_raw & (1 << 3) != 0 { self.periph.i2s[port].fault(); }
            dma.int_raw = 0;
            self.periph.i2s[port].dma = dma;
            self.periph.i2s[port].inner.recycle_rx_buffer(data);
            self.irq_dirty |= irq;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classic_fractional_clock_width_and_slave_selection() {
        let mut i = ClassicI2s::new();
        i.write(0xac, 25 | (1 << 8) | (2 << 14));
        i.write(0xb0, (25 << 6) | (16 << 18));
        assert_eq!(i.inner.rx_rate(), Some(7843));
        i.write(0xb0, (25 << 6) | (24 << 18));
        assert_eq!(i.inner.rx_rate(), Some(3922));
        i.write(0xac, 25 | (1 << 21));
        assert_eq!(i.inner.rx_rate(), None);
        i.write(0xac, 25);
        i.write(0xb0, 16 << 18);
        assert_eq!(i.inner.rx_rate(), None);
        i.write(0xb0, (25 << 6) | (8 << 18));
        assert_eq!(i.inner.rx_rate(), None);
        i.write(0xb0, (25 << 6) | (16 << 18));
        i.write(8, (1 << 5) | (1 << 7));
        assert!(i.inner.rx_data(30_000, false, |_| [1, 2]).is_empty());
        i.write(8, 1 << 5);
        i.write(0x2c, 1 << 3);
        assert_eq!(i.inner.rx_data(30_000, false, |_| [1, 2]), [2, 0]);
        i.write(0x2c, 2 << 3);
        assert_eq!(i.inner.rx_data(30_000, false, |_| [1, 2]), [1, 0]);
    }
}
