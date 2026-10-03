//! A radio network of emulated ESP32-C6 motes in one process — what `--cooja` does with an
//! external kernel, done here without one, so a browser (or a test) can run a whole 802.15.4
//! network with no simulator behind it.
//!
//! The exactness lives where it already did: `Machine::run_until_cycle` stops at the instruction
//! that starts a transmission, and `SocBus::radio_receive` places a frame on the air at its first
//! preamble byte. This module only owns the clock and the medium:
//!
//! - every node advances to the end of a common slice (`slice_ns`, 100 µs by default), a
//!   transmission cutting a node's slice short at the cycle of the `TX_START` write;
//! - the frames a slice produced are then handed to every other node in range, with
//!   `started_ago` set from how far that receiver has run past the transmission, so the frame
//!   lands on the air where it belongs rather than where the slice ended;
//! - a receiver whose own radio is not listening at that moment — because it is transmitting, or
//!   waiting for an acknowledgement of its own — refuses the frame, which is what makes two
//!   simultaneous transmissions collide instead of both being heard.
//!
//! A node may also start late (`start_ns`). That is not cosmetic: two identical images booted at
//! the same instant are deterministic to the cycle, so their application timers stay in lockstep
//! and every broadcast collides forever. Real motes are staggered by their power-on; here it has
//! to be said out loud.

use crate::soc::{machine, Machine};
use esp_soc::{RunUntil, SocBus};

/// 160 MHz: 25/4 ns per cycle, exactly (the same mapping `--cooja` uses).
const NS_NUM: u64 = 25;
const NS_DEN: u64 = 4;
pub fn ns_of_cycle(c: u64) -> u64 { c * NS_NUM / NS_DEN }
pub fn cycle_of_ns(ns: u64) -> u64 { (ns * NS_DEN).div_ceil(NS_NUM) }

/// How far a node runs before the medium looks again. Shorter is more exact and slower; a frame
/// is ~1 ms on the air, so 100 µs keeps a collision a collision.
pub const DEFAULT_SLICE_NS: u64 = 100_000;

/// What a receiver reports for a frame it took. Real path loss is not modelled: every node in
/// `range` hears every other at the same strength.
pub const RSSI_DBM: i8 = -65;
pub const LQI: u8 = 255;

pub struct Node {
    pub m: Machine,
    /// when this node leaves reset, in network time
    pub start_ns: u64,
    pub booted: bool,
    pub halted: bool,
    /// console bytes not yet collected by the front end
    pub console: Vec<u8>,
    pub x: f64,
    pub y: f64,
    pub tx: u64,
    pub rx: u64,
    pub rx_dropped: u64,
}

impl Node {
    /// This node's own position on the network clock.
    pub fn now_ns(&self) -> u64 { self.start_ns + ns_of_cycle(self.m.bus.cycles()) }
    fn running(&self, now_ns: u64) -> bool { self.booted && !self.halted && now_ns >= self.start_ns }
}

pub struct Network {
    pub nodes: Vec<Node>,
    pub now_ns: u64,
    pub slice_ns: u64,
    /// nodes closer than this hear each other; `f64::INFINITY` is one broadcast domain
    pub range: f64,
    pub frames: u64,
    pub collisions: u64,
}

impl Default for Network { fn default() -> Self { Self::new() } }

impl Network {
    pub fn new() -> Self {
        Network { nodes: Vec::new(), now_ns: 0, slice_ns: DEFAULT_SLICE_NS, range: f64::INFINITY, frames: 0, collisions: 0 }
    }

    /// Add a node. `mac` is what its efuses report, so two nodes must not share one: Contiki
    /// derives its link-layer address from it and drops a frame that appears to come from itself.
    /// Unknown board names return an error without adding a node.
    pub fn add(&mut self, mac: [u8; 6], flash_bytes: usize, start_ns: u64, x: f64, y: f64, board: &str) -> Result<usize, String> {
        let board = crate::board::make_board(board).ok_or_else(|| format!("unknown ESP32-C6 board: {board:?}"))?;
        let mut m = machine(mac, flash_bytes);
        m.bus.set_flash_size(flash_bytes);
        m.bus.board = board;
        m.bus.attach_board_devices();
        m.console.capture = true;
        m.console.mask = 2;                              // UART0, where the IDF console goes
        self.nodes.push(Node { m, start_ns, booted: false, halted: false, console: Vec::new(), x, y, tx: 0, rx: 0, rx_dropped: 0 });
        Ok(self.nodes.len() - 1)
    }

    pub fn boot(&mut self) {
        for n in &mut self.nodes { n.m.boot_rom(); n.booted = true; }
    }

    fn in_range(&self, a: usize, b: usize) -> bool {
        if !self.range.is_finite() { return true; }
        let (p, q) = (&self.nodes[a], &self.nodes[b]);
        ((p.x - q.x).powi(2) + (p.y - q.y).powi(2)).sqrt() <= self.range
    }

    /// Advance the whole network to `until_ns`.
    pub fn run_until(&mut self, until_ns: u64) {
        while self.now_ns < until_ns {
            let target = (self.now_ns + self.slice_ns).min(until_ns);
            // (source, network time of the TX_START write, channel, frame)
            let mut sent: Vec<(usize, u64, u8, Vec<u8>)> = Vec::new();
            for i in 0..self.nodes.len() {
                while self.nodes[i].running(target) && self.nodes[i].now_ns() < target {
                    match self.step(i, target) {
                        Some(tx) => sent.push((i, tx.0, tx.1, tx.2)),
                        None => break,
                    }
                }
            }
            for (src, t, _ch, frame) in sent {
                self.frames += 1;
                self.nodes[src].tx += 1;
                for j in 0..self.nodes.len() {
                    if j == src || !self.nodes[j].running(target) || !self.in_range(src, j) { continue; }
                    // the receiver has run to the slice end, so the frame started this long ago
                    let ago = cycle_of_ns(self.nodes[j].now_ns().saturating_sub(t));
                    let taken = self.nodes[j].m.bus.radio_receive(&frame, RSSI_DBM, LQI, Some(ago));
                    self.nodes[j].m.sync_irq();
                    if taken { self.nodes[j].rx += 1; } else { self.nodes[j].rx_dropped += 1; self.collisions += 1; }
                }
            }
            self.now_ns = target;
        }
    }

    /// Run node `i` to `target_ns`. `Some((t, channel, frame))` if it started a transmission on
    /// the way, with time standing at the `TX_START` write.
    fn step(&mut self, i: usize, target_ns: u64) -> Option<(u64, u8, Vec<u8>)> {
        let start_ns = self.nodes[i].start_ns;
        let n = &mut self.nodes[i];
        let target = cycle_of_ns(target_ns.saturating_sub(start_ns));
        let r = n.m.run_until_cycle(target);
        let streams = n.m.bus.console_take();
        if let Some(uart0) = streams.get(1) { n.console.extend_from_slice(uart0); }
        match r {
            RunUntil::Reached => None,
            RunUntil::Yield => {
                let radio = &n.m.bus.periph.radio;
                let (ch, frame) = (radio.channel(), radio.tx_frame.clone());
                Some((start_ns + ns_of_cycle(n.m.bus.cycles()), ch, frame))
            }
            RunUntil::Stop(stop) => {
                if let esp_soc::Stop::SwReset = stop { n.m.reboot(); } else { n.halted = true; }
                None
            }
        }
    }

    /// The node's single WS2812, packed 0xRRGGBB, and how many times it has changed.
    pub fn led(&self, i: usize) -> (u32, u64) {
        self.nodes.get(i).and_then(|n| esp_soc::SocBus::board_ref(&n.m.bus).leds().map(|(l, v)| {
            let c = l.first().copied().unwrap_or([0, 0, 0]);
            ((c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32, v)
        })).unwrap_or((0, 0))
    }

    /// Console bytes node `i` has produced since the last call.
    pub fn take_console(&mut self, i: usize) -> Vec<u8> {
        self.nodes.get_mut(i).map(|n| std::mem::take(&mut n.console)).unwrap_or_default()
    }
}
