//! LEDC0 and LEDC1: eight low-speed channels and four timers each (soc/esp32s31 ledc_reg.h,
//! reg_base.h DR_REG_LEDC0_BASE / DR_REG_LEDC1_BASE; IDF v6.1 esp_hal_ledc/esp32s31 hal/ledc_ll.h).
//!
//! Modelled: the duty in effect (DUTY_R) and its hardware fades. A duty update (DUTY_START,
//! CONF1 bit 31) loads DUTY as the starting duty and runs the channel's gamma ranges
//! (GAMMA_CONF ENTRY_NUM at 0x100 + 4n; range words at 0x400 + 0x40n: DUTY_INC bit 0,
//! DUTY_CYCLE bits 10:1, SCALE bits 20:11, DUTY_NUM bits 30:21): every DUTY_CYCLE PWM periods
//! the duty moves by SCALE, DUTY_NUM times per range, and the last range raises
//! DUTY_CHNG_END_CHn (INT_RAW bit 4 + n). IDF's `ledc_update_duty` for a plain duty is such an
//! update too (one range, DUTY_NUM 1, SCALE 0), and its fade ISR waits for that interrupt
//! (`ledc_fade_start(.., LEDC_FADE_WAIT_DONE)`).
//!
//! The PWM period comes from the channel's timer (TIMERn_CONF: DUTY_RES bits 4:0, CLK_DIV
//! bits 22:5 with 8 fraction bits, PAUSE bit 23, RST bit 24) and the group's source clock
//! (HP_SYS_CLKRST LEDC_CTRL0 LEDCn_CLK_SRC_SEL: 0 XTAL, 1 RC_FAST, 2 PLL_F80M; `ledc_ll_set_slow_clk_sel`).
//! A paused or reset timer holds the fade. Not modelled: the waveform itself (a board sees a
//! pin on or off), HPOINT, the overflow counter and its interrupt, ETM, sleep retention.
//! Everything else is register storage.

/// LEDC0, LEDC1.
pub const BASE: [u32; 2] = [0x2039_2000, 0x2039_c000];
/// HP_SYS_CLKRST LEDC_CTRL0 (hp_sys_clkrst_reg.h, DR_REG_HP_SYS_CLKRST_BASE 0x2058_7000 + 0x148).
pub const CLK_CTRL: u32 = 0x2058_7148;
/// INT_RAW, INT_ST, INT_ENA, INT_CLR.
const INT_RAW: u32 = 0xc0;
const INT_ST: u32 = 0xc4;
const INT_ENA: u32 = 0xc8;
const INT_CLR: u32 = 0xcc;
const GAMMA_CONF: u32 = 0x100;
const GAMMA_RAM: u32 = 0x400;
/// The duty register's fraction bits (DUTY_CH0 is duty << 4, `ledc_ll_set_duty_int_part`).
const FRACTION: u32 = 4;
const DUTY_MASK: u32 = 0x1ff_ffff;

/// The group whose register block holds `address`.
pub fn group(address: u32) -> Option<usize> { BASE.iter().position(|&b| address & !0xfff == b) }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fade {
    /// the range running, of `ranges`
    range: u32,
    ranges: u32,
    /// steps left in this range
    steps: u32,
    /// CPU cycles run since the last step
    elapsed: u64,
}

#[derive(Clone)]
pub struct Group {
    regs: Vec<u32>,
    duty_r: [u32; 8],
    fade: [Option<Fade>; 8],
    raw: u32,
}

impl Default for Group {
    fn default() -> Self { Self { regs: vec![0; 0x400], duty_r: [0; 8], fade: [None; 8], raw: 0 } }
}

impl Group {
    fn reg(&self, off: u32) -> u32 { self.regs[(off as usize >> 2) & 0x3ff] }

    pub fn read(&self, off: u32) -> u32 {
        match off {
            o if o < 8 * 0x14 && o % 0x14 == 0x10 => self.duty_r[(o / 0x14) as usize],
            INT_RAW => self.raw,
            INT_ST => self.raw & self.reg(INT_ENA),
            INT_CLR => 0,
            _ => self.reg(off),
        }
    }

    pub fn write(&mut self, off: u32, value: u32) {
        let idx = (off as usize >> 2) & 0x3ff;
        match off {
            INT_RAW | INT_ST => {}
            INT_CLR => self.raw &= !value,
            o if o < 8 * 0x14 => {
                let ch = (o / 0x14) as usize;
                match o % 0x14 {
                    // CONF0: PARA_UP (bit 4) clears itself. A plain update without DUTY_START
                    // puts DUTY in effect, unless an update is already running.
                    0x0 => {
                        self.regs[idx] = value & !(1 << 4);
                        if value & 1 << 4 != 0 && self.fade[ch].is_none() { self.duty_r[ch] = self.reg(o + 8) & DUTY_MASK; }
                    }
                    // CONF1: DUTY_START (bit 31) clears itself and starts the duty change.
                    0xc => {
                        self.regs[idx] = value & !(1 << 31);
                        if value & 1 << 31 != 0 { self.start(ch); }
                    }
                    0x10 => {}
                    _ => self.regs[idx] = value,
                }
            }
            // TIMERn_CONF: PARA_UP (bit 26) clears itself.
            o if (0xa0..0xc0).contains(&o) && o % 8 == 0 => self.regs[idx] = value & !(1 << 26),
            _ => self.regs[idx] = value,
        }
    }

    fn start(&mut self, ch: usize) {
        let base = ch as u32 * 0x14;
        self.duty_r[ch] = self.reg(base + 8) & DUTY_MASK;
        let ranges = (self.reg(GAMMA_CONF + ch as u32 * 4) & 0x1f).max(1);
        self.fade[ch] = Some(Fade { range: 0, ranges, steps: self.range(ch, 0).3, elapsed: 0 });
        self.finish_empty_ranges(ch);
    }

    /// (increase, cycles, scale, steps) of range `r`.
    fn range(&self, ch: usize, r: u32) -> (bool, u32, u32, u32) {
        let w = self.reg(GAMMA_RAM + ch as u32 * 0x40 + r * 4);
        (w & 1 != 0, (w >> 1) & 0x3ff, (w >> 11) & 0x3ff, (w >> 21) & 0x3ff)
    }

    /// Moves past ranges with no steps left; the last one ends the change.
    fn finish_empty_ranges(&mut self, ch: usize) {
        while let Some(f) = self.fade[ch] {
            if f.steps != 0 { return; }
            if f.range + 1 < f.ranges {
                let next = f.range + 1;
                self.fade[ch] = Some(Fade { range: next, steps: self.range(ch, next).3, elapsed: 0, ..f });
            } else {
                self.fade[ch] = None;
                self.raw |= 1 << (4 + ch);
            }
        }
    }

    /// CPU cycles per PWM period of the channel's timer, None while it is paused, held in
    /// reset or unclocked.
    fn period_cycles(&self, ch: usize, source_hz: u64, cpu_hz: u64) -> Option<u64> {
        let timer = self.reg(ch as u32 * 0x14) & 3;
        let conf = self.reg(0xa0 + timer * 8);
        if conf & (3 << 23) != 0 || source_hz == 0 { return None; }
        let (res, div) = (conf & 0x1f, u64::from((conf >> 5) & 0x3_ffff));
        if res == 0 || div < 256 { return None; }
        let cycles = (u128::from(cpu_hz) * (1u128 << res) * u128::from(div)).div_ceil(256 * u128::from(source_hz));
        Some((cycles as u64).max(1))
    }

    /// Runs the duty changes for `cycles` CPU cycles; true when one ended.
    pub fn advance(&mut self, cycles: u64, source_hz: u64, cpu_hz: u64) -> bool {
        let raw = self.raw;
        for ch in 0..8 {
            if self.fade[ch].is_none() { continue; }
            let Some(period) = self.period_cycles(ch, source_hz, cpu_hz) else { continue };
            // Cycles left over at the end of a range carry into the next one.
            let mut budget = cycles;
            while let Some(mut f) = self.fade[ch] {
                let (inc, cyc, scale, _) = self.range(ch, f.range);
                let every = period * u64::from(cyc.max(1));
                f.elapsed += std::mem::take(&mut budget);
                let due = (f.elapsed / every).min(u64::from(f.steps)) as u32;
                if due == 0 { self.fade[ch] = Some(f); break; }
                let delta = u64::from(scale << FRACTION) * u64::from(due);
                let duty = u64::from(self.duty_r[ch]);
                self.duty_r[ch] = if inc { (duty + delta).min(u64::from(DUTY_MASK)) } else { duty.saturating_sub(delta) } as u32;
                f.elapsed -= every * u64::from(due);
                f.steps -= due;
                if f.steps != 0 { self.fade[ch] = Some(f); break; }
                budget = std::mem::take(&mut f.elapsed);
                self.fade[ch] = Some(f);
                self.finish_empty_ranges(ch);
            }
        }
        self.raw != raw
    }

    /// CPU cycles until the next duty step of a running change.
    pub fn cycles_until_step(&self, source_hz: u64, cpu_hz: u64) -> Option<u64> {
        (0..8).filter_map(|ch| {
            let f = self.fade[ch]?;
            let every = self.period_cycles(ch, source_hz, cpu_hz)? * u64::from(self.range(ch, f.range).1.max(1));
            Some(every.saturating_sub(f.elapsed).max(1))
        }).min()
    }

    pub fn irq(&self) -> bool { self.raw & self.reg(INT_ENA) != 0 }
    pub fn running(&self) -> bool { self.fade.iter().any(Option::is_some) }
}

/// Both groups and the source clock their timers count.
#[derive(Clone, Default)]
pub struct Ledc {
    pub groups: [Group; 2],
}

impl Ledc {
    /// The group's source clock from HP_SYS_CLKRST LEDC_CTRL0 (`clk_ctrl`).
    pub fn source_hz(group: usize, clk_ctrl: u32, rc_fast_hz: u64) -> u64 {
        match (clk_ctrl >> if group == 0 { 3 } else { 9 }) & 3 { 0 => 40_000_000, 1 => rc_fast_hz, 2 => 80_000_000, _ => 0 }
    }
    pub fn running(&self) -> bool { self.groups.iter().any(Group::running) }
    pub fn irqs(&self) -> [bool; 2] { [self.groups[0].irq(), self.groups[1].irq()] }
}

#[cfg(test)]
mod tests {
    use super::*;
    const CPU: u64 = 320_000_000;

    /// Timer 1 at 8 bits and 100 kHz from PLL_F80M (divider 80 MHz / 256 / 100 kHz = 3.125,
    /// 800 in 10.8 fixed point), channel 1 on it: the Mosaico BSP's VCC_3V3 soft start.
    fn ramp_group() -> Group {
        let mut g = Group::default();
        g.write(0xa0 + 8, 8 | (800 << 5) | (1 << 26));
        g.write(0x14, 1 | (1 << 2));
        g
    }

    /// `ledc_set_fade_with_time(0 -> 255, 50 ms)` at 100 kHz: 5000 periods over 255 steps of 1
    /// gives DUTY_CYCLE 19, so 255 x 19 periods = 48.45 ms; the duty climbs one LSB per 19
    /// periods and DUTY_CHNG_END_CH1 (bit 5) rises with the last step.
    #[test]
    fn fade_steps_the_duty_and_ends_with_the_interrupt() {
        let mut g = ramp_group();
        let period = g.period_cycles(1, 80_000_000, CPU).unwrap();
        assert_eq!(period, 3200, "10 us at 320 MHz");
        g.write(INT_ENA, 1 << 5);
        g.write(0x14 + 8, 0);
        g.write(GAMMA_CONF + 4, 1);
        g.write(GAMMA_RAM + 0x40, 1 | (19 << 1) | (1 << 11) | (255 << 21));
        g.write(0x14 + 0xc, 1 << 31);
        assert_eq!(g.read(0x14 + 0xc) >> 31, 0, "DUTY_START clears itself");
        g.write(0x14, 1 | (1 << 2) | (1 << 4));                     // PARA_UP: does not restart
        assert!(g.running());
        assert!(!g.advance(19 * period - 1, 80_000_000, CPU));
        assert_eq!(g.read(0x14 + 0x10), 0);
        assert!(!g.advance(1, 80_000_000, CPU));
        assert_eq!(g.read(0x14 + 0x10), 1 << FRACTION);
        assert_eq!(g.cycles_until_step(80_000_000, CPU), Some(19 * period));
        assert!(!g.advance(253 * 19 * period, 80_000_000, CPU));
        assert_eq!(g.read(0x14 + 0x10), 254 << FRACTION);
        assert!(!g.irq());
        assert!(g.advance(19 * period, 80_000_000, CPU));
        assert_eq!(g.read(0x14 + 0x10), 255 << FRACTION);
        assert!(g.irq() && !g.running());
        assert_eq!(g.read(INT_ST), 1 << 5);
        g.write(INT_CLR, 1 << 5);
        assert!(!g.irq());
    }

    /// A plain duty update (`ledc_update_duty`: one range, DUTY_NUM 1, SCALE 0) puts the duty in
    /// effect at once and ends one period later; two ranges run in order, down then up.
    #[test]
    fn plain_update_and_multi_range_fade() {
        let mut g = ramp_group();
        let period = g.period_cycles(1, 80_000_000, CPU).unwrap();
        g.write(0x14 + 8, 100 << FRACTION);
        g.write(GAMMA_CONF + 4, 1);
        g.write(GAMMA_RAM + 0x40, 1 | (1 << 1) | (1 << 21));
        g.write(0x14 + 0xc, 1 << 31);
        assert_eq!(g.read(0x14 + 0x10), 100 << FRACTION);
        assert!(g.advance(period, 80_000_000, CPU));
        assert_eq!(g.read(0x14 + 0x10), 100 << FRACTION);
        assert_eq!(g.read(INT_RAW), 1 << 5);
        g.write(INT_CLR, 1 << 5);
        g.write(GAMMA_CONF + 4, 2);
        g.write(GAMMA_RAM + 0x40, (2 << 1) | (10 << 11) | (3 << 21));   // down 3 x 10, every 2 periods
        g.write(GAMMA_RAM + 0x44, 1 | (1 << 1) | (5 << 11) | (2 << 21)); // up 2 x 5, every period
        g.write(0x14 + 0xc, 1 << 31);
        assert!(!g.advance(6 * period, 80_000_000, CPU));
        assert_eq!(g.read(0x14 + 0x10), 70 << FRACTION);
        assert!(g.advance(2 * period, 80_000_000, CPU));
        assert_eq!(g.read(0x14 + 0x10), 80 << FRACTION);
        // all in one batch, from the start again
        g.write(INT_CLR, 1 << 5);
        g.write(0x14 + 8, 100 << FRACTION);
        g.write(0x14 + 0xc, 1 << 31);
        assert!(g.advance(100 * period, 80_000_000, CPU));
        assert_eq!(g.read(0x14 + 0x10), 80 << FRACTION);
    }

    /// A paused timer holds the change; the source clock follows LEDCn_CLK_SRC_SEL.
    #[test]
    fn paused_timer_holds_and_source_select() {
        let mut g = ramp_group();
        g.write(0x14 + 8, 0);
        g.write(GAMMA_CONF + 4, 1);
        g.write(GAMMA_RAM + 0x40, 1 | (1 << 1) | (1 << 11) | (4 << 21));
        g.write(0x14 + 0xc, 1 << 31);
        g.write(0xa0 + 8, 8 | (800 << 5) | (1 << 23));
        assert!(!g.advance(1 << 30, 80_000_000, CPU));
        assert_eq!(g.read(0x14 + 0x10), 0);
        assert_eq!(g.cycles_until_step(80_000_000, CPU), None);
        g.write(0xa0 + 8, 8 | (800 << 5));
        assert!(g.advance(4 * 3200, 80_000_000, CPU));
        assert_eq!(g.read(0x14 + 0x10), 4 << FRACTION);
        assert_eq!(Ledc::source_hz(0, 2 << 3, 17_500_000), 80_000_000);
        assert_eq!(Ledc::source_hz(1, 1 << 9, 17_500_000), 17_500_000);
        assert_eq!(Ledc::source_hz(1, 2 << 3, 17_500_000), 40_000_000);
    }
}
