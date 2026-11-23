//! The USB 2.0 On-The-Go controller: Synopsys' DesignWare Hi-Speed USB 2.0 OTG core (DWC2) as a
//! device with buffer DMA, as the ESP32-P4's high-speed port (0x50000000, UTMI+ PHY) has it and
//! the ESP32-S3's full-speed port (0x60080000) has a smaller copy of it, and the cable to a host
//! (`link`).
//!
//! Registers: IDF v5.5.3 components/soc/esp32p4/include/soc/usb_dwc_struct.h (the v4.30a HS
//! instance) and hal/esp32p4/include/hal/usb_dwc_ll.h; bit names and what the driver expects of
//! them: TinyUSB 0.21.0~2 (the `espressif__tinyusb` component) src/portable/synopsys/dwc2/
//! dwc2_type.h, dwc2_common.c and dcd_dwc2.c. What differs between instances is `Dwc2Config`.
//!
//! The core answers transactions the link asks of it (a SETUP, an IN token, an OUT data packet)
//! at once from its endpoint registers, moving the payload through DMA; the link decides when
//! they happen. Slave mode (the data FIFOs from 0x1000) and host mode are not modelled.
pub mod link;
pub use link::{BusEvent, Completion, Handshake, InReply, LinkFault, LinkStats, Outcome, PortStatus, Request, Speed, UsbFunction, UsbLink, UsbTiming};
use crate::device::{Device, WriteEffect};
use crate::regram::RegRam;
use emu_core::ClockDomain;

/// The memory the core's DMA reaches, as the SDMMC host's IDMAC sees it.
pub use crate::sdmmc::SdmmcMemory as DmaMemory;

/// What differs between the instances of the core.
#[derive(Clone, Copy, Debug)]
pub struct Dwc2Config {
    /// GSNPSID, GHWCFG1-4
    pub gsnpsid: u32,
    pub ghwcfg: [u32; 4],
    /// device OUT endpoints, endpoint 0 included (OTG_NUM_EPS + 1)
    pub eps: usize,
    /// device IN endpoints, endpoint 0 included (OTG_NUM_IN_EPS); IN endpoints past them, like
    /// endpoints past `eps`, read 0 and drop writes, and their TX FIFO registers too
    pub in_eps: usize,
    /// width of DxEPTSIZn.XferSize and PktCnt (OTG_TRANS_COUNT_WIDTH, OTG_PACKET_COUNT_WIDTH)
    pub xfer_bits: u32,
    pub pkt_bits: u32,
    /// the FIFO RAM in 32-bit words (OTG_DFIFO_DEPTH)
    pub fifo_words: u32,
    /// each TX FIFO's reset depth in words (OTG_TX_DINEP_DFIFO_DEPTH_n)
    pub tx_fifo_words: u32,
    /// UTMI+ high-speed PHY
    pub high_speed: bool,
    /// the clock `tick` counts
    pub hz: u64,
}

/// v4.30a, the version `usb_dwc_ll_grstctl_core_soft_reset` changes its reset sequence from
/// (IDF v5.5.3 hal/esp32p4/include/hal/usb_dwc_ll.h, USB_DWC_CORE_REG_GSNPSID_4_20a).
pub const GSNPSID_4_20A: u32 = 0x4F54_420A;

impl Dwc2Config {
    /// The ESP32-P4's HS instance: GSNPSID 4.30a (usb_dwc_struct.h, hal/usb_dwc_hal.c), GHWCFG1-4
    /// as TinyUSB 0.21.0~2 recorded them from silicon (dwc2_info.py, `ESP32-P4`), the counter
    /// widths, endpoints and FIFO RAM of soc/esp32p4/register/hw_ver1/soc/usb_dwc_cfg.h
    /// (OTG20_TRANS_COUNT_WIDTH 17, OTG20_PACKET_COUNT_WIDTH 8, OTG20_NUM_EPS 15, OTG20_NUM_IN_EPS 8,
    /// OTG20_DFIFO_DEPTH 1024; GHWCFG4.INEps 7 agrees). See docs/esp32p4.md, "USB".
    pub const fn esp32p4_hs(hz: u64) -> Self {
        Dwc2Config { gsnpsid: 0x4F54_430A, ghwcfg: [0, 0x215F_FFD0, 0x0380_5EB5, 0xDFF1_A030], eps: 16, in_eps: 8, xfer_bits: 17, pkt_bits: 8,
                     fifo_words: 1024, tx_fifo_words: 512, high_speed: true, hz }
    }

    /// The ESP32-S3's FS instance: GSNPSID 4.00a (IDF v5.5.4 hal/usb_dwc_hal.c accepts it as
    /// CORE_REG_GSNPSID_4_00a), GHWCFG1-4 as TinyUSB 0.21.0~6 recorded them from silicon
    /// (dwc2_info.py, `ESP32-S2/S3`), the counter widths, endpoints and FIFO RAM of IDF v5.5.4
    /// components/soc/esp32s3/include/soc/usb_dwc_cfg.h (OTG_TRANS_COUNT_WIDTH 16,
    /// OTG_PACKET_COUNT_WIDTH 7, OTG_NUM_EPS 6, OTG_NUM_IN_EPS 5, OTG_DFIFO_DEPTH 256,
    /// OTG_TX_DINEP_DFIFO_DEPTH_n 256; GHWCFG2.NumDevEps 6 and GHWCFG4.INEps 4 agree), the
    /// full-speed PHY (OTG_HSPHY_INTERFACE 0, OTG_FSPHY_INTERFACE 1).
    pub const fn esp32s3_fs(hz: u64) -> Self {
        Dwc2Config { gsnpsid: 0x4F54_400A, ghwcfg: [0, 0x224D_D930, 0x00C8_04B5, 0xD3F0_A030], eps: 7, in_eps: 5, xfer_bits: 16, pkt_bits: 7,
                     fifo_words: 256, tx_fifo_words: 256, high_speed: false, hz }
    }
}

/// Register offsets (usb_dwc_struct.h).
pub mod reg {
    pub const GOTGCTL: u32 = 0x000; pub const GOTGINT: u32 = 0x004; pub const GAHBCFG: u32 = 0x008; pub const GUSBCFG: u32 = 0x00c;
    pub const GRSTCTL: u32 = 0x010; pub const GINTSTS: u32 = 0x014; pub const GINTMSK: u32 = 0x018; pub const GRXSTSR: u32 = 0x01c;
    pub const GRXSTSP: u32 = 0x020; pub const GRXFSIZ: u32 = 0x024; pub const GNPTXFSIZ: u32 = 0x028; pub const GNPTXSTS: u32 = 0x02c;
    pub const GSNPSID: u32 = 0x040; pub const GHWCFG1: u32 = 0x044; pub const GHWCFG4: u32 = 0x050; pub const GDFIFOCFG: u32 = 0x05c;
    pub const DIEPTXF1: u32 = 0x104;
    pub const DCFG: u32 = 0x800; pub const DCTL: u32 = 0x804; pub const DSTS: u32 = 0x808; pub const DIEPMSK: u32 = 0x810;
    pub const DOEPMSK: u32 = 0x814; pub const DAINT: u32 = 0x818; pub const DAINTMSK: u32 = 0x81c; pub const DIEPEMPMSK: u32 = 0x834;
    pub const DEACHINT: u32 = 0x838;
    /// IN endpoint n at 0x900 + 0x20 n, OUT endpoint n at 0xB00 + 0x20 n: CTL, INT, TSIZ, DMA, (IN) DTXFSTS, DMAB
    pub const DIEP0: u32 = 0x900; pub const DOEP0: u32 = 0xb00; pub const EP_STRIDE: u32 = 0x20;
    pub const EPCTL: u32 = 0x00; pub const EPINT: u32 = 0x08; pub const EPTSIZ: u32 = 0x10; pub const EPDMA: u32 = 0x14; pub const DTXFSTS: u32 = 0x18; pub const EPDMAB: u32 = 0x1c;
    pub const PCGCCTL: u32 = 0xe00;
}

/// GOTGCTL: the overrides of the session signals, the connector ID and the session-valid inputs.
pub mod gotgctl {
    pub const VBVALOEN: u32 = 1 << 2; pub const VBVALOVAL: u32 = 1 << 3; pub const AVALOEN: u32 = 1 << 4; pub const AVALOVAL: u32 = 1 << 5;
    pub const BVALOEN: u32 = 1 << 6; pub const BVALOVAL: u32 = 1 << 7; pub const CIDSTS: u32 = 1 << 16; pub const ASVLD: u32 = 1 << 18;
    pub const BSESVLD: u32 = 1 << 19; pub const OTGVER: u32 = 1 << 20;
    /// read/write: SRQ (1) up to EHEN (12), DBNCEFLTRBYPASS (15), OTGVER (20)
    pub const RW: u32 = 0x1ffe | 1 << 15 | OTGVER;
}
/// GOTGINT.SEDET: session end detected (B-session valid fell).
pub const GOTGINT_SEDET: u32 = 1 << 2;
pub const GAHBCFG_GINT: u32 = 1 << 0;
pub const GAHBCFG_DMAEN: u32 = 1 << 5;
/// GUSBCFG: the PHY selection survives a core soft reset; FHMOD forces host mode, FDMOD device mode.
pub mod gusbcfg {
    pub const PHYIF16: u32 = 1 << 3; pub const ULPI_UTMI_SEL: u32 = 1 << 4; pub const FSINTF: u32 = 1 << 5; pub const PHYSEL: u32 = 1 << 6;
    pub const DDRSEL: u32 = 1 << 7; pub const PHYLPCS: u32 = 1 << 15; pub const FHMOD: u32 = 1 << 29; pub const FDMOD: u32 = 1 << 30;
    pub const KEPT: u32 = PHYIF16 | ULPI_UTMI_SEL | FSINTF | PHYSEL | DDRSEL | PHYLPCS;
}
/// GRSTCTL: core soft reset, FIFO flushes (self-clearing), CSRSTDONE (write 1 to clear, from 4.20a), AHB idle.
pub mod grstctl {
    pub const CSRST: u32 = 1 << 0; pub const HSRST: u32 = 1 << 1; pub const FCRST: u32 = 1 << 2; pub const RXFFLSH: u32 = 1 << 4;
    pub const TXFFLSH: u32 = 1 << 5; pub const TXFNUM: u32 = 0x1f << 6; pub const CSRSTDONE: u32 = 1 << 29; pub const AHBIDL: u32 = 1 << 31;
}
/// GINTSTS / GINTMSK bits.
pub mod gint {
    pub const OTGINT: u32 = 1 << 2; pub const SOF: u32 = 1 << 3; pub const GINAKEFF: u32 = 1 << 6; pub const BOUTNAKEFF: u32 = 1 << 7;
    pub const ESUSP: u32 = 1 << 10; pub const USBSUSP: u32 = 1 << 11; pub const USBRST: u32 = 1 << 12; pub const ENUMDNE: u32 = 1 << 13;
    pub const IEPINT: u32 = 1 << 18; pub const OEPINT: u32 = 1 << 19; pub const RSTDET: u32 = 1 << 23; pub const SRQINT: u32 = 1 << 30;
    pub const WKUINT: u32 = 1 << 31;
    /// the latched bits software clears by writing 1: MMIS, SOF, ESUSP to EOPF, IISOIXFR to RSTDET, CIDSCHG to WKUINT
    pub const W1C: u32 = 1 << 1 | SOF | 0x3f << 10 | 0xf << 20 | 0xf << 28;
}
/// DCFG: device speed (0 high, 1 full on a HS PHY, 3 full at 48 MHz), address.
pub const DCFG_DSPD: u32 = 3;
pub const DCFG_DAD_SHIFT: u32 = 4;
/// DCTL: remote wakeup signalling, soft disconnect, the global NAK status and strobes.
pub mod dctl {
    pub const RWUSIG: u32 = 1 << 0; pub const SDIS: u32 = 1 << 1; pub const GINSTS: u32 = 1 << 2; pub const GONSTS: u32 = 1 << 3;
    pub const SGINAK: u32 = 1 << 7; pub const CGINAK: u32 = 1 << 8; pub const SGONAK: u32 = 1 << 9; pub const CGONAK: u32 = 1 << 10;
    pub const POPRGDNE: u32 = 1 << 11;
    /// read/write: RWUSIG, SDIS, TCTL (6:4), POPRGDNE, GMC (14:13), IGNRFRMNUM (15), NAKONBBLE, ENCONTONBNA
    pub const RW: u32 = RWUSIG | SDIS | 7 << 4 | POPRGDNE | 3 << 13 | 7 << 15;
}
/// DxEPCTLn bits.
pub mod epctl {
    pub const MPSIZ: u32 = 0x7ff; pub const USBAEP: u32 = 1 << 15; pub const DPID: u32 = 1 << 16; pub const NAKSTS: u32 = 1 << 17;
    pub const EPTYP: u32 = 3 << 18; pub const SNPM: u32 = 1 << 20; pub const STALL: u32 = 1 << 21; pub const TXFNUM: u32 = 0xf << 22;
    pub const CNAK: u32 = 1 << 26; pub const SNAK: u32 = 1 << 27; pub const SD0PID: u32 = 1 << 28; pub const SD1PID: u32 = 1 << 29;
    pub const EPDIS: u32 = 1 << 30; pub const EPENA: u32 = 1 << 31;
    pub const EPTYP_ISO: u32 = 1 << 18;
}
/// DIEPINTn / DOEPINTn bits.
pub mod epint {
    pub const XFRC: u32 = 1 << 0; pub const EPDISD: u32 = 1 << 1; pub const AHBERR: u32 = 1 << 2; pub const SETUP: u32 = 1 << 3;
    pub const OTEPDIS: u32 = 1 << 4; pub const INEPNE: u32 = 1 << 6; pub const B2BSTUP: u32 = 1 << 6; pub const TXFE: u32 = 1 << 7;
    pub const BBLEERR: u32 = 1 << 12; pub const NAK: u32 = 1 << 13; pub const STPKTRX: u32 = 1 << 15;
}
/// DOEPTSIZ0.STUPCNT (30:29): back-to-back SETUP packets endpoint 0 may still take.
pub const STUPCNT_SHIFT: u32 = 29;
pub const PKTCNT_SHIFT: u32 = 19;

#[derive(Clone, Copy, Default, Debug)]
struct Ep { ctl: u32, int: u32, tsiz: u32, dma: u32, pid: bool }

#[derive(Clone, Copy, Default, Debug)]
pub struct Dwc2Stats {
    pub setups: u64, pub packets_in: u64, pub packets_out: u64, pub bytes_in: u64, pub bytes_out: u64, pub naks: u64, pub stalls: u64,
    pub resets: u64, pub dma_faults: u64, pub fifo_errors: u64, pub toggle_discards: u64,
}

pub struct Dwc2 {
    pub cfg: Dwc2Config,
    gotgctl: u32, gotgint: u32, gahbcfg: u32, gusbcfg: u32, grstctl: u32, gintsts: u32, gintmsk: u32,
    grxfsiz: u32, gnptxfsiz: u32, gdfifocfg: u32, dieptxf: [u32; 15], pcgcctl: u32,
    dcfg: u32, dctl: u32, diepmsk: u32, doepmsk: u32, daintmsk: u32, diepempmsk: u32,
    ep_in: [Ep; 16], ep_out: [Ep; 16],
    ram: RegRam,
    /// what the cable presents: B-session valid, the speed the last bus reset ended in, SOFs since
    vbus: bool,
    speed: Option<Speed>,
    sof_since: Option<u64>,
    sof_ack: u64,
    suspended: bool,
    /// a SETUP was taken and the token after it has not come yet: SETUP phase done then
    setup_phase: bool,
    /// a SETUP that arrived while control OUT 0 was not enabled, waiting in the RxFIFO
    setup_stash: Option<[u8; 8]>,
    pullup_seen: bool,
    /// time in `cfg.hz` ticks, kept while a cable is attached
    pub now: u64,
    pub link: Option<Box<UsbLink>>,
    pending: bool,
    /// the block's reset line is asserted: registers at reset values, writes ignored
    held: bool,
    /// the PHY carries the core's D+/D- to the cable (the S3 shares its one internal PHY with
    /// USB-Serial-JTAG; `set_phy`)
    phy: bool,
    /// a (micro)frame in ticks at the enumerated speed
    frame: u64,
    pub stats: Dwc2Stats,
    log: bool,
}

impl Dwc2 {
    pub fn new(cfg: Dwc2Config) -> Self {
        let mut d = Dwc2 { cfg, gotgctl: 0, gotgint: 0, gahbcfg: 0, gusbcfg: 0, grstctl: 0, gintsts: 0, gintmsk: 0, grxfsiz: 0, gnptxfsiz: 0, gdfifocfg: 0,
            dieptxf: [0; 15], pcgcctl: 0, dcfg: 0, dctl: 0, diepmsk: 0, doepmsk: 0, daintmsk: 0, diepempmsk: 0,
            ep_in: [Ep::default(); 16], ep_out: [Ep::default(); 16], ram: RegRam::new(), vbus: false, speed: None, sof_since: None,
            sof_ack: 0, suspended: false, setup_phase: false, setup_stash: None, pullup_seen: false, now: 0, link: None, pending: false,
            held: false, phy: true, frame: 0, stats: Dwc2Stats::default(), log: false };
        d.set_speed(None);
        d.reset_registers();
        d
    }

    /// The block's reset (LP_CLKRST RST_EN_USB_OTG20 on the P4): every register to its reset
    /// value; the cable stays. GRXFSIZ, the TX FIFO sizes and GDFIFOCFG reset to the
    /// configuration's FIFO depths (usb_dwc_cfg.h OTG20_RX_DFIFO_DEPTH, OTG20_TX_DINEP_DFIFO_DEPTH_n,
    /// OTG20_DFIFO_DEPTH on the P4, the OTG_ ones on the S3); GUSBCFG to USBTrdTim 5. DCTL.SDIS
    /// resets to 1, so nothing pulls D+ up before the firmware connects.
    pub fn reset_registers(&mut self) {
        let (words, tx) = (self.cfg.fifo_words, self.cfg.tx_fifo_words);
        self.gusbcfg = 5 << 10;
        self.dcfg = 0;
        self.dctl = dctl::SDIS;
        self.pcgcctl = 0;
        self.soft_reset();
        self.grstctl = grstctl::AHBIDL;
        self.grxfsiz = words;
        self.gnptxfsiz = tx << 16 | words;
        for (n, f) in self.dieptxf.iter_mut().enumerate() { *f = tx << 16 | (words + tx * (n as u32 + 1)); }
        self.gdfifocfg = words << 16 | words;
        self.ram = RegRam::new();
        self.notify_pullup();
    }

    /// Core soft reset: the interrupts, the endpoints and every CSR except the PHY selection in
    /// GUSBCFG, DCFG.DevSpd, DCTL.SftDiscon and PCGCCTL; the FIFOs flush.
    fn soft_reset(&mut self) {
        self.gotgctl = 0; self.gotgint = 0; self.gahbcfg = 0; self.gusbcfg &= gusbcfg::KEPT | 0xf << 10; self.gintsts = 0; self.gintmsk = 0;
        self.dcfg &= DCFG_DSPD; self.dctl &= dctl::SDIS; self.diepmsk = 0; self.doepmsk = 0; self.daintmsk = 0; self.diepempmsk = 0;
        self.ep_in = [Ep::default(); 16]; self.ep_out = [Ep::default(); 16];
        self.ep_in[0].ctl = epctl::USBAEP;
        self.ep_out[0].ctl = epctl::USBAEP;
        self.setup_phase = false; self.setup_stash = None;
        self.stats.resets += 1;
    }

    /// The block's reset line (LP_CLKRST RST_EN_USB_OTG20 on the P4): asserting it resets the
    /// registers, which ignore writes until it is released.
    pub fn set_reset(&mut self, asserted: bool) {
        if asserted && !self.held { self.reset_registers(); }
        self.held = asserted;
    }

    fn set_speed(&mut self, speed: Option<Speed>) {
        self.speed = speed;
        let ns = if speed == Some(Speed::High) { 125_000 } else { 1_000_000 };
        self.frame = ((ns as u128 * self.cfg.hz as u128 / 1_000_000_000) as u64).max(1);
    }

    /// Attach a cable to a host: it sees whatever the core presents from now on.
    pub fn attach(&mut self, now: u64, link: UsbLink) {
        self.now = now;
        self.link = Some(Box::new(link));
        self.pullup_seen = false;
        self.notify_pullup();
    }

    /// After a chip reset: the same cable, at the time it had, on a core back at its reset values.
    pub fn reattach(&mut self, now: u64, link: Box<UsbLink>) {
        self.vbus = link.port().power;
        self.now = now;
        self.link = Some(link);
        self.pullup_seen = true;
        self.notify_pullup();
    }

    /// The cable's work is due: `run` it with the memory the DMA reaches.
    #[inline]
    pub fn pending(&self) -> bool { self.pending }

    /// Let the link carry out what is due at the current time.
    pub fn run(&mut self, mem: &mut dyn DmaMemory) {
        self.pending = false;
        let Some(mut link) = self.link.take() else { return };
        link.service(self.now, self, mem);
        self.link = Some(link);
        self.notify_pullup();
    }

    /// `--debug usb_otg` is on.
    pub fn logging(&self) -> bool { self.log }

    fn frame_ticks(&self) -> u64 { self.frame }

    /// GINTSTS.SOF: a (micro)frame has started since software last cleared it.
    fn sof_due(&self) -> bool {
        let Some(since) = self.sof_since else { return false };
        let f = self.frame_ticks().max(1);
        let last = self.now / f * f;
        last >= since && last > self.sof_ack
    }

    /// B-session valid, or its override (GOTGCTL.BVALOEN/BVALOVAL).
    fn bvalid(&self) -> bool { if self.gotgctl & gotgctl::BVALOEN != 0 { self.gotgctl & gotgctl::BVALOVAL != 0 } else { self.vbus } }

    /// D+ pulled up where the host sees it: a session, device mode, no soft disconnect, and a
    /// PHY connecting the core to the cable.
    pub fn pullup(&self) -> bool { self.phy && self.bvalid() && self.gusbcfg & gusbcfg::FHMOD == 0 && self.dctl & dctl::SDIS == 0 }

    /// Connect the core to the cable's PHY or take it away. Without one the host sees no pull-up,
    /// so it never resets or enumerates the device.
    pub fn set_phy(&mut self, on: bool) {
        if on == self.phy { return; }
        self.phy = on;
        self.notify_pullup();
    }

    fn notify_pullup(&mut self) {
        let p = self.pullup();
        if p == self.pullup_seen { return; }
        let now = self.now;
        if let Some(l) = &mut self.link { self.pullup_seen = p; l.device_pullup(now, p); }
    }

    /// Max packet size; both control endpoints take DIEPCTL0's (64, 32, 16 or 8 bytes).
    fn mps(&self, ep: usize, out: bool) -> usize {
        let ctl = if out { self.ep_out[ep].ctl } else { self.ep_in[ep].ctl };
        if ep == 0 { [64, 32, 16, 8][(self.ep_in[0].ctl & 3) as usize] } else { (ctl & epctl::MPSIZ) as usize }
    }

    fn xfer_mask(&self, ep: usize) -> u32 { if ep == 0 { 0x7f } else { (1 << self.cfg.xfer_bits) - 1 } }
    fn pkt_mask(&self, ep: usize, out: bool) -> u32 { if ep == 0 { if out { 1 } else { 3 } } else { (1 << self.cfg.pkt_bits) - 1 } }

    fn tsiz(&self, ep: usize, out: bool) -> (u32, u32) {
        let t = if out { self.ep_out[ep].tsiz } else { self.ep_in[ep].tsiz };
        (t & self.xfer_mask(ep), t >> PKTCNT_SHIFT & self.pkt_mask(ep, out))
    }
    fn set_tsiz(&mut self, ep: usize, out: bool, xfer: u32, pkt: u32) {
        let (xm, pm) = (self.xfer_mask(ep), self.pkt_mask(ep, out));
        let e = if out { &mut self.ep_out[ep] } else { &mut self.ep_in[ep] };
        e.tsiz = (e.tsiz & !(xm | pm << PKTCNT_SHIFT)) | (xfer & xm) | (pkt & pm) << PKTCNT_SHIFT;
    }

    /// The bytes of IN endpoint `ep`'s TX FIFO (DIEPCTLn.TxFNum selects DIEPTXFn; endpoint 0 has GNPTXFSIZ).
    fn tx_fifo_bytes(&self, ep: usize) -> usize {
        let n = if ep == 0 { 0 } else { (self.ep_in[ep].ctl & epctl::TXFNUM) >> 22 } as usize;
        let f = if n == 0 { self.gnptxfsiz } else { self.dieptxf[n - 1] };
        (f >> 16) as usize * 4
    }

    /// DAINT: an endpoint's bit is set while its interrupt register has a bit DIEPMSK/DOEPMSK lets
    /// through (TXFE goes through DIEPEMPMSK); DAINTMSK does not gate it.
    fn daint(&self) -> u32 {
        let mut v = 0;
        for n in 0..self.cfg.eps {
            let i = self.ep_in[n].int;
            if i & self.diepmsk & 0x7f7f != 0 || (i & epint::TXFE != 0 && self.diepempmsk & (1 << n) != 0) { v |= 1 << n; }
            if self.ep_out[n].int & self.doepmsk & 0xffff != 0 { v |= 1 << (16 + n); }
        }
        v
    }

    fn gintsts_now(&self) -> u32 {
        let mut v = self.gintsts & gint::W1C;
        if self.gotgint != 0 { v |= gint::OTGINT; }
        if self.dctl & dctl::GINSTS != 0 { v |= gint::GINAKEFF; }
        if self.dctl & dctl::GONSTS != 0 { v |= gint::BOUTNAKEFF; }
        let d = self.daint() & self.daintmsk;
        if d & 0xffff != 0 { v |= gint::IEPINT; }
        if d >> 16 != 0 { v |= gint::OEPINT; }
        if self.sof_due() { v |= gint::SOF; }
        v
    }

    /// The core's interrupt: GAHBCFG.GINT and an unmasked GINTSTS bit.
    pub fn irq(&self) -> bool { self.gahbcfg & GAHBCFG_GINT != 0 && self.gintsts_now() & self.gintmsk != 0 }

    fn dsts(&self) -> u32 {
        // DSTS.EnumSpd: 0 high speed, 1 full speed on a high-speed PHY, 3 full speed on a
        // full-speed PHY at 48 MHz (TinyUSB 0.21.0~6 dwc2_type.h, DSTS_ENUMSPD_FS)
        let spd = match self.speed { Some(Speed::High) | None => 0, Some(Speed::Full) => if self.cfg.high_speed { 1 } else { 3 } };
        let f = self.frame_ticks().max(1);
        // SOFFN: the (micro)frame number of the last SOF; at high speed it counts microframes
        let soffn = match self.sof_since { Some(s) if self.now >= s => ((self.now / f) & 0x3fff) as u32, _ => 0 };
        (self.suspended as u32) | spd << 1 | soffn << 8
    }

    fn write_epctl(&mut self, ep: usize, out: bool, v: u32) {
        use epctl::*;
        let e = if out { &mut self.ep_out[ep] } else { &mut self.ep_in[ep] };
        let old = e.ctl;
        let rw = match (ep, out) { (0, false) => 3 | STALL | TXFNUM, (0, true) => SNPM | STALL, (_, false) => MPSIZ | USBAEP | EPTYP | STALL | TXFNUM, _ => MPSIZ | USBAEP | EPTYP | SNPM | STALL };
        let mut ctl = (old & !rw) | (v & rw);
        // A control endpoint's STALL is set by software and cleared by the core on a SETUP.
        if ep == 0 { ctl |= old & STALL; }
        if v & SNAK != 0 { ctl |= NAKSTS; if !out { e.int |= epint::INEPNE; } }
        if v & CNAK != 0 { ctl &= !NAKSTS; }
        if v & SD0PID != 0 { e.pid = false; }
        if v & SD1PID != 0 { e.pid = true; }
        if v & EPENA != 0 { ctl |= EPENA; }
        // EPDIS asks to disable an enabled endpoint (control OUT 0 cannot be); it stays set until
        // the disable can complete (`finish_disable`).
        if v & EPDIS != 0 && old & EPENA != 0 && !(out && ep == 0) { ctl |= EPDIS; }
        if ep == 0 { ctl |= USBAEP; }
        e.ctl = ctl & !(SNAK | CNAK | SD0PID | SD1PID);
        self.finish_disable(ep, out);
        if !out && ctl & EPENA != 0 && old & EPENA == 0 && self.tx_fifo_bytes(ep) < self.mps(ep, false) {
            self.stats.fifo_errors += 1;
            if self.log { eprintln!("[usb_otg] IN{} enabled with a {}-byte TX FIFO for {}-byte packets", ep, self.tx_fifo_bytes(ep), self.mps(ep, false)); }
        }
    }

    /// A pending EPDIS completes (EPENA and EPDIS cleared, EPDISD) once its endpoint NAKs: an IN
    /// endpoint with NAK set (INEPNE), an OUT endpoint under the global OUT NAK (BOUTNAKEFF), the
    /// order dcd_dwc2's edpt_disable follows.
    fn finish_disable(&mut self, ep: usize, out: bool) {
        let gonak = self.dctl & dctl::GONSTS != 0;
        let e = if out { &mut self.ep_out[ep] } else { &mut self.ep_in[ep] };
        if e.ctl & epctl::EPDIS == 0 { return; }
        if if out { gonak } else { e.ctl & epctl::NAKSTS != 0 } { e.ctl &= !(epctl::EPENA | epctl::EPDIS); e.int |= epint::EPDISD; }
    }

    fn write_grstctl(&mut self, v: u32) {
        let old = self.grstctl;
        let new_id = self.cfg.gsnpsid >= GSNPSID_4_20A;
        if v & grstctl::CSRST != 0 && old & grstctl::CSRST == 0 {
            self.soft_reset();
            self.notify_pullup();
            // From 4.20a CSRST stays set until software clears it after CSRSTDONE.
            self.grstctl = if new_id { grstctl::CSRST | grstctl::CSRSTDONE } else { 0 };
        } else if new_id {
            let done = self.grstctl & grstctl::CSRSTDONE & !(v & grstctl::CSRSTDONE);
            self.grstctl = (v & grstctl::CSRST) | done;
        } else {
            self.grstctl = 0;
        }
        // The flushes finish at once (the model keeps no FIFO contents); TxFNum reads back.
        self.grstctl = (self.grstctl & (grstctl::CSRST | grstctl::CSRSTDONE)) | (v & grstctl::TXFNUM) | grstctl::AHBIDL;
    }

    fn write_dctl(&mut self, v: u32) {
        let mut d = (self.dctl & (dctl::GINSTS | dctl::GONSTS)) | (v & dctl::RW);
        if v & dctl::SGINAK != 0 { d |= dctl::GINSTS; }
        if v & dctl::CGINAK != 0 { d &= !dctl::GINSTS; }
        if v & dctl::SGONAK != 0 { d |= dctl::GONSTS; }
        if v & dctl::CGONAK != 0 { d &= !dctl::GONSTS; }
        let wake = d & dctl::RWUSIG != 0 && self.dctl & dctl::RWUSIG == 0;
        self.dctl = d;
        if d & dctl::GONSTS != 0 { for n in 1..self.cfg.eps { self.finish_disable(n, true); } }
        self.notify_pullup();
        if wake { let now = self.now; if let Some(l) = &mut self.link { l.device_remote_wakeup(now); } }
    }

    fn ep_regs(off: u32) -> Option<(usize, bool, u32)> {
        match off {
            0x900..=0xaff => Some((((off - reg::DIEP0) / reg::EP_STRIDE) as usize, false, off & 0x1f)),
            0xb00..=0xcff => Some((((off - reg::DOEP0) / reg::EP_STRIDE) as usize, true, off & 0x1f)),
            _ => None,
        }
    }

    fn deliver_setup(&mut self, mem: &mut dyn DmaMemory, pkt: [u8; 8]) {
        let o = &mut self.ep_out[0];
        let cnt = o.tsiz >> STUPCNT_SHIFT & 3;
        // Past three back-to-back SETUPs the last slot is written again (B2BSTUP).
        let at = if cnt == 0 { o.int |= epint::B2BSTUP; o.dma.wrapping_sub(8) } else { o.dma };
        if mem.write(at, &pkt).is_err() { o.int |= epint::AHBERR; self.stats.dma_faults += 1; return; }
        if cnt > 0 { o.dma = o.dma.wrapping_add(8); o.tsiz = (o.tsiz & !(3 << STUPCNT_SHIFT)) | (cnt - 1) << STUPCNT_SHIFT; }
        o.int |= epint::STPKTRX;
    }

    /// The token after a SETUP ends its setup phase: SETUP (DOEPINT0 bit 3), and the core gives
    /// control OUT 0 back to the CPU (EPENA cleared) so the driver can read the packet.
    fn end_setup_phase(&mut self, mem: &mut dyn DmaMemory) -> bool {
        if !self.setup_phase { return true; }
        if let Some(pkt) = self.setup_stash {
            if self.ep_out[0].ctl & epctl::EPENA == 0 { return false; }
            self.setup_stash = None;
            self.deliver_setup(mem, pkt);
        }
        self.setup_phase = false;
        self.ep_out[0].int |= epint::SETUP;
        self.ep_out[0].ctl &= !epctl::EPENA;
        true
    }
}

impl UsbFunction for Dwc2 {
    fn vbus(&mut self, _now: u64, on: bool) {
        if on && !self.vbus { self.gintsts |= gint::SRQINT; }
        if !on && self.vbus { self.gotgint |= GOTGINT_SEDET; self.set_speed(None); self.sof_since = None; self.suspended = false; }
        self.vbus = on;
    }
    fn pullup(&self) -> bool { Dwc2::pullup(self) }
    fn speed_capability(&self) -> Speed { if self.cfg.high_speed && self.dcfg & DCFG_DSPD == 0 { Speed::High } else { Speed::Full } }
    fn bus_reset(&mut self, _now: u64) {
        if self.suspended { self.gintsts |= gint::RSTDET; }
        self.gintsts |= gint::USBRST;
        self.suspended = false;
        self.sof_since = None;
        self.setup_phase = false;
        self.setup_stash = None;
        // USB reset clears USBActEP on every endpoint but 0.
        for e in self.ep_in.iter_mut().chain(self.ep_out.iter_mut()) { e.ctl &= !epctl::USBAEP; }
        self.ep_in[0].ctl |= epctl::USBAEP;
        self.ep_out[0].ctl |= epctl::USBAEP;
        if self.log { eprintln!("[usb_otg] bus reset"); }
    }
    fn enumerated(&mut self, _now: u64, speed: Speed) {
        self.set_speed(Some(speed));
        self.gintsts |= gint::ENUMDNE;
        if self.log { eprintln!("[usb_otg] enumeration done, {:?} speed", speed); }
    }
    fn bus_event(&mut self, now: u64, e: BusEvent) {
        match e {
            BusEvent::SofStart => { self.sof_since = Some(now); self.sof_ack = now; }
            BusEvent::SofStop => self.sof_since = None,
            BusEvent::EarlySuspend => self.gintsts |= gint::ESUSP,
            BusEvent::Suspend => { self.gintsts |= gint::USBSUSP; self.suspended = true; }
            BusEvent::Resume => { self.gintsts |= gint::WKUINT; self.suspended = false; }
        }
        if self.log { eprintln!("[usb_otg] {:?}", e); }
    }

    fn setup(&mut self, mem: &mut dyn DmaMemory, pkt: [u8; 8]) {
        self.stats.setups += 1;
        // A SETUP clears a STALL on the control endpoint (USB 2.0 §8.5.3.4) and the core sets NAK
        // on both control endpoints until the driver clears it.
        for e in [&mut self.ep_in[0], &mut self.ep_out[0]] { e.ctl = (e.ctl & !epctl::STALL) | epctl::NAKSTS; }
        self.ep_in[0].pid = true;
        self.ep_out[0].pid = true;
        self.setup_phase = true;
        if self.ep_out[0].ctl & epctl::EPENA != 0 { self.deliver_setup(mem, pkt); } else { self.setup_stash = Some(pkt); }
        if self.log { eprintln!("[usb_otg] SETUP {:02x?}", pkt); }
    }

    fn token_in(&mut self, mem: &mut dyn DmaMemory, ep: u8, max: usize) -> InReply {
        let n = ep as usize & 15;
        if n == 0 && !self.end_setup_phase(mem) { self.stats.naks += 1; return InReply::Nak; }
        if n >= self.cfg.in_eps { return InReply::Nak; }
        let e = self.ep_in[n];
        if e.ctl & epctl::STALL != 0 { self.stats.stalls += 1; return InReply::Stall; }
        let (xfer, pkt) = self.tsiz(n, false);
        let mps = self.mps(n, false);
        if e.ctl & epctl::EPENA == 0 || e.ctl & epctl::NAKSTS != 0 || self.dctl & dctl::GINSTS != 0 || pkt == 0 || self.tx_fifo_bytes(n) < mps.min(xfer as usize) {
            self.ep_in[n].int |= epint::NAK;
            self.stats.naks += 1;
            return InReply::Nak;
        }
        let len = (xfer as usize).min(mps).min(max);
        let mut data = vec![0u8; len];
        if len > 0 && mem.read(e.dma, &mut data).is_err() {
            self.ep_in[n].int |= epint::AHBERR;
            self.stats.dma_faults += 1;
            return InReply::Nak;
        }
        let pid = e.pid;
        let x = &mut self.ep_in[n];
        x.dma = x.dma.wrapping_add(len as u32);
        if x.ctl & epctl::EPTYP != epctl::EPTYP_ISO { x.pid = !x.pid; }
        self.set_tsiz(n, false, xfer - len as u32, pkt - 1);
        if pkt == 1 { let x = &mut self.ep_in[n]; x.int |= epint::XFRC; x.ctl &= !epctl::EPENA; }
        self.stats.packets_in += 1;
        self.stats.bytes_in += len as u64;
        if self.log { eprintln!("[usb_otg] IN{} {} bytes DATA{}{}", n, len, pid as u8, if pkt == 1 { ", transfer complete" } else { "" }); }
        InReply::Data(data, pid)
    }

    fn token_out(&mut self, mem: &mut dyn DmaMemory, ep: u8, data1: bool, data: &[u8]) -> Handshake {
        let n = ep as usize & 15;
        if n == 0 && !self.end_setup_phase(mem) { self.stats.naks += 1; return Handshake::Nak; }
        if n >= self.cfg.eps { return Handshake::Nak; }
        let e = self.ep_out[n];
        if e.ctl & epctl::STALL != 0 { self.stats.stalls += 1; return Handshake::Stall; }
        let (xfer, pkt) = self.tsiz(n, true);
        if e.ctl & epctl::EPENA == 0 || e.ctl & epctl::NAKSTS != 0 || self.dctl & dctl::GONSTS != 0 || pkt == 0 {
            let x = &mut self.ep_out[n];
            x.int |= epint::NAK;
            if n == 0 && e.ctl & epctl::EPENA == 0 { x.int |= epint::OTEPDIS; }
            self.stats.naks += 1;
            return Handshake::Nak;
        }
        // A data PID that does not match is a retry of a packet already taken: acknowledged and
        // dropped (USB 2.0 §8.6.4).
        if e.ctl & epctl::EPTYP != epctl::EPTYP_ISO && data1 != e.pid {
            self.stats.toggle_discards += 1;
            if self.log { eprintln!("[usb_otg] OUT{} DATA{} dropped, expected DATA{}", n, data1 as u8, e.pid as u8); }
            return Handshake::Ack;
        }
        let mps = self.mps(n, true);
        let len = data.len().min(xfer as usize);
        if len > 0 && mem.write(e.dma, &data[..len]).is_err() {
            self.ep_out[n].int |= epint::AHBERR;
            self.stats.dma_faults += 1;
            return Handshake::Nak;
        }
        let x = &mut self.ep_out[n];
        if data.len() > len { x.int |= epint::BBLEERR; }
        x.dma = x.dma.wrapping_add(len as u32);
        if x.ctl & epctl::EPTYP != epctl::EPTYP_ISO { x.pid = !x.pid; }
        self.set_tsiz(n, true, xfer - len as u32, pkt - 1);
        // A short packet or the last of the count ends the transfer; the endpoint then NAKs.
        let done = pkt == 1 || data.len() < mps;
        if done { let x = &mut self.ep_out[n]; x.int |= epint::XFRC; x.ctl = (x.ctl & !epctl::EPENA) | epctl::NAKSTS; }
        self.stats.packets_out += 1;
        self.stats.bytes_out += len as u64;
        if self.log { eprintln!("[usb_otg] OUT{} {} bytes DATA{}{}", n, data.len(), data1 as u8, if done { ", transfer complete" } else { "" }); }
        Handshake::Ack
    }

    fn remote_wakeup(&self) -> bool { self.dctl & dctl::RWUSIG != 0 }
}

impl Device for Dwc2 {
    fn read(&mut self, off: u32) -> u32 {
        use reg::*;
        if let Some((n, out, r)) = Self::ep_regs(off) {
            if n >= if out { self.cfg.eps } else { self.cfg.in_eps } { return 0; }
            let e = if out { self.ep_out[n] } else { self.ep_in[n] };
            return match r {
                // control OUT 0's packet size is control IN 0's
                EPCTL if n == 0 && out => (e.ctl & !3) | (self.ep_in[0].ctl & 3),
                EPCTL => e.ctl | if n > 0 { (e.pid as u32) << 16 } else { 0 },
                // TXFE: the TX FIFO is empty between transactions
                EPINT => e.int | if out { 0 } else { epint::TXFE },
                EPTSIZ => e.tsiz,
                EPDMA => e.dma,
                DTXFSTS if !out => (self.tx_fifo_bytes(n) / 4) as u32,
                _ => 0,
            };
        }
        match off {
            GOTGCTL => (self.gotgctl & gotgctl::RW) | gotgctl::CIDSTS | (self.bvalid() as u32) << 19
                | if self.gotgctl & gotgctl::AVALOEN != 0 && self.gotgctl & gotgctl::AVALOVAL != 0 { gotgctl::ASVLD } else { 0 },
            GOTGINT => self.gotgint,
            GAHBCFG => self.gahbcfg,
            GUSBCFG => self.gusbcfg,
            GRSTCTL => self.grstctl,
            GINTSTS => self.gintsts_now(),
            GINTMSK => self.gintmsk,
            GRXSTSR | GRXSTSP => 0,
            GRXFSIZ => self.grxfsiz,
            GNPTXFSIZ => self.gnptxfsiz,
            // non-periodic TX queue: four free entries, the FIFO empty
            GNPTXSTS => 4 << 16 | self.gnptxfsiz >> 16,
            GSNPSID => self.cfg.gsnpsid,
            0x044..=0x050 => self.cfg.ghwcfg[((off - GHWCFG1) / 4) as usize],
            GDFIFOCFG => self.gdfifocfg,
            0x104..=0x13c => { let i = ((off - DIEPTXF1) / 4) as usize; if i + 1 < self.cfg.in_eps { self.dieptxf[i] } else { 0 } }
            DCFG => self.dcfg,
            DCTL => self.dctl,
            DSTS => self.dsts(),
            DIEPMSK => self.diepmsk,
            DOEPMSK => self.doepmsk,
            DAINT => self.daint(),
            DAINTMSK => self.daintmsk,
            DIEPEMPMSK => self.diepempmsk,
            DEACHINT => 0,
            PCGCCTL => self.pcgcctl,
            _ => self.ram.read(off),
        }
    }

    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        use reg::*;
        if self.held { return WriteEffect::NONE; }
        if let Some((n, out, r)) = Self::ep_regs(off) {
            if n >= if out { self.cfg.eps } else { self.cfg.in_eps } { return WriteEffect::NONE; }
            match r {
                EPCTL => self.write_epctl(n, out, v),
                EPINT => { let e = if out { &mut self.ep_out[n] } else { &mut self.ep_in[n] }; e.int &= !v; }
                EPTSIZ => {
                    // bits 30:29: STUPCNT on control OUT 0, MC / RxDPID elsewhere; none on IN 0
                    let upper = if n == 0 && !out { 0 } else { 3 << 29 };
                    let e = if out { &mut self.ep_out[n] } else { &mut self.ep_in[n] };
                    e.tsiz = v & upper;
                    self.set_tsiz(n, out, v, v >> PKTCNT_SHIFT);
                }
                EPDMA => { if out { self.ep_out[n].dma = v } else { self.ep_in[n].dma = v } }
                _ => {}
            }
            return WriteEffect::NONE;
        }
        match off {
            GOTGCTL => { self.gotgctl = v & gotgctl::RW; self.notify_pullup(); }
            GOTGINT => self.gotgint &= !v,
            GAHBCFG => self.gahbcfg = v & 0x01e0_01bf,
            GUSBCFG => { self.gusbcfg = v; self.notify_pullup(); }
            GRSTCTL => self.write_grstctl(v),
            GINTSTS => { self.gintsts &= !(v & gint::W1C); if v & gint::SOF != 0 { self.sof_ack = self.now; } }
            GINTMSK => self.gintmsk = v,
            GRXFSIZ => self.grxfsiz = v & 0xffff,
            GNPTXFSIZ => self.gnptxfsiz = v,
            GDFIFOCFG => self.gdfifocfg = v,
            0x104..=0x13c => { let i = ((off - DIEPTXF1) / 4) as usize; if i + 1 < self.cfg.in_eps { self.dieptxf[i] = v; } }
            GSNPSID | 0x044..=0x050 | GRXSTSR | GRXSTSP | GNPTXSTS | DSTS | DAINT | DEACHINT => {}
            DCFG => self.dcfg = v,
            DCTL => self.write_dctl(v),
            DIEPMSK => self.diepmsk = v,
            DOEPMSK => self.doepmsk = v,
            DAINTMSK => self.daintmsk = v,
            DIEPEMPMSK => self.diepempmsk = v & 0xffff,
            PCGCCTL => self.pcgcctl = v,
            _ => self.ram.write(off, v),
        }
        WriteEffect::NONE
    }

    fn irq_sources(&self) -> u64 { self.irq() as u64 }

    /// Time runs only while a cable is attached; the core itself never needs a clock.
    fn clock(&self) -> Option<ClockDomain> { self.link.as_ref().map(|_| ClockDomain::Cpu) }

    fn tick(&mut self, ticks: u64) {
        self.now += ticks;
        if let Some(l) = &mut self.link { if l.tick_to(self.now) { self.pending = true; } }
    }

    fn has_deadline(&self) -> bool { true }

    /// The link's next event, and the next SOF while its interrupt is unmasked.
    fn next_deadline(&self) -> Option<u64> {
        let link = self.link.as_ref()?.wake_at();
        let sof = (self.sof_since.is_some() && self.gintmsk & gint::SOF != 0 && self.gahbcfg & GAHBCFG_GINT != 0).then(|| { let f = self.frame_ticks().max(1); (self.now / f + 1) * f });
        let t = match (link, sof) { (Some(a), Some(b)) => a.min(b), (a, b) => a.or(b)? };
        Some(t.saturating_sub(self.now))
    }

    fn debug(&mut self, on: bool) { self.log = on; if let Some(l) = &mut self.link { l.log = on; } }
}

#[cfg(test)]
mod tests;
