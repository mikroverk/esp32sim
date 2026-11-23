//! Host software for the USB cable (`esp_periph::usb_otg::UsbLink`): what a host's USB
//! stack does with a device plugged into its root port, written against the link's requests and
//! the time they take, so the device firmware runs between them.
//!
//! `UsbHost` powers the port, waits for the device to connect, debounces it, resets it and
//! enumerates it (USB 2.0 §9.1.2, chapter 9 requests), then moves control and bulk transfers.
//! `msc::Msc` runs SCSI over bulk-only transport on top, `fat32` writes and reads files on the
//! volume the device presents, `script` drives all of it from a command file (`--usb-host`).
//!
//! Time: the host learns that a request finished at the next microframe boundary after it did
//! (the one-microframe interrupt threshold Linux's EHCI driver programs, USBCMD.ITC) and submits
//! the next one there; host-side waits (debounce, reset recovery) are emulated time. The port
//! the host runs on (`UsbPort`) lets emulated time pass: a `Machine` runs its firmware meanwhile.
pub mod fat32;
pub mod msc;
pub mod script;
#[cfg(test)]
mod tests;

use crate::machine::Machine;
use crate::soc::{Soc, SocBus, Stop};
use esp_periph::usb_otg::{Completion, Outcome, Request, Speed, UsbLink};

/// What the host needs from the machine it is plugged into.
pub trait UsbPort {
    /// The cable, if the chip has a host attached.
    fn link(&mut self) -> Option<&mut UsbLink>;
    /// Let emulated time run to `t_ns`: the device, its firmware and the link meanwhile.
    fn run_until(&mut self, t_ns: u64) -> Result<(), String>;
    /// A line typed at the firmware's console (UART0).
    fn console(&mut self, _text: &str) {}
}

impl<S: Soc> UsbPort for Machine<S> {
    fn link(&mut self) -> Option<&mut UsbLink> { self.bus.usb_link() }

    /// A chip reset is taken through the ROM, as a normal run does; the run's time limit holds.
    fn run_until(&mut self, t_ns: u64) -> Result<(), String> {
        let mut insns = u64::MAX;
        run_machine_until(self, t_ns, &mut insns, |m| { m.reboot(); Ok(()) })
    }

    fn console(&mut self, text: &str) { self.bus.uart_input(0, text.as_bytes()); }
}

/// Run `m` to `t_ns` of emulated time in slices that end exactly there (`max_cycles`), within
/// the run's time limit and `insns` instructions (counted down). A chip reset goes to `reset`,
/// which comes back up or says why the run ends.
pub fn run_machine_until<S: Soc>(m: &mut Machine<S>, t_ns: u64, insns: &mut u64, mut reset: impl FnMut(&mut Machine<S>) -> Result<(), String>) -> Result<(), String> {
    let target = (t_ns as u128 * S::CPU_HZ as u128).div_ceil(1_000_000_000) as u64;
    let limit = m.max_cycles;
    while m.bus.cycles() < target {
        if m.bus.cycles() >= limit { return Err(format!("the run's time limit ({:.3} s)", limit as f64 / S::CPU_HZ as f64)); }
        if *insns == 0 { return Err("the run's instruction limit".into()); }
        let slice = target.min(limit);
        m.max_cycles = slice;
        let before = m.run_steps();
        let stop = m.run(*insns);
        *insns = insns.saturating_sub(m.run_steps().saturating_sub(before));
        let script_stop = m.max_cycles != slice;
        m.max_cycles = limit;
        if script_stop { m.max_cycles = 0; return Err("the script stopped the run".into()); }
        match stop {
            Stop::Halted | Stop::MaxInsns => {}
            Stop::SwReset => {
                let cause = m.bus.reset_cause();
                eprintln!("[emu] chip reset at t={:.3}s: cause {:#x} ({})", m.seconds(), cause, esp_periph::reset_cause_name(cause));
                reset(m)?;
            }
            s => return Err(format!("the run stopped: {s:?}")),
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UsbError {
    /// the endpoint STALLed (data received before it)
    Stall(Vec<u8>),
    Babble,
    Disconnected,
    NotRunning,
    Timeout,
    /// a descriptor or a reply that makes no sense
    Protocol(String),
    /// the machine under the host stopped (time limit, a crash)
    Port(String),
    NoLink,
}
impl std::fmt::Display for UsbError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            UsbError::Stall(_) => write!(f, "STALL"), UsbError::Babble => write!(f, "babble"), UsbError::Disconnected => write!(f, "device disconnected"),
            UsbError::NotRunning => write!(f, "port not enabled"), UsbError::Timeout => write!(f, "timeout"), UsbError::Protocol(s) => write!(f, "{s}"),
            UsbError::Port(s) => write!(f, "{s}"), UsbError::NoLink => write!(f, "no USB host attached"),
        }
    }
}

/// Standard request codes and descriptor types (USB 2.0 §9.4, Tables 9-4 and 9-5).
pub mod req {
    pub const GET_STATUS: u8 = 0; pub const CLEAR_FEATURE: u8 = 1; pub const SET_FEATURE: u8 = 3; pub const SET_ADDRESS: u8 = 5;
    pub const GET_DESCRIPTOR: u8 = 6; pub const SET_CONFIGURATION: u8 = 9;
    pub const DEVICE: u8 = 1; pub const CONFIGURATION: u8 = 2; pub const STRING: u8 = 3; pub const INTERFACE: u8 = 4; pub const ENDPOINT: u8 = 5;
    pub const DEVICE_QUALIFIER: u8 = 6;
    /// feature selectors (Table 9-6)
    pub const ENDPOINT_HALT: u16 = 0; pub const DEVICE_REMOTE_WAKEUP: u16 = 1;
}

pub fn setup_packet(request_type: u8, request: u8, value: u16, index: u16, length: u16) -> [u8; 8] {
    let (v, i, l) = (value.to_le_bytes(), index.to_le_bytes(), length.to_le_bytes());
    [request_type, request, v[0], v[1], i[0], i[1], l[0], l[1]]
}

/// The device descriptor (USB 2.0 §9.6.1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceDescriptor {
    pub usb: u16, pub class: u8, pub subclass: u8, pub protocol: u8, pub mps0: u8, pub vendor: u16, pub product: u16, pub release: u16,
    pub manufacturer: u8, pub product_string: u8, pub serial: u8, pub configurations: u8,
}
impl DeviceDescriptor {
    pub fn parse(d: &[u8]) -> Result<Self, UsbError> {
        if d.len() < 18 || d[0] < 18 || d[1] != req::DEVICE { return Err(UsbError::Protocol(format!("not a device descriptor: {:02x?}", d))); }
        let w = |i: usize| u16::from_le_bytes([d[i], d[i + 1]]);
        Ok(DeviceDescriptor { usb: w(2), class: d[4], subclass: d[5], protocol: d[6], mps0: d[7], vendor: w(8), product: w(10), release: w(12),
                              manufacturer: d[14], product_string: d[15], serial: d[16], configurations: d[17] })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EndpointInfo { pub address: u8, pub attributes: u8, pub mps: u16, pub interval: u8 }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfaceInfo { pub number: u8, pub alternate: u8, pub class: u8, pub subclass: u8, pub protocol: u8, pub endpoints: Vec<EndpointInfo> }
/// A configuration descriptor and what follows it (§9.6.3-§9.6.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigInfo { pub value: u8, pub attributes: u8, pub max_power_ma: u16, pub interfaces: Vec<InterfaceInfo> }
impl ConfigInfo {
    pub fn parse(d: &[u8]) -> Result<Self, UsbError> {
        if d.len() < 9 || d[1] != req::CONFIGURATION { return Err(UsbError::Protocol(format!("not a configuration descriptor: {:02x?}", &d[..d.len().min(9)]))); }
        let mut c = ConfigInfo { value: d[5], attributes: d[7], max_power_ma: d[8] as u16 * 2, interfaces: Vec::new() };
        let mut i = d[0] as usize;
        while i + 2 <= d.len() {
            let (len, kind) = (d[i] as usize, d[i + 1]);
            if len < 2 || i + len > d.len() { return Err(UsbError::Protocol(format!("descriptor at {i} runs past the configuration"))); }
            let b = &d[i..i + len];
            match kind {
                req::INTERFACE if len >= 9 => c.interfaces.push(InterfaceInfo { number: b[2], alternate: b[3], class: b[5], subclass: b[6], protocol: b[7], endpoints: Vec::new() }),
                req::ENDPOINT if len >= 7 => {
                    let ep = EndpointInfo { address: b[2], attributes: b[3], mps: u16::from_le_bytes([b[4], b[5]]) & 0x7ff, interval: b[6] };
                    if let Some(f) = c.interfaces.last_mut() { f.endpoints.push(ep); }
                }
                _ => {}
            }
            i += len;
        }
        Ok(c)
    }
}

/// Host-side timings (USB 2.0 §7.1.7.3, §9.2.6.2, §9.2.6.3) and limits.
#[derive(Clone, Copy, Debug)]
pub struct HostTiming {
    /// connect debounce TATTDB, 100 ms
    pub debounce_ns: u64,
    /// reset recovery TRSTRCY, 10 ms
    pub reset_recovery_ns: u64,
    /// SetAddress() recovery TDSETADDR, 2 ms
    pub set_address_ns: u64,
    /// how long a control transfer may take (Linux USB_CTRL_GET_TIMEOUT, 5 s)
    pub control_timeout_ns: u64,
    /// how often the root hub's port status is looked at while waiting for a connect
    pub port_poll_ns: u64,
}
impl Default for HostTiming {
    fn default() -> Self { HostTiming { debounce_ns: 100_000_000, reset_recovery_ns: 10_000_000, set_address_ns: 2_000_000, control_timeout_ns: 5_000_000_000, port_poll_ns: 1_000_000 } }
}

pub struct UsbHost<'p> {
    port: &'p mut dyn UsbPort,
    pub timing: HostTiming,
    pub address: u8,
    pub mps0: u16,
    pub speed: Option<Speed>,
    pub device: Option<DeviceDescriptor>,
    pub config: Option<ConfigInfo>,
    pub strings: Vec<(u8, String)>,
    /// print what happens on stderr (`[usb-host] ...`)
    pub log: bool,
}

/// The microframe the host software works in.
pub const UFRAME_NS: u64 = 125_000;

impl<'p> UsbHost<'p> {
    pub fn new(port: &'p mut dyn UsbPort) -> Self {
        UsbHost { port, timing: HostTiming::default(), address: 0, mps0: 64, speed: None, device: None, config: None, strings: Vec::new(), log: false }
    }

    pub fn port(&mut self) -> &mut dyn UsbPort { &mut *self.port }

    fn link(&mut self) -> Result<&mut UsbLink, UsbError> { self.port.link().ok_or(UsbError::NoLink) }

    pub fn now_ns(&mut self) -> u64 { self.port.link().map_or(0, |l| l.now_ns()) }

    pub fn say(&mut self, what: std::fmt::Arguments) {
        if self.log { let t = self.now_ns(); eprintln!("[usb-host] t={}.{:06} {}", t / 1_000_000_000, t / 1000 % 1_000_000, what); }
    }

    /// Let `ns` of emulated time pass with nothing new on the bus.
    pub fn idle_ns(&mut self, ns: u64) -> Result<(), UsbError> { let t = self.now_ns() + ns; self.port.run_until(t).map_err(UsbError::Port) }

    /// Time to the start of the next microframe.
    fn next_uframe(&mut self) -> u64 { (self.now_ns() / UFRAME_NS + 1) * UFRAME_NS }

    /// Submit `req` and wait for it, looking once a microframe; past `timeout_ns` it is cancelled.
    pub fn request(&mut self, req: Request, timeout_ns: u64) -> Result<Completion, UsbError> {
        let deadline = self.now_ns() + timeout_ns;
        self.link()?.submit(req).map_err(|e| UsbError::Protocol(e.into()))?;
        loop {
            if let Some(c) = self.link()?.take_completion() { return Ok(c); }
            if self.now_ns() >= deadline {
                let l = self.link()?;
                l.cancel();
                l.take_completion();
                return Err(UsbError::Timeout);
            }
            let t = self.next_uframe().min(deadline);
            self.port.run_until(t).map_err(UsbError::Port)?;
        }
    }

    fn data(c: Completion) -> Result<Vec<u8>, UsbError> {
        match c.outcome {
            Outcome::Done(d) => Ok(d),
            Outcome::Stall(d) => Err(UsbError::Stall(d)),
            Outcome::Babble(_) => Err(UsbError::Babble),
            Outcome::Disconnected => Err(UsbError::Disconnected),
            Outcome::NotRunning => Err(UsbError::NotRunning),
            Outcome::Cancelled => Err(UsbError::Timeout),
        }
    }

    pub fn power_on(&mut self) -> Result<(), UsbError> { let c = self.request(Request::Attach, 1_000_000)?; Self::data(c).map(|_| ()) }
    pub fn power_off(&mut self) -> Result<(), UsbError> {
        let c = self.request(Request::Detach, 1_000_000)?;
        self.address = 0; self.config = None; self.speed = None;
        Self::data(c).map(|_| ())
    }

    /// Wait for a device to pull D+ up, looking at the port every `port_poll_ns`.
    pub fn wait_connect(&mut self, timeout_ns: u64) -> Result<(), UsbError> {
        let deadline = self.now_ns() + timeout_ns;
        while !self.link()?.port().connected {
            if self.now_ns() >= deadline { return Err(UsbError::Timeout); }
            let t = (self.now_ns() + self.timing.port_poll_ns).min(deadline);
            self.port.run_until(t).map_err(UsbError::Port)?;
        }
        Ok(())
    }

    /// Port reset (SE0, chirp); returns the speed it ended in.
    pub fn reset(&mut self) -> Result<Speed, UsbError> {
        let c = self.request(Request::Reset, 1_000_000_000)?;
        let d = Self::data(c)?;
        let speed = if d.first() == Some(&(Speed::High as u8)) { Speed::High } else { Speed::Full };
        self.speed = Some(speed);
        self.address = 0;
        self.config = None;
        // EP0's packet size until the device descriptor gives it: 64 at high speed (fixed) and,
        // as a first guess, at full speed too, where the first GET_DESCRIPTOR(64) reads
        // bMaxPacketSize0 (Linux v6.12 drivers/usb/core/hub.c, hub_port_init). With 8 a device
        // whose EP0 takes 64 sends its 18-byte descriptor in one packet, more than the host takes.
        self.mps0 = 64;
        Ok(speed)
    }

    pub fn control(&mut self, setup: [u8; 8], data: Vec<u8>) -> Result<Vec<u8>, UsbError> {
        let to = self.timing.control_timeout_ns;
        let c = self.request(Request::Control { setup, data, mps: self.mps0 }, to)?;
        Self::data(c)
    }
    pub fn get_descriptor(&mut self, kind: u8, index: u8, lang: u16, len: u16) -> Result<Vec<u8>, UsbError> {
        self.control(setup_packet(0x80, req::GET_DESCRIPTOR, (kind as u16) << 8 | index as u16, lang, len), Vec::new())
    }
    pub fn set_address(&mut self, address: u8) -> Result<(), UsbError> {
        self.control(setup_packet(0x00, req::SET_ADDRESS, address as u16, 0, 0), Vec::new())?;
        self.address = address;
        let r = self.timing.set_address_ns;
        self.idle_ns(r)
    }
    pub fn set_configuration(&mut self, value: u8) -> Result<(), UsbError> {
        self.control(setup_packet(0x00, req::SET_CONFIGURATION, value as u16, 0, 0), Vec::new())?;
        // configuring resets every endpoint's data toggle to DATA0 (§9.1.1.5)
        self.link()?.reset_toggles();
        Ok(())
    }
    /// CLEAR_FEATURE(ENDPOINT_HALT), which also puts both ends' data toggle back to DATA0.
    pub fn clear_halt(&mut self, ep_addr: u8) -> Result<(), UsbError> {
        self.control(setup_packet(0x02, req::CLEAR_FEATURE, req::ENDPOINT_HALT, ep_addr as u16, 0), Vec::new())?;
        self.link()?.reset_toggle(ep_addr);
        Ok(())
    }

    pub fn string(&mut self, index: u8, lang: u16) -> Result<String, UsbError> {
        let d = self.get_descriptor(req::STRING, index, lang, 255)?;
        if d.len() < 2 || d[0] < 2 || d[1] != req::STRING { return Err(UsbError::Protocol(format!("string {index}: {:02x?}", d))); }
        let n = (d[0] as usize).min(d.len());
        let units: Vec<u16> = d[2..n].as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
        Ok(String::from_utf16_lossy(&units))
    }

    /// The first GET_DESCRIPTOR(DEVICE, 64) at address 0, read for bMaxPacketSize0 alone (Linux
    /// v6.12 drivers/usb/core/hub.c: get_bMaxPacketSize0, hub_port_init). With the host's
    /// 64-byte guess, a device whose EP0 takes 8 bytes ends the data stage with one short packet:
    /// the descriptor's first 8 bytes, as far as bMaxPacketSize0. A read shorter than that, or one
    /// that is not a device descriptor, is EPROTO there; a bMaxPacketSize0 other than 8, 16, 32 or
    /// 64 is "Invalid ep0 maxpacket" (EMSGSIZE). Linux retries both with a port reset in between,
    /// then gives up; the scripted host gives up at once.
    fn first_mps0(&mut self) -> Result<u8, UsbError> {
        let d = self.get_descriptor(req::DEVICE, 0, 0, 64)?;
        if d.len() < 8 || d[1] != req::DEVICE { return Err(UsbError::Protocol(format!("not a device descriptor: {:02x?}", &d[..d.len().min(18)]))); }
        if ![8, 16, 32, 64].contains(&d[7]) { return Err(UsbError::Protocol(format!("invalid bMaxPacketSize0 {}", d[7]))); }
        Ok(d[7])
    }

    /// Power, connect, debounce, reset, address and configure the device on the port, the way a
    /// host's hub driver does (USB 2.0 §9.1.2): bMaxPacketSize0 from the first read of the device
    /// descriptor at address 0 (`first_mps0`), SET_ADDRESS, the full device descriptor, the
    /// configuration, the strings, SET_CONFIGURATION.
    pub fn enumerate(&mut self, connect_timeout_ns: u64) -> Result<(), UsbError> {
        if !self.link()?.port().power { self.power_on()?; self.say(format_args!("VBUS on")); }
        self.wait_connect(connect_timeout_ns)?;
        let connected_at = self.link()?.port().change_ns;
        self.say(format_args!("device connected"));
        let settle = (connected_at + self.timing.debounce_ns).saturating_sub(self.now_ns());
        self.idle_ns(settle)?;
        let speed = self.reset()?;
        self.say(format_args!("port reset: {:?} speed", speed));
        let r = self.timing.reset_recovery_ns;
        self.idle_ns(r)?;
        self.mps0 = self.first_mps0()? as u16;
        self.set_address(1)?;
        self.say(format_args!("address 1"));
        let dev = DeviceDescriptor::parse(&self.get_descriptor(req::DEVICE, 0, 0, 18)?)?;
        self.say(format_args!("device {:04x}:{:04x}, USB {:x}.{:02x}, class {:02x}/{:02x}/{:02x}, EP0 {} bytes, {} configuration(s)",
                              dev.vendor, dev.product, dev.usb >> 8, dev.usb & 0xff, dev.class, dev.subclass, dev.protocol, dev.mps0, dev.configurations));
        let head = self.get_descriptor(req::CONFIGURATION, 0, 0, 9)?;
        if head.len() < 9 { return Err(UsbError::Protocol("short configuration descriptor".into())); }
        let total = u16::from_le_bytes([head[2], head[3]]);
        let cfg = ConfigInfo::parse(&self.get_descriptor(req::CONFIGURATION, 0, 0, total)?)?;
        for f in &cfg.interfaces {
            let eps: Vec<String> = f.endpoints.iter().map(|e| format!("{:02x} ({} bytes)", e.address, e.mps)).collect();
            self.say(format_args!("interface {}: class {:02x}/{:02x}/{:02x}, endpoints {}", f.number, f.class, f.subclass, f.protocol, eps.join(" ")));
        }
        let langs = self.get_descriptor(req::STRING, 0, 0, 255)?;
        let lang = if langs.len() >= 4 { u16::from_le_bytes([langs[2], langs[3]]) } else { 0x0409 };
        self.strings.clear();
        for i in [dev.manufacturer, dev.product_string, dev.serial] {
            if i == 0 { continue; }
            let s = self.string(i, lang)?;
            self.say(format_args!("string {}: {:?}", i, s));
            self.strings.push((i, s));
        }
        self.set_configuration(cfg.value)?;
        self.say(format_args!("configuration {} set ({} mA)", cfg.value, cfg.max_power_ma));
        self.device = Some(dev);
        self.config = Some(cfg);
        Ok(())
    }

    /// usb_reset_device (Linux v6.12 drivers/usb/core/hub.c: usb_reset_and_verify_device and
    /// hub_port_init's new scheme): reset, GET_DESCRIPTOR(64) at address 0, reset again,
    /// SET_ADDRESS to the old address, the descriptors read again and compared with the old
    /// ones, then SET_CONFIGURATION with the old configuration. Each reset is followed by 50 ms
    /// (hub_port_reset: TRSTRCY 10 ms "plus some extra" 40 ms), SET_ADDRESS by 10 ms.
    pub fn reset_device(&mut self) -> Result<(), UsbError> {
        let (dev, cfg) = match (self.device.clone(), self.config.clone()) { (Some(d), Some(c)) => (d, c), _ => return Err(UsbError::Protocol("no configured device to reset".into())) };
        let address = self.address.max(1);
        // A known device keeps its EP0 packet size through the resets: hub_port_init guesses only
        // for a new one (`initial`).
        self.reset()?;
        self.mps0 = dev.mps0 as u16;
        self.idle_ns(50_000_000)?;
        if self.first_mps0()? != dev.mps0 { return Err(UsbError::Protocol("the reset changed bMaxPacketSize0".into())); }
        self.reset()?;
        self.mps0 = dev.mps0 as u16;
        self.idle_ns(50_000_000)?;
        self.set_address(address)?;
        self.idle_ns(10_000_000)?;
        let now = DeviceDescriptor::parse(&self.get_descriptor(req::DEVICE, 0, 0, 18)?)?;
        let head = self.get_descriptor(req::CONFIGURATION, 0, 0, 9)?;
        if head.len() < 9 { return Err(UsbError::Protocol("short configuration descriptor".into())); }
        let total = u16::from_le_bytes([head[2], head[3]]);
        let again = ConfigInfo::parse(&self.get_descriptor(req::CONFIGURATION, 0, 0, total)?)?;
        if now != dev || again != cfg { return Err(UsbError::Protocol("the device's descriptors changed".into())); }
        self.set_configuration(cfg.value)?;
        self.say(format_args!("device reset, configuration {} restored", cfg.value));
        self.device = Some(dev);
        self.config = Some(cfg);
        Ok(())
    }

    pub fn bulk_out(&mut self, ep: u8, mps: u16, data: Vec<u8>, timeout_ns: u64) -> Result<(), UsbError> {
        let c = self.request(Request::BulkOut { ep: ep & 15, data, mps, zlp: false }, timeout_ns)?;
        Self::data(c).map(|_| ())
    }
    pub fn bulk_in(&mut self, ep: u8, mps: u16, len: usize, timeout_ns: u64) -> Result<Vec<u8>, UsbError> {
        let c = self.request(Request::BulkIn { ep: ep & 15, len, mps }, timeout_ns)?;
        Self::data(c)
    }

    /// Stop SOFs; the device suspends after 3 ms of it (§7.1.7.6).
    pub fn suspend(&mut self) -> Result<(), UsbError> { let c = self.request(Request::Suspend, 1_000_000)?; Self::data(c).map(|_| ()) }
    /// Drive resume for 20 ms, then SOFs again (§7.1.7.7).
    pub fn resume(&mut self) -> Result<(), UsbError> { let c = self.request(Request::Resume, 100_000_000)?; Self::data(c).map(|_| ()) }
}
