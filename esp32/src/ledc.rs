//! Classic HS/LS banks over the shared LEDC engine.
//! IDF v5.5.4 components/soc/esp32/register/soc/ledc_reg.h:1464-1472,
//! 1666-1674; clk_tree_defs.h:40 gives the nominal 8.5 MHz RC_FAST source.
use emu_core::ClockDomain;
use esp_periph::{Device, Ledc, RegRam, WriteEffect, APB_HZ};

pub struct ClassicLedc {
    groups: [Ledc<true>; 2],
    regs: RegRam,
    dirty: u16,
    pub clock_enabled: bool,
    #[cfg(test)]
    pub(crate) tick_calls: u64,
}
impl Default for ClassicLedc { fn default() -> Self { Self::new() } }
impl ClassicLedc {
    pub fn new() -> Self {
        Self { groups: std::array::from_fn(|_| Ledc::new_classic()), regs: RegRam::new(), dirty: u16::MAX, clock_enabled: false,
            #[cfg(test)] tick_calls: 0 }
    }
    fn source(apb: bool, conf: u32) -> u64 {
        if conf & (1 << 25) == 0 { 1_000_000 } else if apb { APB_HZ } else { 8_500_000 }
    }
    fn channel(off: u32) -> Option<(usize, u32)> { (off < 0x140).then_some(((off / 0xa0) as usize, off % 0xa0)) }
    fn timer(off: u32) -> Option<(usize, u32)> { (0x140..0x180).contains(&off).then(|| (((off - 0x140) / 0x20) as usize, 0xa0 + (off - 0x140) % 0x20)) }
    fn raw(&mut self, off: u32) -> u32 {
        let hs = self.groups[0].read(off);
        let ls = self.groups[1].read(off);
        (hs & 15) | ((ls & 15) << 4) | ((hs & 0xff0) << 4) | ((ls & 0xff0) << 12)
    }
    pub fn take_signal_updates(&mut self) -> u16 { std::mem::take(&mut self.dirty) }
    pub fn signal_level(&self, channel: usize) -> (bool, bool) { self.groups[channel / 8].signal_level(channel % 8) }
    pub fn pwm(&self, signal: usize) -> Option<(f64, u32)> {
        let ch = signal.checked_sub(71)?;
        if ch >= 16 || !self.clock_enabled { return None; }
        let group = ch / 8;
        let timer = self.groups[group].channel_timer(ch % 8);
        let hz = Self::source(group == 0 || self.regs.read(0x190) & 1 != 0, self.groups[group].timer_conf(timer));
        self.groups[group].channel_output(ch % 8, hz)
    }
}
impl Device for ClassicLedc {
    fn read(&mut self, off: u32) -> u32 {
        if let Some((g, o)) = Self::channel(off) {
            return if o % 0x14 == 0x10 { self.groups[g].read(o) } else { self.regs.read(off) };
        }
        if let Some((g, o)) = Self::timer(off) {
            return if o % 8 == 4 { self.groups[g].read(o) } else { self.regs.read(off) };
        }
        match off { 0x180 => self.raw(0xc0), 0x184 => self.raw(0xc4), 0x188 => self.raw(0xc8), 0x18c => 0, _ => self.regs.read(off) }
    }
    fn write(&mut self, off: u32, value: u32) -> WriteEffect {
        if let Some((g, o)) = Self::channel(off) {
            let reg = o % 0x14;
            if reg == 16 { return WriteEffect::NONE; }
            let value = value & match reg { 0 => 0x8000001f, 4 => 0xfffff, 8 => 0x1ffffff, _ => u32::MAX };
            // An HS channel has no PARA_UP: setting DUTY_START starts an update and clearing it
            // cancels one. Other writes take effect at once but start nothing: ESP32's
            // `ledc_ll_set_duty_start` spins while DUTY_START is set, so IDF rewrites CONF1 and CONF0
            // while an update is still pending (IDF v5.5.4 hal/esp32/include/hal/ledc_ll.h:483-489).
            let start = g == 0 && reg == 12 && (self.regs.read(off) ^ value) & (1 << 31) != 0;
            self.regs.write(off, value);
            if g == 0 || reg == 0 && value & 16 != 0 {
                let base = off - reg;
                let local = o - reg;
                for r in [4, 8, 12] { self.groups[g].write(local + r, self.regs.read(base + r)); }
                let para_up = if g == 1 || start { 16 } else { 0 };
                self.groups[g].write(local, self.regs.read(base) | para_up);
                self.regs.write(base, self.regs.read(base) & !16);
                self.dirty |= 1 << (g * 8 + local as usize / 0x14);
            }
        } else if let Some((g, o)) = Self::timer(off) {
            if o % 8 == 0 {
                self.regs.write(off, value & 0x07ffffff & !(1 << 26));
                if g == 0 || value & (1 << 26) != 0 {
                    self.groups[g].write(o, value | (1 << 26));
                    if value & (1 << 24) != 0 { self.groups[g].write(o, (value & !(1 << 24)) | (1 << 26)); }
                }
            }
        } else if matches!(off, 0x188 | 0x18c) {
            let target = if off == 0x188 { 0xc8 } else { 0xcc };
            self.groups[0].write(target, (value & 15) | ((value >> 4) & 0xff0));
            self.groups[1].write(target, ((value >> 4) & 15) | ((value >> 12) & 0xff0));
        } else if !matches!(off, 0x180 | 0x184) { self.regs.write(off, value); }
        WriteEffect::NONE
    }
    fn irq_sources(&self) -> u64 { self.groups.iter().any(|g| g.irq_sources() != 0) as u64 }
    fn clock(&self) -> Option<ClockDomain> {
        (self.clock_enabled && self.groups.iter().any(|g| g.any_timer())).then_some(ClockDomain::Apb)
    }
    fn tick(&mut self, ticks: u64) {
        #[cfg(test)] { self.tick_calls += 1; }
        if !self.clock_enabled { return; }
        for (g, group) in self.groups.iter_mut().enumerate() {
            let apb = g == 0 || self.regs.read(0x190) & 1 != 0;
            let before: [u32; 8] = std::array::from_fn(|ch| group.read(ch as u32 * 0x14 + 12));
            // A fade moves the duty between its start and its end: a step to or from 0 flips the level.
            let fading = group.fading();
            let duty: [u32; 8] = if fading { std::array::from_fn(|ch| group.read(ch as u32 * 0x14 + 16)) } else { [0; 8] };
            group.tick_with_source(ticks, |_, conf| Self::source(apb, conf));
            for ch in 0..8 {
                let base = g as u32 * 0xa0 + ch * 0x14;
                if fading && group.read(ch * 0x14 + 16) != duty[ch as usize] { self.dirty |= 1 << (g * 8 + ch as usize); }
                if before[ch as usize] & (1 << 31) != 0 && group.read(ch * 0x14 + 12) & (1 << 31) == 0 {
                    self.regs.write(base + 12, self.regs.read(base + 12) & !(1 << 31));
                    self.dirty |= 1 << (g * 8 + ch as usize);
                }
            }
        }
    }
    fn has_deadline(&self) -> bool { true }
    fn next_deadline(&self) -> Option<u64> {
        if !self.clock_enabled { return None; }
        self.groups.iter().enumerate().filter_map(|(g, group)| {
            let apb = g == 0 || self.regs.read(0x190) & 1 != 0;
            group.next_wrap(|_, conf| Self::source(apb, conf))
        }).min()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn configure(ledc: &mut ClassicLedc, channel: usize, timer: usize, low_speed: bool) {
        ledc.clock_enabled = true;
        if low_speed {
            ledc.write(0x190, 1);
        }
        let timer_off = if low_speed { 0x160 } else { 0x140 } + timer as u32 * 8;
        let channel_off = if low_speed { 0xa0 } else { 0 } + channel as u32 * 0x14;
        ledc.write(
            timer_off,
            8 | (16000 << 5) | (1 << 25) | u32::from(low_speed) << 26,
        );
        ledc.write(channel_off + 8, 64 << 4);
        ledc.write(
            channel_off + 12,
            (1 << 31) | (1 << 30) | (1 << 20) | (1 << 10),
        );
        ledc.write(channel_off, timer as u32 | 4 | u32::from(low_speed) << 4);
    }

    #[test]
    fn timer_reset_restarts_without_leaving_the_timer_stopped() {
        let mut ledc = ClassicLedc::new();
        configure(&mut ledc, 0, 0, false);
        ledc.tick(8000);
        assert_eq!(ledc.read(0x144), 128);
        ledc.write(0x140, 8 | (16000 << 5) | (1 << 25) | (1 << 24));
        assert_eq!(ledc.read(0x144), 0);
        ledc.tick(16000);
        assert_eq!(ledc.read(0x10), 64 << 4);
    }

    #[test]
    fn high_and_low_speed_channels_latch_duty_and_report_pwm() {
        let mut ledc = ClassicLedc::new();
        assert_eq!(ledc.clock(), None);
        configure(&mut ledc, 0, 0, false);
        assert_eq!(ledc.clock(), Some(ClockDomain::Apb));
        configure(&mut ledc, 0, 0, true);
        assert_eq!(ledc.read(0x10), 0);
        assert_eq!(ledc.read(0xb0), 0);
        ledc.tick(15999);
        assert_eq!(ledc.read(0x10), 0);
        ledc.tick(1);
        assert_eq!(ledc.read(0x10), 64 << 4);
        assert_eq!(ledc.read(0xb0), 64 << 4);
        for signal in [71, 79] {
            let (hz, duty) = ledc.pwm(signal).unwrap();
            assert!((hz - 5000.0).abs() < 0.001);
            assert_eq!(duty, 16384);
        }
    }

    #[test]
    fn low_speed_shadow_registers_require_para_up() {
        let mut ledc = ClassicLedc::new();
        ledc.clock_enabled = true;
        ledc.write(0x190, 1);
        let timer = 8 | (16000 << 5) | (1 << 25);
        let duty_start = (1 << 31) | (1 << 30) | (1 << 20) | (1 << 10);
        ledc.write(0x160, timer);
        ledc.write(0xa8, 64 << 4);
        ledc.write(0xac, duty_start);
        ledc.write(0xa0, 4 | (1 << 4));
        ledc.tick(100_000);
        assert!(ledc.pwm(79).is_none(), "timer shadow has not been latched");
        assert_eq!(ledc.read(0xb0), 0);
        ledc.write(0x160, timer | (1 << 26));
        ledc.tick(16000);
        assert_eq!(ledc.read(0xb0), 64 << 4);
        ledc.write(0xa8, 128 << 4);
        ledc.write(0xac, duty_start);
        ledc.tick(16000);
        assert_eq!(ledc.read(0xb0), 64 << 4, "channel shadow has not been latched");
        ledc.write(0xa0, 4 | (1 << 4));
        assert_eq!(ledc.read(0xa0) & (1 << 4), 0);
        assert_eq!(ledc.read(0x160) & (1 << 26), 0);
        ledc.tick(16000);
        assert_eq!(ledc.read(0xb0), 128 << 4);
    }

    #[test]
    fn clearing_duty_start_before_the_wrap_cancels_a_pending_update() {
        let mut ledc = ClassicLedc::new();
        configure(&mut ledc, 0, 0, false);
        ledc.tick(16000);
        assert_eq!(ledc.read(0x10), 64 << 4);
        ledc.write(0x08, 128 << 4);
        ledc.write(0x0c, (1 << 31) | (1 << 30) | (1 << 20) | (1 << 10));
        ledc.write(0x0c, (1 << 30) | (1 << 20) | (1 << 10));
        ledc.tick(16000);
        assert_eq!(ledc.read(0x10), 64 << 4);
    }

    #[test]
    fn low_speed_clock_selection_uses_idf_rc_fast_and_ref_tick() {
        let mut ledc = ClassicLedc::new();
        configure(&mut ledc, 0, 0, true);
        ledc.tick(16000);
        ledc.write(0x190, 0);
        assert_eq!(ledc.pwm(79).unwrap().0, 531.25);
        ledc.write(0x160, 8 | (16000 << 5) | (1 << 26));
        assert_eq!(ledc.pwm(79).unwrap().0, 62.5);
    }

    /// An HS fade (`ledc_set_fade_with_step`: DUTY_START written last, no PARA_UP on the HS group)
    /// steps the duty every DUTY_CYCLE periods and raises DUTY_CHNG_END_HSCH0 (INT_RAW bit 8).
    #[test]
    fn high_speed_fade_steps_and_ends_with_the_interrupt() {
        let mut ledc = ClassicLedc::new();
        ledc.clock_enabled = true;
        ledc.write(0x140, 8 | (16000 << 5) | (1 << 25));          // 5 kHz from APB: 16000 ticks a period
        ledc.write(0x188, 1 << 8);
        ledc.write(0x08, 0);
        ledc.write(0x0c, (1 << 30) | (3 << 20) | (2 << 10) | 10);   // up 3 x 10, every 2 periods
        ledc.write(0x00, 4);
        ledc.write(0x0c, (1 << 31) | (1 << 30) | (3 << 20) | (2 << 10) | 10);
        assert_eq!(ledc.next_deadline(), Some(16000));
        ledc.tick(16000);
        assert_eq!(ledc.read(0x10), 0);
        ledc.take_signal_updates();
        ledc.tick(2 * 16000);
        assert_eq!(ledc.read(0x10), 10 << 4);
        assert_eq!(ledc.take_signal_updates(), 1);
        assert_eq!(ledc.pwm(71).unwrap().1, (160 * 65535 + 2048) / 4096);
        ledc.tick(3 * 16000);
        assert_eq!((ledc.read(0x10), ledc.irq_sources()), (20 << 4, 0));
        ledc.tick(16000);
        assert_eq!((ledc.read(0x10), ledc.irq_sources()), (30 << 4, 1));
        assert_eq!(ledc.read(0x0c) >> 31, 0);
        assert_eq!(ledc.next_deadline(), None);
    }

    /// IDF writes the fade parameters with a read-modify-write of CONF1 while the plain update that
    /// `ledc_channel_config` started still holds DUTY_START, then spins until it clears: the plain
    /// update completes at its wrap, and only the next DUTY_START starts the fade.
    #[test]
    fn conf1_rewritten_with_duty_start_held_does_not_restart() {
        let mut ledc = ClassicLedc::new();
        ledc.clock_enabled = true;
        ledc.write(0x140, 8 | (16000 << 5) | (1 << 25));
        ledc.write(0x0c, (1 << 31) | (1 << 30) | (1 << 20) | (1 << 10));  // plain update of duty 0
        ledc.write(0x00, 4);
        ledc.write(0x08, 0);
        ledc.write(0x0c, (1 << 31) | (1 << 30) | (3 << 20) | (1 << 10) | 10);
        assert_eq!(ledc.read(0x0c) >> 31, 1);
        ledc.tick(16000);
        assert_eq!((ledc.read(0x0c) >> 31, ledc.read(0x10)), (0, 0), "the plain update ends, no fade ran");
        ledc.write(0x0c, (1 << 31) | (1 << 30) | (3 << 20) | (1 << 10) | 10);
        ledc.tick(4 * 16000);
        assert_eq!((ledc.read(0x0c) >> 31, ledc.read(0x10)), (0, 30 << 4));
    }

    /// The HS timer 0 overflow interrupt asks for each wrap; shrinking the timer leaves its phase
    /// past the new period, so the next wrap is one tick away, not an underflow.
    #[test]
    fn shrinking_a_timer_past_its_phase_wraps_at_the_next_tick() {
        let mut ledc = ClassicLedc::new();
        ledc.clock_enabled = true;
        ledc.write(0x140, 14 | (256 << 5) | (1 << 25));          // 14-bit, divider 1, APB
        ledc.write(0x188, 1);
        ledc.tick(10_000);
        ledc.write(0x140, 1 | (256 << 5) | (1 << 25));
        assert_eq!(ledc.next_deadline(), Some(1));
        ledc.tick(1);
        assert!(ledc.read(0x144) < 2, "the counter is back inside the period");
    }

    #[test]
    fn interrupt_status_and_clear_cover_timer_and_duty_completion() {
        let mut ledc = ClassicLedc::new();
        configure(&mut ledc, 0, 0, false);
        ledc.write(0x188, (1 << 8) | 1);
        assert_eq!(ledc.irq_sources(), 0);
        ledc.tick(16000);
        assert_eq!(ledc.read(0x180) & 0x101, 0x101);
        assert_eq!(ledc.read(0x184) & 0x101, 0x101);
        assert_eq!(ledc.irq_sources(), 1);
        ledc.write(0x18c, 0x101);
        assert_eq!(ledc.irq_sources(), 0);
    }
}
