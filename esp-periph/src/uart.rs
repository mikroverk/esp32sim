//! UART: TX to the host console, RX from it through the 128-byte receive FIFO; the transmit
//! side reads as idle (its FIFO count is 0 and TXFIFO_EMPTY/TX_DONE stay raised). The register
//! map differs a little between the chips — field widths, where rxfifo_rst sits — so the chip
//! crate picks a [`UartLayout`].
use std::collections::VecDeque;
use crate::device::{Device, WriteEffect};
use crate::regram::RegRam;

const RX_FIFO_SIZE: usize = 128;
const INT_RXFIFO_FULL: u32 = 1 << 0;
const INT_TXFIFO_EMPTY: u32 = 1 << 1;
const INT_RXFIFO_OVF: u32 = 1 << 4;
const INT_TX_DONE: u32 = 1 << 14;
const AUTOBAUD_PULSE_115200: u32 = 80_000_000 / 115_200;
/// TXFIFO_EMPTY and TX_DONE: always true here, so INT_CLR cannot take them down.
const INT_ALWAYS: u32 = INT_TXFIFO_EMPTY | INT_TX_DONE;

/// What differs between the chips' UART register maps (IDF `uart_reg.h` per chip).
#[derive(Clone, Copy, Debug)]
pub struct UartLayout {
    /// CONF1 rxfifo_full_thrhd: mask of the field at bit 0; txfifo_empty_thrhd starts right above it
    pub thrhd_mask: u32,
    /// CONF0 (CONF0_SYNC on the C6) rxfifo_rst bit
    pub rxfifo_rst: u32,
    /// STATUS rxfifo_cnt: mask of the field at bit 0
    pub rxfifo_cnt_mask: u32,
    /// Register and self-clearing bit used to synchronize configuration into the UART clock domain.
    pub reg_update_off: u32,
    pub reg_update_mask: u32,
    pub autobaud_mask: u32,
    pub lowpulse_off: u32,
    pub highpulse_off: u32,
    pub rxd_count_off: u32,
}
impl UartLayout {
    pub const S3: UartLayout = UartLayout { thrhd_mask: 0x3ff, rxfifo_rst: 1 << 17, rxfifo_cnt_mask: 0x3ff, reg_update_off: 0x80, reg_update_mask: 1 << 31, autobaud_mask: 1 << 27, lowpulse_off: 0x28, highpulse_off: 0x2c, rxd_count_off: 0x30 };
    pub const C3: UartLayout = UartLayout { thrhd_mask: 0x1ff, rxfifo_rst: 1 << 17, rxfifo_cnt_mask: 0x3ff, reg_update_off: 0x80, reg_update_mask: 1 << 31, autobaud_mask: 1 << 27, lowpulse_off: 0x28, highpulse_off: 0x2c, rxd_count_off: 0x30 };
    pub const C6: UartLayout = UartLayout { thrhd_mask: 0xff, rxfifo_rst: 1 << 22, rxfifo_cnt_mask: 0xff, reg_update_off: 0x98, reg_update_mask: 1, autobaud_mask: 1 << 19, lowpulse_off: 0x7c, highpulse_off: 0x80, rxd_count_off: 0x84 };
}

// ------------------------------------------------------------------ UART
pub struct Uart { pub tx_out: Vec<u8>, pub int_raw: u32, pub int_ena: u32, layout: UartLayout, rx: VecDeque<u8>, ram: RegRam }
impl Uart {
    pub fn new(layout: UartLayout) -> Self { Uart { tx_out: Vec::new(), int_raw: INT_ALWAYS, int_ena: 0, layout, rx: VecDeque::new(), ram: RegRam::new() } }
    /// Bytes from the host into the receive FIFO; what does not fit is dropped and flagged RXFIFO_OVF.
    pub fn host_input(&mut self, data: &[u8]) {
        let before = self.rx.len();
        for &b in data {
            if self.rx.len() >= RX_FIFO_SIZE { self.int_raw |= INT_RXFIFO_OVF; break; }
            self.rx.push_back(b);
        }
        let accepted = self.rx.len() - before;
        if accepted > 0 && self.ram.read(0x20) & self.layout.autobaud_mask != 0 {
            // A raw TCP stream has no baud metadata. Match esptool's initial 115200-baud sync:
            // the ROM waits for more than 127 RX edges, then derives the divider from the minimum
            // low/high pulse widths. Ten edge opportunities per UART frame are a conservative
            // approximation; the repeated 0x55 sync payload reaches the threshold immediately.
            let edges = self.ram.read(self.layout.rxd_count_off).saturating_add((accepted as u32).saturating_mul(10));
            self.ram.write(self.layout.lowpulse_off, AUTOBAUD_PULSE_115200);
            self.ram.write(self.layout.highpulse_off, AUTOBAUD_PULSE_115200);
            self.ram.write(self.layout.rxd_count_off, edges.min(0x3ff));
        }
        self.refresh_rx_full();
    }
    /// Bytes that can enter RX without setting the hardware overflow condition.
    pub fn rx_capacity(&self) -> usize { RX_FIFO_SIZE - self.rx.len() }
    /// CONF1 rxfifo_full_thrhd (the layout says how wide); the silicon reset value is 0x60, a
    /// driver that wants every byte sets 1. RXFIFO_FULL is a level here: it stays raised while the count is at or
    /// over the threshold, so a driver that clears it before draining is woken again.
    fn rx_full_threshold(&self) -> usize { ((self.ram.read(0x24) & self.layout.thrhd_mask) as usize).max(1) }
    fn refresh_rx_full(&mut self) {
        if self.rx.len() >= self.rx_full_threshold() { self.int_raw |= INT_RXFIFO_FULL; }
        else { self.int_raw &= !INT_RXFIFO_FULL; }
    }
    pub fn rx_pending(&self) -> usize { self.rx.len() }
    pub fn read(&mut self, off: u32) -> u32 {
        match off {
            0x0 => { let value = self.rx.pop_front().map(|b| b as u32).unwrap_or(0); self.refresh_rx_full(); value }
            0x4 => self.int_raw,
            0x8 => self.int_raw & self.int_ena,
            0xc => self.int_ena,
            0x1c => 0xe000_c000 | (self.rx.len() as u32 & self.layout.rxfifo_cnt_mask),   // STATUS: rxfifo_cnt, tx count 0, TXD/RTSN/DSRN idle levels as on silicon
            x if x == self.layout.reg_update_off => self.ram.read(off) & !self.layout.reg_update_mask,
            _ => self.ram.read(off),
        }
    }
    pub fn write(&mut self, off: u32, v: u32) {
        match off {
            0x0 => self.tx_out.push(v as u8),
            0xc => self.int_ena = v,
            0x10 => { self.int_raw &= !v | INT_ALWAYS; self.refresh_rx_full(); }
            0x20 => {
                if v & self.layout.rxfifo_rst != 0 { self.rx.clear(); }
                if v & self.layout.autobaud_mask != 0 && self.ram.read(0x20) & self.layout.autobaud_mask == 0 {
                    self.ram.write(self.layout.lowpulse_off, 0xfff);
                    self.ram.write(self.layout.highpulse_off, 0xfff);
                    self.ram.write(self.layout.rxd_count_off, 0);
                }
                self.ram.write(off, v);
            }   // CONF0 rxfifo_rst / AUTOBAUD_EN
            0x24 => { self.ram.write(off, v); self.refresh_rx_full(); }
            x if x == self.layout.reg_update_off => self.ram.write(off, v & !self.layout.reg_update_mask),
            _ => self.ram.write(off, v),
        }
    }
    pub fn irq(&self) -> bool { self.int_raw & self.int_ena != 0 }
}
impl Device for Uart {
    fn read(&mut self, off: u32) -> u32 { Uart::read(self, off) }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect { Uart::write(self, off, v); WriteEffect::NONE }
    fn irq_sources(&self) -> u64 { self.irq() as u64 }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The Linux esp32_uart driver's receive path: threshold 1, RXFIFO_FULL enabled, count from
    /// STATUS, pop the FIFO, then INT_CLR — and the line must drop only once the FIFO is empty.
    #[test]
    fn receive_fifo_drives_rxfifo_full_as_a_level() {
        let mut u = Uart::new(UartLayout::S3);
        u.write(0x24, 1); u.write(0xc, INT_RXFIFO_FULL);
        assert!(!u.irq());
        u.host_input(b"ro");
        assert!(u.irq()); assert_eq!(u.read(0x1c) & 0x3ff, 2);
        u.write(0x10, INT_RXFIFO_FULL);          // cleared early: still two bytes waiting
        assert!(u.irq());
        assert_eq!((u.read(0x0), u.read(0x0)), (b'r' as u32, b'o' as u32));
        u.write(0x10, INT_RXFIFO_FULL);
        assert!(!u.irq()); assert_eq!(u.read(0x1c) & 0x3ff, 0); assert_eq!(u.read(0x0), 0);
        assert_eq!(u.read(0x4) & INT_ALWAYS, INT_ALWAYS);
    }
    #[test]
    fn clear_before_drain_deasserts_once_fifo_falls_below_threshold() {
        let mut u = Uart::new(UartLayout::C3);
        u.write(0x24, 2); u.write(0xc, INT_RXFIFO_FULL);
        u.host_input(b"abc");
        assert!(u.irq());
        u.write(0x10, INT_RXFIFO_FULL); // level immediately reasserts: three bytes still pending
        assert!(u.irq());
        assert_eq!(u.read(0), b'a' as u32); // count reaches threshold: still asserted
        assert!(u.irq());
        assert_eq!(u.read(0), b'b' as u32); // below threshold: hardware level drops
        assert!(!u.irq());
    }
    #[test]
    fn receive_fifo_overflow_is_flagged_and_reset_by_conf0() {
        let mut u = Uart::new(UartLayout::S3);
        assert_eq!(u.rx_capacity(), RX_FIFO_SIZE);
        u.host_input(&[b'x'; RX_FIFO_SIZE + 3]);
        assert_eq!(u.read(0x1c) & 0x3ff, RX_FIFO_SIZE as u32);
        assert_eq!(u.rx_capacity(), 0);
        assert_ne!(u.read(0x4) & INT_RXFIFO_OVF, 0);
        u.write(0x20, 1 << 17);
        assert_eq!(u.read(0x1c) & 0x3ff, 0);
        assert_eq!(u.rx_capacity(), RX_FIFO_SIZE);
    }
    /// The C6 map: the IDF driver's default txfifo_empty_thrhd of 10 sits in bits 15:8 of CONF1,
    /// right above an 8-bit rxfifo_full_thrhd, and rxfifo_rst is bit 22 of CONF0_SYNC.
    #[test]
    fn c6_layout_reads_its_own_fields() {
        let mut u = Uart::new(UartLayout::C6);
        u.write(0x24, (10 << 8) | 1); u.write(0xc, INT_RXFIFO_FULL);
        u.host_input(b"x");
        assert!(u.irq());
        u.write(0x20, 1 << 17);   // the S3's rxfifo_rst bit means nothing here
        assert_eq!(u.read(0x1c) & 0xff, 1);
        u.write(0x20, 1 << 22);
        assert_eq!(u.read(0x1c) & 0xff, 0);
    }

    #[test]
    fn host_sync_completes_autobaud_measurement() {
        for layout in [UartLayout::S3, UartLayout::C3, UartLayout::C6] {
            let mut u = Uart::new(layout);
            u.write(0x20, layout.autobaud_mask);
            assert_eq!(u.read(layout.rxd_count_off), 0);
            u.host_input(&[0x55; 16]);
            assert!(u.read(layout.rxd_count_off) > 127);
            assert_eq!(u.read(layout.lowpulse_off), AUTOBAUD_PULSE_115200);
            assert_eq!(u.read(layout.highpulse_off), AUTOBAUD_PULSE_115200);
        }
    }

    #[test]
    fn register_update_self_clears_at_each_layouts_offset() {
        for layout in [UartLayout::S3, UartLayout::C3, UartLayout::C6] {
            let mut u = Uart::new(layout);
            u.write(layout.reg_update_off, layout.reg_update_mask | 0x1234);
            assert_eq!(u.read(layout.reg_update_off) & layout.reg_update_mask, 0);
            assert_eq!(u.read(layout.reg_update_off) & 0x1234, 0x1234 & !layout.reg_update_mask);
        }
    }
}
