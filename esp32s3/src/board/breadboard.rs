//! `breadboard` — a bare ESP32-S3 module on a breadboard: nothing is soldered to it, so the host
//! (a page drawing whatever the user wired: LEDs, buttons, …) needs every pin's state rather than a
//! fixed set of board devices.
//!
//! Every GPIO 0–48 is one entry of the `leds()` strip, so a page gets it through the existing `ring`
//! message with no new protocol:
//!
//! - red: the pin's duty over the last 20 ms window, 0–255: the fraction of the window it was
//!   driven high (from `gpio_output_at`), or, for a pin driven by LEDC/MCPWM (which synthesize no
//!   edges), the peripheral's own duty sampled at the window's end;
//! - green: 1 while the pin's output driver is enabled, 0 for an input;
//! - blue: the level last driven on the pin as an output (1 = high), so a static pin is exact.
//!
//! The strip is republished only when a window changes something. Inputs come from the host through
//! the `gpio` (or `btn`) message.
use esp_soc::board::{BoardModel, VirtualCycle};

pub const PINS: usize = 49;
const CPU_HZ: u64 = 240_000_000;
/// The duty window: 20 ms, so at most 50 strip updates per emulated second.
pub const WINDOW: u64 = CPU_HZ / 50;

type Pwm<'a> = &'a dyn Fn(u8) -> Option<(f64, u32)>;

pub struct Breadboard {
    /// cycle up to which `acc` is counted
    last: u64,
    enabled: u64,
    output: u64,
    window_start: u64,
    acc: [u64; PINS],
    /// LEDC/MCPWM duty (0–65535) per pin, sampled at the last window end
    pwm: [Option<u32>; PINS],
    strip: [[u8; 3]; PINS],
    updates: u64,
    gpio_events: u64,
}
impl Default for Breadboard { fn default() -> Self { Self::new() } }
impl Breadboard {
    pub fn new() -> Self {
        Breadboard { last: 0, enabled: 0, output: 0, window_start: 0, acc: [0; PINS], pwm: [None; PINS], strip: [[0; 3]; PINS], updates: 0, gpio_events: 0 }
    }
    fn high(&self) -> u64 { self.enabled & self.output }
    /// Count high time up to `cycle`.
    fn settle(&mut self, cycle: u64) {
        if cycle <= self.last { return; }
        let dt = cycle - self.last;
        let high = self.high();
        for (p, acc) in self.acc.iter_mut().enumerate() { if high >> p & 1 != 0 { *acc += dt; } }
        self.last = cycle;
    }
    /// Close every window that ended by `cycle`. `pwm` gives the peripheral duty per pin at that
    /// moment when the caller has it; otherwise the last sample stands in.
    fn close_windows(&mut self, cycle: u64, pwm: Option<Pwm<'_>>) {
        while cycle >= self.window_start + WINDOW {
            let end = self.window_start + WINDOW;
            self.settle(end);
            if let Some(pwm) = pwm { for (p, slot) in self.pwm.iter_mut().enumerate() { *slot = pwm(p as u8).map(|(_, d)| d); } }
            let high = self.high();
            let mut changed = false;
            let enabled = self.enabled;
            for (p, ((acc, pwm), strip)) in self.acc.iter_mut().zip(&self.pwm).zip(self.strip.iter_mut()).enumerate() {
                let edge = ((*acc).min(WINDOW) * 255 + WINDOW / 2) / WINDOW;
                let duty = match pwm { Some(d) => ((*d as u64 * 255 + 32_767) / 65_535) as u8, None => edge as u8 };
                let entry = [duty, (enabled >> p & 1) as u8, (high >> p & 1) as u8];
                if *strip != entry { *strip = entry; changed = true; }
                *acc = 0;
            }
            if changed { self.updates += 1; }
            self.window_start = end;
        }
    }
}
impl BoardModel for Breadboard {
    fn name(&self) -> &'static str { "breadboard" }
    fn gpio_changes(&mut self, changes: &[(u8, bool)]) { self.gpio_events += changes.len() as u64; }
    fn gpio_output_at(&mut self, cycle: VirtualCycle, changes: &[(u8, bool)], enabled: u64, output: u64) {
        self.gpio_changes(changes);
        self.close_windows(cycle, None);
        self.settle(cycle);
        self.enabled = enabled;
        self.output = output;
    }
    fn gpio_events(&self) -> u64 { self.gpio_events }
    fn uses_pwm_levels(&self) -> bool { true }
    fn next_deadline(&self) -> Option<VirtualCycle> { Some(self.window_start + WINDOW) }
    fn advance_to(&mut self, cycle: VirtualCycle) { self.close_windows(cycle, None); }
    fn advance_inputs_with_clock(&mut self, cycle: VirtualCycle, clock: &dyn Fn(u8) -> Option<(f64, u32)>) {
        self.close_windows(cycle, Some(clock));
    }
    fn leds(&self) -> Option<(&[[u8; 3]], u64)> { Some((&self.strip, self.updates)) }
    fn report(&self) -> String {
        let on: Vec<String> = self.strip.iter().enumerate().filter(|(_, s)| s[0] > 0).map(|(p, s)| format!("GPIO{}={}", p, s[0])).collect();
        format!("[emu] breadboard: gpio events {}, strip updates {}, duty [{}]", self.gpio_events, self.updates, on.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duty_from_edges_and_pwm() {
        let mut b = Breadboard::new();
        // GPIO2 output, high for the first quarter of the window
        b.gpio_output_at(0, &[(2, true)], 1 << 2, 1 << 2);
        b.gpio_output_at(WINDOW / 4, &[(2, false)], 1 << 2, 0);
        b.advance_inputs_with_clock(WINDOW, &|p| if p == 5 { Some((1000.0, 32_768)) } else { None });
        let (strip, updates) = b.leds().unwrap();
        assert_eq!(strip[2], [64, 1, 0]);
        assert_eq!(strip[5], [128, 0, 0]);
        assert_eq!(updates, 1);
        // GPIO2 stays low through window 2: duty 0, still an output
        let pwm = |p: u8| if p == 5 { Some((1000.0, 32_768)) } else { None };
        b.advance_inputs_with_clock(2 * WINDOW, &pwm);
        assert_eq!(b.leds().unwrap().0[2], [0, 1, 0]);
        assert_eq!(b.leds().unwrap().1, 2);
        // window 3 equals window 2: no republish
        b.advance_inputs_with_clock(3 * WINDOW, &pwm);
        assert_eq!(b.leds().unwrap().1, 2);
        // a window crossed inside gpio_output_at keeps the last PWM sample
        b.gpio_output_at(4 * WINDOW + 1, &[(2, true)], 1 << 2, 1 << 2);
        assert_eq!(b.leds().unwrap().0[5], [128, 0, 0]);
    }
}
