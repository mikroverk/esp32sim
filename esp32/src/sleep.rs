//! Classic deep sleep only. Sources: ESP-IDF v5.5.4 components/soc/esp32/
//! register/soc/rtc_cntl_reg.h and components/esp_hw_support/port/esp32/rtc_sleep.c.
//! Entry is synchronous at SLEEP_EN; analog sequencing/delays are not simulated.
use crate::bus::SocBus;

impl SocBus {
    #[inline(never)]
    pub(crate) fn enter_deep_sleep(&mut self) {
        let rtc = &self.periph.rtc.0.ram;
        if rtc.read(0x84) & (1 << 31) == 0 {
            return; // DIG_PWC.DG_WRAP_PD_EN: light sleep is not modeled.
        }
        // PWC: SLOWMEM PD_EN/FORCE_PU/FORCE_PD = 17/16/15;
        // FASTMEM = 14/13/12 (rtc_cntl_reg.h:1153-1188).
        // Powered-down RAM becomes zero: deterministic model choice, not silicon contents.
        let pwc = rtc.read(0x80);
        if pwc & (1 << 15) != 0 || pwc & (1 << 17) != 0 && pwc & (1 << 16) == 0 {
            self.rtc_slow.fill(0);
            self.written(0x5000_0000, self.rtc_slow.len());
        }
        if pwc & (1 << 12) != 0 || pwc & (1 << 14) != 0 && pwc & (1 << 13) == 0 {
            self.rtc_fast.fill(0);
            self.written(0x400c_0000, self.rtc_fast.len());
        }
        // The powered-down digital domain cannot keep running a timer watchdog
        // or DMA/radio/PWM engines. Use their existing reset/clock gates once at
        // entry; no sleep condition is added to any device tick.
        self.periph.timg = std::array::from_fn(crate::timers::ClassicTimer::new);
        self.periph.wifi.clock_enabled = false;
        self.periph.ledc.clock_enabled = false;
        self.periph.rmt.clock_enabled = false;
        for i2s in &mut self.periph.i2s {
            i2s.clock_enabled = false;
        }
        self.deep_sleep = true;
        self.ext0_wake();
    }

    /// Called only on entry and GPIO input changes, never from tick/dispatch.
    pub(crate) fn ext0_wake(&mut self) {
        if !self.deep_sleep {
            return;
        }
        let rtc = &mut self.periph.rtc.0;
        // WAKEUP_ENA[22:11], EXT0 trigger bit 0; WAKEUP_CAUSE[10:0].
        // rtc_cntl_reg.h:505-529; soc/rtc.h RTC_EXT0_TRIG_EN.
        if rtc.ram.read(0x34) & (1 << 11) == 0 {
            return;
        }
        let pwc = rtc.ram.read(0x80);
        // RTC peripheral domain must be powered (PWC PD_EN/FORCE_PU/FORCE_PD).
        if pwc & (1 << 18) != 0 || pwc & (1 << 20) != 0 && pwc & (1 << 19) == 0 {
            return;
        }
        let Some(pin) = self.periph.adc.ext0_pad() else {
            return;
        };
        let high = rtc.ram.read(0x60) & (1 << 30) != 0; // EXT_WAKEUP0_LV
        if (self.periph.gpio.gpio.input & (1 << pin) != 0) != high {
            return;
        }
        // rtc_sleep.c:249-270: only GPIO/SDIO can reject on classic, not EXT0.
        // An already active EXT0 level therefore immediately wakes, not SLP_REJECT.
        rtc.ram.write(0x34, (rtc.ram.read(0x34) & !0x7ff) | 1);
        rtc.ram.write(0x18, rtc.ram.read(0x18) & !(1 << 31));
        rtc.reset_cause = 5; // esp_rom/esp32/include/esp32/rom/rtc.h DEEPSLEEP_RESET
        rtc.sw_reset = true;
        self.irq_dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Bus as _;

    #[test]
    fn digital_engines_stop_on_power_down() {
        let mut b = SocBus::new(4 << 20, [0; 6]);
        b.write32(0x3ff0_00c0, (1 << 4) | (1 << 21) | (1 << 11) | (1 << 9))
            .unwrap();
        b.write32(0x3ff0_00cc, 0x406).unwrap();
        b.write32(
            0x3ff5_f000,
            (1 << 31) | (1 << 30) | (1 << 11) | (1 << 10) | (2 << 13),
        )
        .unwrap();
        b.write32(0x3ff5_f010, 10).unwrap();
        b.write32(0x3ff5_f098, 1).unwrap();
        assert!(b.periph.wifi.clock_enabled);
        assert!(b.periph.ledc.clock_enabled);
        assert!(b.periph.rmt.clock_enabled);
        assert!(b.periph.i2s.iter().all(|i| i.clock_enabled));
        b.write32(0x3ff4_8084, 1 << 31).unwrap();
        b.write32(0x3ff4_8018, 1 << 31).unwrap();
        b.tick(4096);
        assert_eq!(b.read32(0x3ff5_f09c).unwrap(), 0);
        assert!(!b.periph.wifi.clock_enabled);
        assert!(!b.periph.ledc.clock_enabled);
        assert!(!b.periph.rmt.clock_enabled);
        assert!(b.periph.i2s.iter().all(|i| !i.clock_enabled));
    }
}
