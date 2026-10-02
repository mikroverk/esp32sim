//! Classic ESP32 timer-group layout: T0/T1, LACT and the main watchdog.
use emu_core::ClockDomain;
use esp_periph::{Device, TimerGroup, WriteEffect};

#[derive(Default)]
struct Lact {
    config: u32,
    rtc: u32,
    count: u64,
    latch: u64,
    alarm: u64,
    load: u64,
    prescale_acc: u64,
}

impl Lact {
    fn divider(&self) -> u64 {
        let divider = ((self.config >> 13) & 0xffff) as u64;
        if divider == 0 {
            65_536
        } else {
            divider
        }
    }

    fn tick(&mut self, apb_ticks: u64, int_raw: &mut u32) {
        if self.config & (1 << 31) == 0 {
            return;
        }
        let divider = self.divider();
        self.prescale_acc += apb_ticks;
        let mut steps = self.prescale_acc / divider;
        self.prescale_acc %= divider;
        if steps == 0 {
            return;
        }

        let increasing = self.config & (1 << 30) != 0;
        let alarm = self.alarm;
        let gap = |count: u64| {
            if increasing {
                alarm.checked_sub(count)
            } else {
                count.checked_sub(alarm)
            }
            .filter(|&distance| distance > 0)
        };
        if self.config & (1 << 10) != 0 {
            if let Some(distance) = gap(self.count).filter(|&distance| distance <= steps) {
                *int_raw |= 1 << 3;
                steps -= distance;
                if self.config & (1 << 29) != 0 {
                    self.count = self.load;
                    if let Some(period) = gap(self.load) {
                        steps %= period;
                    }
                } else {
                    self.count = self.alarm;
                    self.config &= !(1 << 10);
                }
            }
        }
        self.count = if increasing {
            self.count.wrapping_add(steps)
        } else {
            self.count.wrapping_sub(steps)
        };
    }

    fn next_deadline(&self) -> Option<u64> {
        if self.config & ((1 << 31) | (1 << 10)) != (1 << 31) | (1 << 10) {
            return None;
        }
        let steps = if self.config & (1 << 30) != 0 {
            self.alarm.checked_sub(self.count)
        } else {
            self.count.checked_sub(self.alarm)
        }?;
        (steps > 0).then(|| {
            steps
                .saturating_mul(self.divider())
                .saturating_sub(self.prescale_acc)
        })
    }
}

pub struct ClassicTimer {
    pub timer: TimerGroup,
    lact: Lact,
    group: usize,
    wdt_ticks: u64,
    wdt_acc: u64,
    wdt_stage: usize,
    wdt_unlocked: bool,
    wdt_conf: u32,
    reset_cause: Option<u32>,
}

impl ClassicTimer {
    pub fn new(group: usize) -> Self {
        Self {
            timer: TimerGroup::new(),
            lact: Lact::default(),
            group,
            wdt_ticks: 0,
            wdt_acc: 0,
            wdt_stage: 0,
            wdt_unlocked: false,
            wdt_conf: 0,
            reset_cause: None,
        }
    }

    pub fn take_reset(&mut self) -> Option<u32> {
        self.reset_cause.take()
    }

    fn wdt_tick(&mut self, ticks: u64) {
        let conf = self.wdt_conf;
        if conf & (1 << 31) == 0 {
            return;
        }
        let prescale = ((self.timer.read(0x4c) >> 16) as u64).max(1);
        self.wdt_acc += ticks;
        self.wdt_ticks += self.wdt_acc / prescale;
        self.wdt_acc %= prescale;
        while self.wdt_stage < 4 {
            let action = (conf >> (29 - 2 * self.wdt_stage)) & 3;
            if action == 0 {
                self.wdt_stage += 1;
                continue;
            }
            let hold = self.timer.read(0x50 + 4 * self.wdt_stage as u32) as u64;
            if self.wdt_ticks < hold.max(1) {
                break;
            }
            self.wdt_ticks = 0;
            self.wdt_stage += 1;
            match action {
                1 => self.timer.int_raw |= 1 << 2,
                2 => self.reset_cause = Some(if self.group == 0 { 11 } else { 17 }),
                3 => self.reset_cause = Some(if self.group == 0 { 7 } else { 8 }),
                _ => {}
            }
            if self.reset_cause.is_some() {
                break;
            }
        }
        if self.wdt_stage == 4 {
            self.wdt_stage = 0;
        }
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
        if pending & (1 << 2) != 0 {
            if self.wdt_conf & (1 << 21) != 0 {
                bits |= 1 << 2;
            }
            if self.wdt_conf & (1 << 22) != 0 {
                bits |= 1 << 6;
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
            0x70 => self.lact.config,
            0x74 => self.lact.rtc,
            0x78 => self.lact.latch as u32,
            0x7c => (self.lact.latch >> 32) as u32,
            0x84 => self.lact.alarm as u32,
            0x88 => (self.lact.alarm >> 32) as u32,
            0x8c => self.lact.load as u32,
            0x90 => (self.lact.load >> 32) as u32,
            0x98 => self.timer.int_ena,
            0x9c => self.timer.int_raw,
            0xa0 => self.timer.int_raw & self.timer.int_ena,
            _ => self.timer.read(off),
        }
    }

    fn write(&mut self, off: u32, value: u32) -> WriteEffect {
        match off {
            0x48..=0x5c if self.wdt_unlocked => {
                if off == 0x48 {
                    if (self.wdt_conf ^ value) & (1 << 31) != 0 {
                        self.wdt_ticks = 0;
                        self.wdt_acc = 0;
                        self.wdt_stage = 0;
                    }
                    self.wdt_conf = value;
                }
                self.timer.write(off, value);
            }
            0x60 if self.wdt_unlocked => {
                self.wdt_ticks = 0;
                self.wdt_acc = 0;
                self.wdt_stage = 0;
            }
            0x64 => {
                self.wdt_unlocked = value == 0x50d8_3aa1;
                self.timer.write(off, value);
            }
            0x48..=0x60 => {}
            0x70 => self.lact.config = value,
            0x74 => self.lact.rtc = value,
            0x80 => self.lact.latch = self.lact.count,
            0x84 => self.lact.alarm = (self.lact.alarm & !0xffff_ffff) | value as u64,
            0x88 => self.lact.alarm = (self.lact.alarm & 0xffff_ffff) | ((value as u64) << 32),
            0x8c => self.lact.load = (self.lact.load & !0xffff_ffff) | value as u64,
            0x90 => self.lact.load = (self.lact.load & 0xffff_ffff) | ((value as u64) << 32),
            0x94 => self.lact.count = self.lact.load,
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
        self.timer.tick(ticks);
        self.lact.tick(ticks, &mut self.timer.int_raw);
        self.wdt_tick(ticks);
    }

    fn has_deadline(&self) -> bool {
        true
    }

    fn next_deadline(&self) -> Option<u64> {
        [
            <TimerGroup as Device>::next_deadline(&self.timer),
            self.lact.next_deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }
}
