//! The C6's WiFi side, as far as an unmodified ESP-IDF station has asked for it so far.
//!
//! Nothing here is copied from the S3: the C6 has its own modem subsystem (baseband at
//! `0x600A0000`, the 802.15.4 MAC at `0x600A3000` in `radio.rs`, the WiFi MAC further up), and
//! every register below is here because the closed PHY/WiFi library was seen waiting on it, with
//! the waiting code named. The specimen is `examples/c6-wifi-station`; the plan and the order of
//! work are in `docs/wifi-c6-plan.md`.
use esp_periph::{Device, RegRam, WriteEffect};

/// The modem baseband block (`0x600A0000`). Register RAM, plus the handshakes the PHY library
/// polls:
///
/// - **Channel switch.** The ROM's `freq_chan_en_sw` puts the channel index into `+0xC0` bits
///   13:7 and pulses bit 14 (start); the library's `ram_set_chan_freq_sw_start` then spins on
///   `+0xCC` bit 8 (done). The synthesiser settling is not modelled: the switch is done at the
///   start pulse, and stays done until the next one.
/// - **IQ estimate.** `ram_iq_est_enable` (from `dc_iq_est_new`, which the scan runs on every
///   channel) sets `+0x474` bit 0 (enable) then bit 1 (start) and spins on `+0x4A0` bit 16
///   (done). There is no signal to estimate: done follows the start bit, and the result
///   registers read as written, zero.
pub struct ModemBb {
    ram: RegRam,
    chan_done: bool,
    /// the channel index of the last switch, and how many there were (for `--debug` and tests)
    pub chan_index: u32,
    pub chan_switches: u32,
}
impl Default for ModemBb { fn default() -> Self { Self::new() } }
impl ModemBb {
    pub fn new() -> Self { ModemBb { ram: RegRam::new(), chan_done: false, chan_index: 0, chan_switches: 0 } }
}

const FREQ_CHAN: u32 = 0xc0;
const FREQ_CHAN_START: u32 = 1 << 14;
const FREQ_STATUS: u32 = 0xcc;
const FREQ_STATUS_DONE: u32 = 1 << 8;
const IQ_EST: u32 = 0x474;
const IQ_EST_START: u32 = 1 << 1;
const IQ_EST_STATUS: u32 = 0x4a0;
const IQ_EST_DONE: u32 = 1 << 16;

impl Device for ModemBb {
    fn read(&mut self, off: u32) -> u32 {
        match off {
            // Inferred from libphy txdc_cal_new: ideal zero DC imbalance, synchronous completion.
            0x418 => self.ram.read(off) & 0x003f_ffff | (u32::from(self.ram.read(off) & 1 != 0) << 22),
            0x814 => if self.ram.read(0x810) & 1 != 0 { 7 << 14 } else { 0 },
            FREQ_STATUS => self.ram.read(off) & !FREQ_STATUS_DONE | if self.chan_done { FREQ_STATUS_DONE } else { 0 },
            IQ_EST_STATUS => self.ram.read(off) & !IQ_EST_DONE | if self.ram.read(IQ_EST) & IQ_EST_START != 0 { IQ_EST_DONE } else { 0 },
            _ => self.ram.read(off),
        }
    }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        if off == FREQ_CHAN && v & FREQ_CHAN_START != 0 && self.ram.read(off) & FREQ_CHAN_START == 0 {
            self.chan_index = (v >> 7) & 0x7f;
            self.chan_switches += 1;
            self.chan_done = true;
        }
        self.ram.write(off, v);
        WriteEffect::NONE
    }
}

/// The 802.11 MAC (`0x600A4000`) that the closed `libpp`/`libnet80211` drive
/// (`mac_version:HAL_MAC_ESP32AX_761`). Undocumented. It is a relative of the S3's MAC
/// (`esp32s3::periph::WifiMac`): the same mechanisms at shifted offsets, so that model is the map
/// of what to look for, never a source of addresses. Each register below is named by the
/// library function that uses it (`docs/wifi-c6-plan.md` has the table). Register RAM, plus:
///
/// - **`hal_init`** sets `+0xDDC` bit 1 and spins until bit 0 reads set: the core's ready flag.
/// - **Events** at `+0xC48`, cleared by writing `+0xC4C`: bit 7 a transmission completed
///   (`lmacPostTxComplete`), bit 14 a frame was received (`lmacProcessRxSucData`). Any event
///   asserts the MAC's interrupt source.
/// - **RX ring**: base `+0x084`, the descriptor the hardware fills next `+0x088`, the last one
///   it filled `+0x08C`, reload request `+0x080` bit 0. The registers hold the low 20 bits of a
///   descriptor's address. `+0xC70` is the hardware's own full pointer: the library never writes
///   it and takes the high 12 bits of the last descriptor from it (`hal_mac_rx_get_last_dscr`),
///   then walks its ring up to that address. A descriptor is three words:
///   `size:14 length:14 _:2 has_data:1 owner:1`, the buffer, the next descriptor. The fields are
///   two bits wider than the S3's: the library arms a 1700-byte buffer as `0x81A906A4`, which is
///   1700 in both, and re-arms with the length set back to the size.
/// - **TX queues**: queue n's control word is at `+0xD6C - 16n`, the descriptor's low 20 bits in
///   it, bits 31:30 start the transmission (`hal_mac_txq_enable`). Completion sets bit n of
///   `+0xCB8`, cleared by writing the bit to `+0xCB4`. The result word (`hal_mac_get_txq_pmd`,
///   in the next block) stays zero: success.
///
/// The access point and the network behind it are the chip-independent ones from `esp-soc`; the
/// bus moves frames between them and the guest's descriptors (`bus.rs`).
pub struct WifiMac {
    ram: RegRam,
    pub log: bool,
    pub events: u32,
    pub rx_base: u32, pub rx_next: u32, pub rx_last: u32,
    pub txq_complete: u32, pub tx_pending: Vec<(u8, u32)>,
    /// the access point, the network and the counts, shared with the other chips
    pub link: esp_soc::wifi::StationLink,
}
impl Default for WifiMac { fn default() -> Self { Self::new() } }

const MAC_INIT: u32 = 0xddc;
const MAC_INIT_READY: u32 = 1;
const MAC_EVENTS: u32 = 0xc48;
const MAC_EVENTS_CLR: u32 = 0xc4c;
pub const EVENT_TX_DONE: u32 = 1 << 7;
pub const EVENT_RX: u32 = 1 << 14;
const RX_RELOAD: u32 = 0x080;
const RX_BASE: u32 = 0x084;
const RX_NEXT: u32 = 0x088;
const RX_LAST: u32 = 0x08c;
const ADDR_HIGH: u32 = 0xc70;
const TXQ0: u32 = 0xd6c;
const TXQ_STRIDE: u32 = 16;
/// the queue state registers have bits 10:0; past queue 10 the offsets belong to other registers
const TXQ_COUNT: u32 = 11;
const TXQ_COMPLETE_CLR: u32 = 0xcb4;
const TXQ_COMPLETE: u32 = 0xcb8;
/// descriptors and buffers are in HP SRAM: the high 12 bits of every DMA address
const SRAM_HIGH: u32 = 0x4080_0000;

impl WifiMac {
    pub fn new() -> Self {
        WifiMac { ram: RegRam::new(), log: false, events: 0, rx_base: 0, rx_next: 0, rx_last: 0, txq_complete: 0, tx_pending: Vec::new(), link: Default::default() }
    }
    /// A descriptor address from the 20 bits a register holds.
    pub fn addr(&self, low: u32) -> u32 { SRAM_HIGH | (low & 0xf_ffff) }
    fn txq_of(off: u32) -> Option<u8> {
        (off <= TXQ0 && (TXQ0 - off).is_multiple_of(TXQ_STRIDE) && (TXQ0 - off) / TXQ_STRIDE < TXQ_COUNT).then(|| ((TXQ0 - off) / TXQ_STRIDE) as u8)
    }
    /// The hardware sent the frame in `queue`.
    pub fn tx_done(&mut self, queue: u8) {
        self.txq_complete |= 1 << queue; self.events |= EVENT_TX_DONE;
        let off = TXQ0 - TXQ_STRIDE * queue as u32;
        let v = self.ram.read(off); self.ram.write(off, v & !(3 << 30));
    }
    /// The hardware filled `desc` and moved on to `next`.
    pub fn rx_filled(&mut self, desc: u32, next: u32) {
        self.rx_last = desc & 0xf_ffff; self.rx_next = next & 0xf_ffff; self.link.rx_delivered(desc); self.events |= EVENT_RX;
    }
}

impl Device for WifiMac {
    fn read(&mut self, off: u32) -> u32 {
        let v = match off {
            MAC_INIT => self.ram.read(off) | MAC_INIT_READY,
            MAC_EVENTS => self.events,
            RX_BASE => self.rx_base & 0xf_ffff, RX_NEXT => self.rx_next & 0xf_ffff, RX_LAST => self.rx_last,
            TXQ_COMPLETE => self.txq_complete,
            ADDR_HIGH => if self.link.last_rx_desc() != 0 { self.link.last_rx_desc() } else { SRAM_HIGH },
            TXQ_COMPLETE_CLR => 0,
            _ => self.ram.read(off),
        };
        if self.log { eprintln!("[wifi] rd +{:#05x} -> {:#010x}", off, v); }
        v
    }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        if self.log { eprintln!("[wifi] wr +{:#05x} <- {:#010x}", off, v); }
        match off {
            MAC_EVENTS_CLR => self.events &= !v,
            RX_BASE => {
                // where the hardware restarts when the ring has run dry; rewriting it while the ring
                // is live must not rewind the hardware's own pointer (as on the S3)
                self.rx_base = v & 0xf_ffff;
                if self.rx_next == 0 { self.rx_next = self.rx_base; }
                self.ram.write(off, v);
            }
            RX_RELOAD => {
                if v & 1 != 0 && self.rx_next == 0 { self.rx_next = self.rx_base; }
                self.ram.write(off, v & !1);
            }
            TXQ_COMPLETE_CLR => self.txq_complete &= !v,
            o if Self::txq_of(o).is_some() => {
                self.ram.write(off, v);
                if v & (1 << 31) != 0 { self.tx_pending.push((Self::txq_of(o).unwrap(), v & 0xf_ffff)); }
            }
            _ => self.ram.write(off, v),
        }
        WriteEffect::NONE
    }
    fn irq_sources(&self) -> u64 { (self.events != 0) as u64 }
    fn debug(&mut self, on: bool) { self.log = on; }
}
