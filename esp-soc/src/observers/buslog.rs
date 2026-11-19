//! `--buslog FILE`: GPIO edges as compact, human/AI-readable text.
//!
//! Vcd writes a standards-compliant waveform for GTKWave; this observer writes the same edges as
//! short lines an LLM can read directly: one line per time slice, several pins per line, long idle
//! gaps folded away. A bit-banged panel (NT7534 / ST77xx) shows its CS/RS/SCK/MOSI activity here
//! without needing a waveform viewer.
//!
//! Example output:
//! ```text
//! # buslog: t=seconds since reset, g<pin>^=high g<pin>v=low
//! 0.021436  g41v g40v g42v
//! 0.021502  g42^ g43^
//! ... 0.184s idle ...
//! 0.210001  g41^
//! ```
use crate::observe::{Ctx, Observer, Wants};
use crate::soc::Soc;

pub struct BusLog {
    path: String,
    cpu_hz: u64,
    /// (cycle, pin, level) in arrival order; sorted before writing.
    events: Vec<(u64, u8, bool)>,
    /// Seconds of no edges above which a gap is folded into one `... idle ...` line.
    idle_fold_s: f64,
    /// Cap on emitted edge lines, so a noisy bus cannot blow up the file.
    max_lines: usize,
}

impl BusLog {
    pub fn new(path: &str, cpu_hz: u64) -> Self {
        Self { path: path.to_string(), cpu_hz, events: Vec::new(), idle_fold_s: 0.05, max_lines: 4000 }
    }
    /// Override the idle-gap folding threshold (seconds).
    pub fn with_idle_fold(mut self, seconds: f64) -> Self { self.idle_fold_s = seconds; self }

    fn secs(&self, cycles: u64) -> f64 { cycles as f64 / self.cpu_hz as f64 }

    /// Record an edge. Inherent (not on the generic trait) so formatting is unit-testable
    /// without naming a concrete SoC type.
    pub fn edge(&mut self, cycle: u64, pin: u8, level: bool) { self.events.push((cycle, pin, level)); }

    /// Render collected edges to `self.path`; returns a one-line summary.
    pub fn render(&mut self) -> String {
        let Ok(f) = std::fs::File::create(&self.path) else {
            return format!("[buslog] cannot write {}", self.path);
        };
        use std::io::Write;
        let mut out = std::io::BufWriter::new(f);
        let _ = writeln!(out, "# buslog: t=seconds since reset, g<pin>^=high g<pin>v=low");
        let _ = writeln!(out, "# {} gpio edges", self.events.len());

        self.events.sort_by_key(|e| e.0);
        let mut lines = 0usize;
        let mut i = 0usize;
        let mut prev_t = 0.0f64;
        while i < self.events.len() && lines < self.max_lines {
            let t = self.secs(self.events[i].0);
            // Fold a long idle gap into a single line.
            if t - prev_t > self.idle_fold_s {
                let _ = writeln!(out, "... {:.3}s idle ...", t - prev_t);
                lines += 1;
                if lines >= self.max_lines { break; }
            }
            // Group every edge sharing this exact cycle onto one line.
            let cycle = self.events[i].0;
            let mut line = format!("{:.6} ", t);
            let mut n = 0usize;
            while i < self.events.len() && self.events[i].0 == cycle && n < 64 {
                let (_, pin, level) = self.events[i];
                line.push_str(&format!(" g{}{}", pin, if level { '^' } else { 'v' }));
                i += 1;
                n += 1;
            }
            let _ = writeln!(out, "{}", line);
            lines += 1;
            prev_t = t;
        }
        if i < self.events.len() {
            let _ = writeln!(out, "# ... {} more edges omitted (raise line cap if needed)", self.events.len() - i);
        }
        format!("[buslog] wrote {} edge lines ({}/{} edges) to {}", lines, i, self.events.len(), self.path)
    }
}

impl<S: Soc> Observer<S> for BusLog {
    fn name(&self) -> &'static str { "buslog" }
    fn wants(&self) -> Wants { Wants::GPIO }
    fn on_gpio(&mut self, cycle: u64, pin: u8, level: bool) { self.edge(cycle, pin, level); }
    fn report(&mut self, _cx: &Ctx) -> String { self.render() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_log(events: &[(u64, u8, bool)], idle: f64) -> String {
        let path = std::env::temp_dir().join(format!("buslog-test-{}-{}.txt", std::process::id(), events.len()));
        let p = path.to_string_lossy().to_string();
        let mut b = BusLog::new(&p, 1_000_000).with_idle_fold(idle);
        for &(c, pin, lvl) in events { b.edge(c, pin, lvl); }
        let summary = b.render();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let _ = std::fs::remove_file(&path);
        assert!(summary.starts_with("[buslog] wrote"), "summary: {summary}");
        text
    }

    #[test]
    fn groups_same_cycle_and_folds_idle() {
        // Two edges at t=0.000001, then a 200 ms gap, then one edge.
        let text = write_log(&[(1, 41, false), (1, 40, false), (1, 42, false), (200_001, 41, true)], 0.05);
        assert!(text.contains("g41v g40v g42v"), "same-cycle edges must share one line:\n{text}");
        assert!(text.contains("idle"), "long gap must be folded:\n{text}");
        assert!(text.contains("g41^"), "later rising edge must appear:\n{text}");
        let edge_lines = text.lines().filter(|l| l.chars().next().unwrap_or('x').is_ascii_digit()).count();
        assert_eq!(edge_lines, 2, "one line per time slice, not per edge:\n{text}");
    }

    #[test]
    fn is_readable_with_a_time_axis() {
        let text = write_log(&[(500_000, 2, true), (500_100, 2, false)], 0.05);
        assert!(text.contains("0.500000"), "timestamp in seconds expected:\n{text}");
        assert!(text.contains("g2^") && text.contains("g2v"), "pin + level markers expected:\n{text}");
        assert!(!text.contains("vcd"), "must not emit VCD syntax:\n{text}");
    }
}
