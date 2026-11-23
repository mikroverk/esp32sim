//! The USB-OTG core and the SDMMC host on the S3 bus: the shared PHY, the card's slot through the
//! GPIO matrix, card detect, their interrupt sources and what their DMA reaches.
use super::*;
use esp_periph::usb_otg::Request;

const USB_OTG: u32 = 0x6008_0000;
const SDMMC: u32 = 0x6002_8000;
const RTC_USB_CONF: u32 = 0x6000_8120;
const USB_WRAP_OTG_CONF: u32 = 0x6003_9000;
const GPIO_FUNC_IN_SEL: u32 = 0x6000_4154;
const GPIO_FUNC_OUT_SEL: u32 = 0x6000_4554;

fn bus() -> SocBus { SocBus::new(1 << 20, 0, [0; 6]) }

/// VBUS on and the core out of soft disconnect: whether the host then sees D+ pulled up.
fn powered(b: &mut SocBus) {
    esp_soc::SocBus::attach_usb_host(b, Default::default()).unwrap();
    esp_soc::SocBus::usb_link(b).unwrap().submit(Request::Attach).unwrap();
    b.tick(64);
    b.flush_ticks();
    assert!(esp_soc::SocBus::usb_link(b).unwrap().take_completion().is_some(), "VBUS on at the next device tick");
    assert_eq!(b.read32(USB_OTG).unwrap() >> 19 & 1, 1, "GOTGCTL.BSESVLD");
    b.write32(USB_OTG + 0x804, 0).unwrap();
}
fn connected(b: &mut SocBus) -> bool { esp_soc::SocBus::usb_link(b).unwrap().port().connected }

/// At reset the internal PHY is USB-Serial-JTAG's: the OTG core's pull-up does not reach the
/// host until RTC_CNTL_USB_CONF hands the PHY over (as `usb_wrap_ll_phy_enable_external(false)`
/// does), and USB-Serial-JTAG loses its host meanwhile. An external PHY (USB_WRAP PHY_SEL) has
/// nothing on it.
#[test]
fn the_otg_core_reaches_the_cable_only_through_the_internal_phy() {
    let mut b = bus();
    powered(&mut b);
    assert!(!connected(&mut b) && b.periph.usb.connected, "the PHY serves USB-Serial-JTAG");
    b.write32(RTC_USB_CONF, 1 << 20 | 1 << 19).unwrap();
    assert!(connected(&mut b) && !b.periph.usb.connected, "SW_HW_USB_PHY_SEL + SW_USB_PHY_SEL: the PHY is the core's");
    b.write32(USB_WRAP_OTG_CONF, 1 << 2).unwrap();
    assert!(!connected(&mut b), "USB_WRAP PHY_SEL picks the external PHY");
    b.write32(USB_WRAP_OTG_CONF, 0).unwrap();
    b.write32(RTC_USB_CONF, 1 << 20).unwrap();
    assert!(!connected(&mut b) && b.periph.usb.connected, "back to USB-Serial-JTAG");
}

/// Without SW_HW_USB_PHY_SEL the eFuse USB_PHY_SEL (EFUSE_RD_REPEAT_DATA1 bit 25) decides.
#[test]
fn the_efuse_hands_the_phy_to_otg_when_software_does_not_choose() {
    let mut b = bus();
    b.periph.efuse.ram.write(0x34, 1 << 25);
    powered(&mut b);
    assert!(connected(&mut b) && !b.periph.usb.connected);
    b.write32(RTC_USB_CONF, 1 << 20).unwrap();
    assert!(!connected(&mut b) && b.periph.usb.connected, "software takes the choice back");
}

/// A chip reset keeps the cable and RTC_CNTL_USB_CONF, so the PHY stays the core's; the reset
/// core is soft-disconnected again until the firmware connects.
#[test]
fn a_chip_reset_keeps_the_usb_cable_and_the_phy_owner() {
    let mut b = bus();
    powered(&mut b);
    b.write32(RTC_USB_CONF, 1 << 20 | 1 << 19).unwrap();
    assert!(connected(&mut b));
    esp_soc::SocBus::reboot(&mut b, [0; 6]);
    let p = esp_soc::SocBus::usb_link(&mut b).unwrap().port();
    assert!(p.power && !p.connected, "VBUS stays, the reset core is soft-disconnected");
    b.write32(USB_OTG + 0x804, 0).unwrap();
    assert!(connected(&mut b) && !b.periph.usb.connected);
}

/// The core's identity and FIFO RAM are the S3's full-speed instance (usb_dwc_cfg.h).
#[test]
fn the_otg_core_is_the_full_speed_instance() {
    let mut b = bus();
    assert_eq!(b.read32(USB_OTG + 0x40).unwrap(), 0x4F54_400A, "GSNPSID 4.00a");
    assert_eq!(b.read32(USB_OTG + 0x48).unwrap(), 0x224D_D930, "GHWCFG2");
    assert_eq!(b.read32(USB_OTG + 0x24).unwrap(), 256, "GRXFSIZ: OTG_RX_DFIFO_DEPTH");
    assert_eq!(b.read32(USB_OTG + 0x28).unwrap(), 256 << 16 | 256, "GNPTXFSIZ");
}

/// CMD8 with the given CMD card number, and the RINTSTS it leaves.
fn cmd8(b: &mut SocBus, slot: u32) -> u32 {
    b.write32(SDMMC + 0x44, 0xffff_ffff).unwrap();
    b.write32(SDMMC + 0x28, 0x1aa).unwrap();
    b.write32(SDMMC + 0x2c, 1 << 31 | slot << 16 | 1 << 6 | 8).unwrap();
    assert_eq!(b.read32(SDMMC + 0x2c).unwrap() >> 31, 0, "taken within the write");
    b.read32(SDMMC + 0x44).unwrap()
}

/// The card answers on the slot whose clock (SDHOST_CCLK_OUT_2, signal 173) the GPIO matrix puts
/// on a pad, and the host's interrupt is source 30.
#[test]
fn the_card_answers_on_the_slot_the_matrix_clocks() {
    let mut b = bus();
    esp_soc::SocBus::attach_sd_card(&mut b, Box::new(esp_periph::sdmmc::MemStore(vec![0; 1 << 20])), Default::default()).unwrap();
    assert_eq!(cmd8(&mut b, 1) & (1 << 8), 1 << 8, "no clock on any pin: response timeout");
    b.write32(GPIO_FUNC_OUT_SEL + 4 * 36, 173).unwrap();
    assert_eq!(cmd8(&mut b, 1), 1 << 2, "CMD_DONE, no error");
    assert_eq!(b.read32(SDMMC + 0x30).unwrap(), 0x1aa, "R7 echoes the check pattern");
    assert_eq!(cmd8(&mut b, 0) & (1 << 8), 1 << 8, "nothing on slot 0");
    b.write32(SDMMC + 0x24, 1 << 2).unwrap();
    b.write32(SDMMC, 1 << 4).unwrap();
    let _ = cmd8(&mut b, 1);
    b.write32(SDMMC + 0x44, 0).unwrap();
    assert_eq!(b.periph.source_status()[0] >> crate::periph::SRC_SDIO_HOST & 1, 1, "source 30 asserted");
    b.write32(SDMMC + 0x44, 1 << 2).unwrap();
    assert_eq!(b.periph.source_status()[0] >> crate::periph::SRC_SDIO_HOST & 1, 0, "and released");
}

/// SYSTEM_PERIP_RST_EN1.SDIO_HOST_RST (bit 7) resets the host's registers when set, and they
/// ignore writes while it stays set, as the OTG core does under USB_RST.
#[test]
fn the_sdmmc_host_ignores_writes_while_held_in_reset() {
    const RST_EN1: u32 = 0x600c_0024;
    let mut b = bus();
    esp_soc::SocBus::attach_sd_card(&mut b, Box::new(esp_periph::sdmmc::MemStore(vec![0; 1 << 20])), Default::default()).unwrap();
    b.write32(GPIO_FUNC_OUT_SEL + 4 * 36, 173).unwrap();
    b.write32(SDMMC + 0x18, 1).unwrap();
    assert_eq!(b.read32(SDMMC + 0x18).unwrap(), 1, "CTYPE: slot 0 4-bit");
    b.write32(RST_EN1, 1 << 7).unwrap();
    assert_eq!(b.read32(SDMMC + 0x18).unwrap(), 0, "setting the bit resets CTYPE");
    b.write32(SDMMC + 0x18, 1).unwrap();
    assert_eq!(b.read32(SDMMC + 0x18).unwrap(), 0, "held: the write is ignored");
    b.write32(RST_EN1, 1 << 7).unwrap();
    assert_eq!(cmd8(&mut b, 1), 0, "held: no command starts, RINTSTS stays clear");
    assert_eq!(b.periph.sdmmc.stats.commands, 0);
    b.write32(RST_EN1, 0).unwrap();
    b.write32(SDMMC + 0x18, 1).unwrap();
    assert_eq!(b.read32(SDMMC + 0x18).unwrap(), 1, "released: writes take effect");
    assert_eq!(cmd8(&mut b, 1), 1 << 2, "and the card answers");
}

/// CDETECT and WRTPRT read the card-detect and write-protect signals of the matrix: the constant
/// inputs IDF ties them to without such pins (zero; one inverted), or a pin.
#[test]
fn card_detect_and_write_protect_follow_the_matrix() {
    let mut b = bus();
    assert_eq!(b.read32(SDMMC + 0x50).unwrap(), 0, "reset: constant zero, both slots detect a card");
    b.write32(GPIO_FUNC_IN_SEL + 4 * 195, 0x80 | 0x38).unwrap();
    assert_eq!(b.read32(SDMMC + 0x50).unwrap(), 2, "slot 1 tied high: no card");
    b.write32(GPIO_FUNC_IN_SEL + 4 * 197, 0x80 | 0x40 | 0x38).unwrap();
    assert_eq!(b.read32(SDMMC + 0x54).unwrap(), 0, "write protect: one, inverted");
    b.write32(GPIO_FUNC_IN_SEL + 4 * 196, 0x80 | 5).unwrap();
    assert_eq!(b.read32(SDMMC + 0x54).unwrap(), 1, "slot 0 from GPIO5, floating high");
}

/// The DMA reaches internal SRAM on the data bus; a write moves the page versions so no decoded
/// code there survives; anything else is a bus error.
#[test]
fn the_dma_reaches_internal_sram_only() {
    use esp_periph::sdmmc::SdmmcMemory;
    let mut b = bus();
    let vbase = b.ver_base[SRC_SRAM as usize] as usize;
    let page = ((0x3FC9_0000 - DRAM_LOW) as usize + 0x8000) >> VPAGE_SHIFT;
    let before = b.page_ver[vbase + page];
    let mut d = SramDma { sram: &mut b.sram, page_ver: &mut b.page_ver, vbase };
    d.write(0x3FC9_0000, &[1, 2, 3, 4]).unwrap();
    let mut r = [0u8; 4];
    d.read(0x3FC9_0000, &mut r).unwrap();
    assert_eq!(r, [1, 2, 3, 4]);
    assert!(d.read(0x3FCF_FFFE, &mut r).is_err(), "past the end of SRAM");
    assert!(d.write(0x4037_8000, &r).is_err(), "the instruction bus is out of its reach");
    assert!(d.read(0x3C00_0000, &mut r).is_err(), "so is the cache window");
    assert_ne!(b.page_ver[vbase + page], before);
    assert_eq!(b.read32(0x3FC9_0000).unwrap(), 0x0403_0201);
}
