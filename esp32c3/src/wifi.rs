//! C3 MAC, analog register master and ideal-radio calibration handshakes.
use esp_periph::{Device, RegRam, WriteEffect};
/// The 802.11 MAC the closed `libpp`/`libnet80211` drive. Undocumented by Espressif; the
/// C3 register layout is modelled from the closed driver's accesses, using the S3 model as a starting
/// point. esp32-open-mac describes the classic ESP32, not this MAC. No C3 silicon comparison.
pub struct WifiMac {
    pub ram: RegRam,
    pub ram2: RegRam,
    pub log: bool,
    /// TSF: 1 MHz counter (offset applied to the CPU cycle clock), latched into WDEV 0x18/0x1c
    pub tsf_offset: i64,
    pub tsf_latched: u64,
    pub now_cycles: u64,
    /// interrupt events (0xc3c; cleared by writing 0xc40): bit 7 = TX complete, bits 14/24 = RX data (libpp wDev_ProcessFiq)
    pub events: u32,
    pub pwr_events: u32,
    /// per-queue completion bitmap (0xca8 bits 10:0, cleared via 0xca4)
    pub txq_complete: u32,
    pub txq_error: u32,
    pub tx_pending: Vec<(u8, u32)>,
    /// RX descriptor ring: base written by the driver (0x088), the descriptor the hardware fills next, the last one filled
    pub rx_base: u32,
    pub rx_next: u32,
    pub rx_last: u32,
    /// the access point, the network and the counts, shared with the other chips
    pub link: esp_soc::wifi::StationLink,
}

impl WifiMac {
    pub fn new() -> Self {
        WifiMac {
            ram: RegRam::new(),
            ram2: RegRam::new(),
            log: false,
            tsf_offset: 0,
            tsf_latched: 0,
            now_cycles: 0,
            rx_base: 0,
            rx_next: 0,
            rx_last: 0,
            events: 0,
            pwr_events: 0,
            txq_complete: 0,
            txq_error: 0,
            tx_pending: Vec::new(),
            link: Default::default(),
        }
    }
    pub fn irq(&self) -> bool {
        self.events != 0 || self.pwr_events != 0
    }
    /// TX queue n has its PLCP0 register at 0xd08 - 8n (hal_mac_txq_enable: (0x0c0067a1 - n) << 3).
    /// Eleven PLCP slots end at 0xcb8; extending to 16 would overlap state/clear registers
    /// at 0xcb0 and 0xca8. This is a driver-derived limit, not a silicon queue count.
    fn txq_of(off: u32) -> Option<u8> {
        if off <= 0xd08 && (0xd08 - off).is_multiple_of(8) && (0xd08 - off) / 8 < 11 {
            Some(((0xd08 - off) / 8) as u8)
        } else {
            None
        }
    }
    pub fn read(&mut self, block: u32, off: u32) -> u32 {
        let v = match (block, off) {
            (0x33, 0xd14) => self.ram.read(off) | 1, // hal_init: writes bit 1, waits for bit 0
            (0x33, 0xc3c) => self.events,
            (0x33, 0x088) => self.rx_base & 0xf_ffff,
            (0x33, 0x08c) => self.rx_next & 0xf_ffff,
            (0x33, 0x090) => self.rx_last,
            (0x33, 0xca8) => self.txq_error & 0x7ff, // txq state types 0/1 (errors/collisions)
            (0x33, 0xcb0) => self.txq_complete & 0xf, // txq state type 2: completed queues
            (0x35, 0x118) => self.pwr_events,
            (0x35, 0x18) => self.tsf_latched as u32,
            (0x35, 0x1c) => (self.tsf_latched >> 32) as u32,
            (0x35, 0x128) => self.ram2.read(off),
            (0x33, _) => self.ram.read(off),
            (_, _) => self.ram2.read(off),
        };
        if self.log {
            eprintln!("[wifi] rd {:#x}+{:#05x} -> {:#010x}", block, off, v);
        }
        v
    }
    pub fn write(&mut self, block: u32, off: u32, v: u32) {
        if self.log {
            eprintln!("[wifi] wr {:#x}+{:#05x} <- {:#010x}", block, off, v);
        }
        match (block, off) {
            (0x33, 0xc40) => {
                self.events &= !v;
            }
            (0x33, 0x088) => {
                // BASE_RX_DSCR: where the hardware restarts when the ring runs dry. Software rewrites it
                // every time it recycles descriptors, but that must NOT rewind the hardware's current
                // pointer — doing so re-delivers into descriptors the stack has already moved past.
                self.rx_base = esp_periph::DMA_ADDR_BASE | (v & 0xf_ffff);
                if self.rx_next == 0 {
                    self.rx_next = self.rx_base;
                }
                self.ram.write(off, v);
            }
            (0x33, 0x084) => {
                // DSCR_RELOAD: software has appended recycled descriptors and asks the hardware to
                // re-read the chain.
                // Measured against the blob: rewinding here makes every second frame land in a
                // descriptor the stack has moved past, and it is recycled instead of indicated. The
                // hardware keeps its own pointer; base only matters once the ring has run dry.
                if v & 1 != 0 && self.rx_next == 0 {
                    self.rx_next = self.rx_base;
                }
                self.ram.write(off, v & !1);
            }
            (0x35, 0x11c) => {
                self.pwr_events &= !v;
            }
            (0x35, 0x0c) => {
                let now = (self.now_cycles / (crate::periph::CPU_HZ / 1_000_000)) as i64;
                if v & 3 != 0 {
                    self.tsf_latched = (now + self.tsf_offset) as u64;
                } // latch
                if v & (1 << 4) != 0 {
                    let set = (self.ram2.read(0x10) as u64) | ((self.ram2.read(0x14) as u64) << 32);
                    self.tsf_offset = set as i64 - now;
                } // load
                self.ram2.write(off, v);
            }
            (0x33, 0xca4) => {
                self.txq_error &= !(v & 0x7ff);
            }
            (0x33, 0xcac) => {
                self.txq_complete &= !(v & 0xf);
            }
            (0x33, o) if Self::txq_of(o).is_some() => {
                // MAC_TX_PLCP0[queue]
                self.ram.write(off, v);
                if v & (1 << 31) != 0 {
                    let q = Self::txq_of(o).unwrap();
                    self.tx_pending
                        .push((q, esp_periph::DMA_ADDR_BASE | (v & 0xf_ffff)));
                }
            }
            (0x33, _) => self.ram.write(off, v),
            (_, _) => self.ram2.write(off, v),
        }
    }
    /// Hardware finished sending the frame in `queue`.
    pub fn tx_done(&mut self, queue: u8) {
        self.txq_complete |= 1 << queue;
        self.events |= 1 << 7;
        let o = 0xd08 - 8 * queue as u32;
        let v = self.ram.read(o);
        self.ram.write(o, v & !(3 << 30));
        // result word (hal_mac_get_txq_pmd): bits 15:12 = status code, 0 = success (3 would trap the blob)
        let r = 0x320 - 76 * queue as u32;
        let w = self.ram2.read(r);
        self.ram2.write(r, w & !(0xf << 12));
    }
}

/// The MAC spans blocks 0x33..0x35; the table mounts it three times with `delta` = block index << 12.
impl Device for WifiMac {
    fn read(&mut self, off: u32) -> u32 {
        WifiMac::read(self, 0x33 + (off >> 12), off & 0xfff)
    }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        WifiMac::write(self, 0x33 + (off >> 12), off & 0xfff, v);
        WriteEffect::NONE
    }
    fn irq_sources(&self) -> u64 {
        self.irq() as u64
    }
    fn debug(&mut self, on: bool) {
        self.log = on;
    }
}

impl Default for WifiMac {
    fn default() -> Self {
        Self::new()
    }
}

/// Ideal-radio IQ estimator: the closed driver starts a sample with CTRL bits0/1,
/// then polls DONE. Correlation outputs remain zero, matching the S3 radio model.
/// Completion latency is deterministic, not an analog timing model. The driver polls DONE;
/// evaluate its timestamp on access so this non-interrupting device needs no ticking or deadline.
#[derive(Default)]
pub struct FeIq {
    ram: RegRam,
    pub now_cycles: u64,
    done_at: Option<u64>,
}
impl Device for FeIq {
    fn read(&mut self, off: u32) -> u32 {
        if off == 0x174 {
            return self.ram.read(off) | (u32::from(self.done_at.is_some_and(|t| self.now_cycles >= t)) << 16);
        }
        self.ram.read(off)
    }
    fn write(&mut self, off: u32, value: u32) -> WriteEffect {
        if off == 0x144 && value & 3 == 3 && self.ram.read(off) & 3 != 3 {
            self.done_at = Some((self.now_cycles / 2 + 80) * 2);
        }
        if off != 0x174 { self.ram.write(off, value); }
        WriteEffect::NONE
    }
}

pub struct I2cMst {
    pub ram: RegRam,
    pub ana: std::collections::HashMap<u32, u8>,
}
impl Default for I2cMst {
    fn default() -> Self {
        Self::new()
    }
}

impl I2cMst {
    pub fn new() -> Self {
        I2cMst {
            ram: RegRam::new(),
            ana: Default::default(),
        }
    }
    pub fn read(&mut self, off: u32) -> u32 {
        match off {
            0x0 | 0x4 => {
                // I2C0_CTRL: [7:0] slave, [15:8] reg, [23:16] data, [24] write, [25] busy
                let c = self.ram.read(off);
                if c & (1 << 24) == 0 {
                    let key = c & 0xffff;
                    let d = *self.ana.get(&key).unwrap_or(&0) as u32;
                    (c & !(0xff << 16) & !(1 << 25)) | (d << 16)
                } else {
                    c & !(1 << 25)
                }
            }
            // analog-block handshakes (BBPLL cal, pkdet, txdc/rxdc comparators...): the blob writes a start bit and
            // polls a done bit in 26:24; comparator sign bits 31:30 read as 0 — enough for its search loops to run
            0x40..=0x5c => (self.ram.read(off) & 0x3fff_ffff) | (7 << 24),
            _ => self.ram.read(off),
        }
    }
    pub fn write(&mut self, off: u32, v: u32) {
        if (off == 0 || off == 4) && v & (1 << 24) != 0 {
            self.ana.insert(v & 0xffff, (v >> 16) as u8);
        }
        self.ram.write(off, v);
    }
}

impl Device for I2cMst {
    fn read(&mut self, off: u32) -> u32 {
        I2cMst::read(self, off)
    }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        I2cMst::write(self, off, v);
        WriteEffect::NONE
    }
}
