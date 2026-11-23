//! Pulse counter (PCNT) units of the classic ESP32, S3, C6 and P4.
//!
//! Each unit has two channels: a pulse input counts on rising/falling edges (POS/NEG_MODE 0
//! ignore, 1 increment, 2 decrement) and a control input modifies that (HCTRL/LCTRL_MODE 0 keep,
//! 1 invert, 2 disable). Inputs arrive through the GPIO matrix. Counters are 16-bit signed; high/low
//! limits, thresholds and zero crossings raise the unit's interrupt. The fields are the same on
//! all four chips; the classic ESP32 has eight units, so its CNT, INT, STATUS and CTRL registers sit
//! further up (IDF v5.5.4 soc/esp32/register/soc/pcnt_reg.h; esp32s3 and esp32c6
//! register/soc/pcnt_reg.h; esp32p4 register/hw_ver3/soc/pcnt_reg.h). Not modelled: the glitch
//! filter, the P4's step (CHANGE_CONF) and reset-pad inputs.
use crate::RegRam;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PcntLayout { Classic, S3, C6, P4 }

pub struct Pcnt { layout: PcntLayout, pub conf: [[u32; 3]; 8], pub cnt: [i16; 8], pub status: [u32; 8], pub int_raw: u32, pub int_ena: u32, pub ctrl: u32, ram: RegRam, pub events: u64 }

impl Pcnt {
    pub fn new(layout: PcntLayout) -> Self { Pcnt { layout, conf: [[0; 3]; 8], cnt: [0; 8], status: [0; 8], int_raw: 0, int_ena: 0, ctrl: 0, ram: RegRam::new(), events: 0 } }
    pub fn irq(&self) -> bool { self.int_raw & self.int_ena != 0 }
    fn units(&self) -> u32 { if self.layout == PcntLayout::Classic { 8 } else { 4 } }
    /// The matrix input signals (pulse, control) of channel `ch` of unit `u`, from each chip's
    /// soc/gpio_sig_map.h (IDF v5.5.4): classic PCNT_SIG_CH0_IN0_IDX 39 (units 5-7 from 71), S3 33,
    /// C6 101, P4 PCNT_SIG_CH0_PAD_IN0_IDX 141 (grouped by channel, then unit).
    pub fn signals(&self, u: u32, ch: u32) -> (u32, u32) {
        match self.layout {
            PcntLayout::Classic => { let base = if u < 5 { 39 + 4 * u } else { 71 + 4 * (u - 5) }; (base + ch, base + 2 + ch) }
            PcntLayout::S3 => (33 + 4 * u + ch, 35 + 4 * u + ch),
            PcntLayout::C6 => (101 + 4 * u + ch, 103 + 4 * u + ch),
            PcntLayout::P4 => (141 + 4 * ch + u, 149 + 4 * ch + u),
        }
    }
    pub fn read(&self, off: u32) -> u32 {
        let n = self.units();
        match off {
            _ if off < 12 * n => { let u = (off / 12) as usize; self.conf[u][((off % 12) / 4) as usize] }
            _ if off < 16 * n => self.cnt[((off - 12 * n) / 4) as usize] as u16 as u32,
            _ if off == 16 * n => self.int_raw,
            _ if off == 16 * n + 4 => self.int_raw & self.int_ena,
            _ if off == 16 * n + 8 => self.int_ena,
            _ if (16 * n + 0x10..20 * n + 0x10).contains(&off) => self.status[((off - 16 * n - 0x10) / 4) as usize],
            _ if off == 20 * n + 0x10 => self.ctrl,
            0xfc if self.layout == PcntLayout::S3 => 0x1912_0400,
            _ => self.ram.read(off),
        }
    }
    pub fn write(&mut self, off: u32, v: u32) {
        let n = self.units();
        match off {
            _ if off < 12 * n => { let u = (off / 12) as usize; self.conf[u][((off % 12) / 4) as usize] = v; }
            _ if off == 16 * n + 8 => self.int_ena = v,
            _ if off == 16 * n + 0xc => self.int_raw &= !v,
            _ if off == 20 * n + 0x10 => { self.ctrl = v; for (u, cnt) in self.cnt.iter_mut().enumerate().take(n as usize) { if v & (1 << (2 * u)) != 0 { *cnt = 0; } } }
            _ => self.ram.write(off, v),
        }
    }
    /// A GPIO input changed: `sig(idx)` gives the level of matrix input signal `idx` (None = not routed).
    pub fn gpio_edge(&mut self, pin: u8, level: bool, sig: &dyn Fn(u32) -> Option<(u8, bool)>) {
        for u in 0..self.units() as usize {
            if self.ctrl & (1 << (2 * u)) != 0 || self.ctrl & (1 << (2 * u + 1)) != 0 { continue; }   // reset held / paused
            let conf0 = self.conf[u][0];
            for ch in 0..2u32 {
                let (pulse, control) = self.signals(u as u32, ch);
                let Some((sp, _)) = sig(pulse) else { continue };
                if sp != pin { continue; }
                let sh = 16 + 8 * ch;
                let mode = if level { (conf0 >> (sh + 2)) & 3 } else { (conf0 >> sh) & 3 };          // pos_mode / neg_mode
                let mut delta: i32 = match mode { 1 => 1, 2 => -1, _ => 0 };
                if delta != 0 {
                    let ctrl_level = sig(control).is_none_or(|(_, l)| l);
                    let cm = if ctrl_level { (conf0 >> (sh + 4)) & 3 } else { (conf0 >> (sh + 6)) & 3 };   // hctrl / lctrl
                    match cm { 1 => delta = -delta, 2 => delta = 0, _ => {} }
                }
                if delta != 0 { self.count(u, delta); }
            }
        }
    }
    /// Counts the queued GPIO input edges (`Gpio::input_changes`, emptied). FUNCn_IN_SEL holds the
    /// pin in bits 5:0, IN_INV_SEL at bit 6 and SIG_IN_SEL at bit 7 on all four chips (IDF v5.5.4
    /// soc/<chip>/register/soc/gpio_reg.h; esp32p4 register/hw_ver1 and hw_ver3/soc/gpio_reg.h
    /// GPIO_SIGn_IN_SEL, which `gpio_ll_set_input_signal_from` and the P4 ROM's `gpio_matrix_in`
    /// set). An input without SIG_IN_SEL is not routed. Past the pins, the constant-high input
    /// GPIO_MATRIX_CONST_ONE_INPUT (0x38; 0x3F on the P4, soc/gpio_pins.h) reads high and any
    /// other value low.
    pub fn matrix_edges(&mut self, gpio: &mut crate::Gpio) {
        let (select, one) = (0x80, if self.layout == PcntLayout::P4 { 0x3f } else { 0x38 });
        // Each edge sees the other pins as they were at that edge: start from the levels before the
        // queued edges and replay them in order.
        let mut input = gpio.input;
        for &(pin, level) in gpio.input_changes.iter().rev() { input = (input & !(1u64 << (pin & 63))) | (u64::from(!level) << (pin & 63)); }
        let (pins, selections) = (gpio.pins, &gpio.func_in_sel);
        for (pin, level) in gpio.input_changes.drain(..) {
            input = (input & !(1u64 << (pin & 63))) | (u64::from(level) << (pin & 63));
            let sig = |idx: u32| -> Option<(u8, bool)> {
                let sel = *selections.get(idx as usize)?;
                if sel & select != select { return None; }
                let (pin, inv) = ((sel & 0x3f) as u8, sel & 0x40 != 0);
                Some(if pin < pins { (pin, ((input >> pin) & 1 != 0) ^ inv) } else { (u8::MAX, (sel & 0x3f == one) ^ inv) })
            };
            self.gpio_edge(pin, level, &sig);
        }
    }
    fn count(&mut self, u: usize, delta: i32) {
        let conf0 = self.conf[u][0]; let conf1 = self.conf[u][1]; let conf2 = self.conf[u][2];
        let old = self.cnt[u] as i32; let mut new = old + delta;
        // STATUS: CNT_THR_ZERO_MODE 1:0 (0 from positive, 1 from negative), THRES1_LAT 2, THRES0_LAT 3,
        // L_LIM 4, H_LIM 5, ZERO_LAT 6; IDF reads the events as STATUS >> 2 (hal/pcnt_ll.h).
        let mut ev = if new == 0 && old != 0 && conf0 & (1 << 11) != 0 { (1 << 6) | u32::from(old < 0) } else { 0 };
        if conf0 & (1 << 12) != 0 && new >= (conf2 & 0xffff) as i16 as i32 { ev |= 1 << 5; new = 0; }         // h_lim
        if conf0 & (1 << 13) != 0 && new <= (conf2 >> 16) as i16 as i32 { ev |= 1 << 4; new = 0; }             // l_lim
        if conf0 & (1 << 14) != 0 && new == (conf1 & 0xffff) as i16 as i32 { ev |= 1 << 3; }                   // thres0
        if conf0 & (1 << 15) != 0 && new == (conf1 >> 16) as i16 as i32 { ev |= 1 << 2; }                      // thres1
        self.cnt[u] = new as i16; self.events += 1;
        if ev != 0 { self.status[u] = ev; self.int_raw |= 1 << u; }
    }
}

impl crate::Device for Pcnt {
    fn read(&mut self, off: u32) -> u32 { Pcnt::read(self, off) }
    fn write(&mut self, off: u32, v: u32) -> crate::WriteEffect { Pcnt::write(self, off, v); crate::WriteEffect::NONE }
    fn irq_sources(&self) -> u64 { self.irq() as u64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unit 1 channel 0 counts falling edges of its pulse pin up to the high limit, on each layout's
    /// signals and register offsets.
    #[test]
    fn each_layout_counts_on_its_signals_and_registers() {
        // (layout, CNT of unit 1, INT_RAW, STATUS of unit 1, CTRL)
        for (layout, cnt, raw, status, ctrl) in [(PcntLayout::S3, 0x34, 0x40, 0x54, 0x60), (PcntLayout::C6, 0x34, 0x40, 0x54, 0x60),
                                                 (PcntLayout::P4, 0x34, 0x40, 0x54, 0x60), (PcntLayout::Classic, 0x64, 0x80, 0x94, 0xb0)] {
            let mut p = Pcnt::new(layout);
            let (pulse, control) = p.signals(1, 0);
            p.write(12, (1 << 16) | (1 << 12));                  // U1_CONF0: NEG_MODE increment, H_LIM enabled
            p.write(20, 3);                                      // U1_CONF2: H_LIM 3
            p.write(raw + 8, 2);
            let sig = |idx: u32| if idx == pulse { Some((7, false)) } else if idx == control { Some((8, true)) } else { None };
            p.gpio_edge(7, true, &sig);
            p.gpio_edge(9, false, &sig);
            assert_eq!(p.read(cnt), 0, "{layout:?}: a rising edge is ignored, another pin is not the input");
            p.gpio_edge(7, false, &sig);
            p.gpio_edge(7, false, &sig);
            assert_eq!(p.read(cnt), 2, "{layout:?}");
            assert!(!p.irq());
            p.gpio_edge(7, false, &sig);
            assert_eq!((p.read(cnt), p.read(raw), p.read(status)), (0, 2, 1 << 5), "{layout:?}: H_LIM resets the count");
            assert!(p.irq());
            p.write(raw + 12, 2);
            assert!(!p.irq());
            // Counting down through zero with THR_ZERO_EN: ZERO_LAT and the mode "from positive".
            p.write(12, (2 << 16) | (1 << 11));
            p.cnt[1] = 1;
            p.gpio_edge(7, false, &sig);
            assert_eq!((p.read(cnt), p.read(status)), (0, 1 << 6), "{layout:?}");
            p.write(raw + 12, 2);
            p.write(12, (1 << 16) | (1 << 12));
            p.gpio_edge(7, false, &sig);
            p.write(ctrl, 1 << 2);                               // PULSE_CNT_RST_U1 held
            p.gpio_edge(7, false, &sig);
            assert_eq!(p.read(cnt), 0, "{layout:?}");
        }
        let p = Pcnt::new(PcntLayout::Classic);
        assert_eq!(p.signals(4, 1), (56, 58));
        assert_eq!(p.signals(7, 0), (79, 81));
        assert_eq!(Pcnt::new(PcntLayout::P4).signals(3, 1), (148, 156));
    }

    /// A quadrature encoder through the matrix, as IDF's rotary_encoder example wires it: channel 0
    /// counts A's edges with B as control, channel 1 B's edges with A; four counts per detent cycle.
    /// Both chips route with SIG_IN_SEL (bit 7), as `gpio_matrix_in` writes it; the P4's
    /// constants are 0x3F high and 0x3E low.
    #[test]
    fn quadrature_through_the_matrix() {
        let select = 0x80;
        for (layout, one) in [(PcntLayout::C6, 0x38), (PcntLayout::P4, 0x3f)] {
            let mut p = Pcnt::new(layout);
            let mut gpio = crate::Gpio::with_pins(31);
            let (a, b) = (p.signals(0, 0), p.signals(0, 1));
            gpio.func_in_sel[a.0 as usize] = select; gpio.func_in_sel[a.1 as usize] = select | 2;
            gpio.func_in_sel[b.0 as usize] = select | 2; gpio.func_in_sel[b.1 as usize] = select;
            // ch0: falling increments, rising decrements, low control inverts; ch1 the other way round
            p.write(0, (1 << 16) | (2 << 18) | (1 << 22) | (2 << 24) | (1 << 26) | (1 << 30));
            gpio.set_input(0, true); gpio.set_input(2, true);
            gpio.input_changes.clear();
            for (pin, level) in [(0, false), (2, false), (0, true), (2, true)] { gpio.set_input(pin, level); }
            p.matrix_edges(&mut gpio);
            assert_eq!((p.cnt[0], gpio.input_changes.len()), (4, 0), "{layout:?}");
            for (pin, level) in [(2, false), (0, false), (2, true), (0, true)] { gpio.set_input(pin, level); }
            p.matrix_edges(&mut gpio);
            assert_eq!(p.cnt[0], 0, "{layout:?}");
            // A constant control input: low (0x3C on the C6, 0x3E on the P4) inverts channel 0.
            gpio.func_in_sel[a.1 as usize] = select | if one == 0x38 { 0x3c } else { 0x3e };
            gpio.set_input(0, false);
            p.matrix_edges(&mut gpio);
            assert_eq!(p.cnt[0], -1, "{layout:?}");
            // Without SIG_IN_SEL the pulse input is not routed: its pin's edges do not count.
            gpio.func_in_sel[a.0 as usize] = 0;
            gpio.set_input(0, true);
            p.matrix_edges(&mut gpio);
            assert_eq!(p.cnt[0], -1, "{layout:?}");
        }
    }
}
