//! Classic ESP32 timer-group layout: T0/T1 and LACT.
//! ESP-IDF v5.5.4 components/soc/esp32/register/soc/timer_group_reg.h:
//! T0 edge/level bits 12/11 (42-53), LACT enable/direction/reload/divider (397-420),
//! alarm enable bit 10 (433-438). Timing and reset completion are inferred.
use emu_core::ClockDomain;
use esp_periph::{Device, TimerGroup, WriteEffect};
use esp_periph::timg::Timer;

pub struct ClassicTimer {
    pub timer: TimerGroup,
    lact: Timer,
    lact_rtc: u32,
}

impl ClassicTimer {
    pub fn new(_group: usize) -> Self {
        Self { timer: TimerGroup::new(), lact: Timer::default(), lact_rtc: 0 }
    }

    fn irq_bits(&self) -> u64 {
        let pending = self.timer.int_raw & self.timer.int_ena;
        let mut bits = 0;
        for n in 0..2 {
            if pending & (1 << n) != 0 {
                let config = self.timer.t[n].config;
                if config & (1 << 11) != 0 {
                    bits |= 1 << n;
                }
                if config & (1 << 12) != 0 {
                    bits |= 1 << (4 + n);
                }
            }
        }
        if pending & (1 << 3) != 0 {
            if self.lact.config & (1 << 11) != 0 {
                bits |= 1 << 3;
            }
            if self.lact.config & (1 << 12) != 0 {
                bits |= 1 << 7;
            }
        }
        bits
    }
}

impl Device for ClassicTimer {
    fn read(&mut self, off: u32) -> u32 {
        match off {
            0x00..=0x47 => self.timer.t[(off / 0x24) as usize].read(off % 0x24),
            0x70 => self.lact.config,
            0x74 => self.lact_rtc,
            0x78..=0x94 => self.lact.read(off - 0x74),
            0x98 => self.timer.int_ena,
            0x9c => self.timer.int_raw,
            0xa0 => self.timer.int_raw & self.timer.int_ena,
            _ => self.timer.read(off),
        }
    }

    fn write(&mut self, off: u32, value: u32) -> WriteEffect {
        match off {
            0x00..=0x47 => self.timer.t[(off / 0x24) as usize].write(off % 0x24, value, u64::MAX),
            0x70 => self.lact.config = value,
            0x74 => self.lact_rtc = value,
            0x78..=0x94 => self.lact.write(off - 0x74, value, u64::MAX),
            0x98 => self.timer.int_ena = value,
            0xa4 => self.timer.int_raw &= !value,
            _ => self.timer.write(off, value),
        }
        WriteEffect::NONE
    }

    fn irq_sources(&self) -> u64 {
        self.irq_bits()
    }

    fn clock(&self) -> Option<ClockDomain> {
        Some(ClockDomain::Apb)
    }

    fn tick(&mut self, ticks: u64) {
        for (i, timer) in self.timer.t.iter_mut().enumerate() {
            if timer.step(ticks, u64::MAX) { self.timer.int_raw |= 1 << i; }
        }
        if self.lact.step(ticks, u64::MAX) { self.timer.int_raw |= 1 << 3; }
    }

    fn has_deadline(&self) -> bool {
        true
    }

    fn next_deadline(&self) -> Option<u64> {
        [
            <TimerGroup as Device>::next_deadline(&self.timer),
            self.lact.deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_classic_timers_keep_full_width_and_share_alarm_semantics() {
        for (config, load, trigger, latch, count, alarm) in [
            (0, 0x18, 0x20, 0xc, 4, 0x10),
            (0x24, 0x3c, 0x44, 0x30, 0x28, 0x34),
            (0x70, 0x8c, 0x94, 0x80, 0x78, 0x84),
        ] {
            let mut t = ClassicTimer::new(0);
            t.write(load, u32::MAX - 1);
            t.write(load + 4, 0x80000000);
            t.write(trigger, 1);
            t.write(alarm, 1);
            t.write(alarm + 4, 0x80000001);
            t.write(config, (1 << 31) | (1 << 30) | (1 << 10) | (2 << 13));
            assert_eq!(t.next_deadline(), Some(6));
            t.tick(6);
            t.write(latch, 1);
            assert_eq!(t.read(count), 1);
            assert_eq!(t.read(count + 4), 0x80000001);
            assert_ne!(t.read(0x9c), 0);
            assert_eq!(t.next_deadline(), None);
        }
    }
}
