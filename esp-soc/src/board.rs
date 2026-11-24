//! Boards around the SoC. The SoC model emits generic events (GPIO edges, RMT symbol streams,
//! SPI bytes, LCD frames, camera requests); a `BoardModel` interprets them as the devices wired to
//! the pins and offers what the UI and the scripts need back.
use esp_periph::i2c::I2cDevice;

pub type VirtualCycle = u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoardEdge {
    pub cycle: VirtualCycle,
    pub pin: u8,
    pub level: bool,
}

/// Physical routes for a single-lane SPI transaction. Output masks allow mirrored routes.
/// `cs` contains asserted active-low hardware selects and low, enabled software GPIO selects.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SpiPins {
    pub sclk: u64,
    pub mosi: u64,
    pub miso: Option<u8>,
    pub cs: u64,
}

/// What a board does with the SoC's pin-level activity.
pub trait BoardModel {
    fn name(&self) -> &'static str;
    /// GPIO output level changes, in order.
    fn gpio_changes(&mut self, _changes: &[(u8, bool)]) {}
    /// Output changes at the SoC bus cycle. The drive masks also expose output-enable
    /// transitions (including releasing a low pin), needed by open-drain pulse protocols.
    /// Called immediately after a GPIO output/enable write on chips with pin transport.
    fn gpio_output_at(&mut self, _cycle: VirtualCycle, changes: &[(u8, bool)], _enabled: u64, _output: u64) {
        if !changes.is_empty() { self.gpio_changes(changes); }
    }
    /// A completed RMT transmission, decoded to bits by the peripheral model, with the pin the
    /// GPIO matrix has that channel routed to. Drivers that take a fresh channel per refresh
    /// (the Arduino NeoPixel one does) make the channel meaningless; the pin names the strip.
    fn rmt_frame(&mut self, _pin: u8, _bits: &[bool]) {}
    /// Bytes a GP-SPI master (`host` = 2 or 3) shifted out on MOSI.
    fn spi_tx(&mut self, _host: u8, _data: &[u8]) {}
    /// One complete GP-SPI transaction. The default preserves transmit-only boards and models an
    /// unattached MISO line.
    fn spi_transfer(&mut self, host: u8, tx: &[u8], rx_len: usize) -> Vec<u8> {
        self.spi_tx(host, tx);
        vec![0xff; rx_len]
    }
    /// Opt in to physical SPI routes. Fixed boards avoid route decoding.
    fn uses_spi_pins(&self) -> bool { false }
    /// Pin-aware boards select their devices using these routes and return MISO bytes.
    /// The default preserves the controller-based callback.
    fn spi_transfer_pins(&mut self, host: u8, _pins: SpiPins, tx: &[u8], rx_len: usize) -> Vec<u8> {
        self.spi_transfer(host, tx, rx_len)
    }
    /// Opt in to UART route decoding and receive polling. Cached by `attach_board_devices`.
    fn uses_uart_pins(&self) -> bool { false }
    /// A completed UART byte, with the routing and baud at the FIFO write. Console output is
    /// independent. Inverted signals and bit-level serial timing are not modelled here.
    fn uart_tx(&mut self, _cycle: VirtualCycle, _route: crate::uart::UartRoute, _byte: u8) {}
    /// Completed device-to-chip bytes to deliver at the next device tick. Pin numbers name
    /// chip GPIOs. The callback receives the current bus cycle for pacing. Unrouted input is dropped; a baud mismatch raises UART FRM_ERR.
    fn uart_rx(&mut self, _cycle: VirtualCycle) -> Vec<crate::uart::UartInput> { Vec::new() }
    fn gpio_events(&self) -> u64 { 0 }
    /// Devices on the I2C buses: (bus, 7-bit address, device).
    fn i2c_devices(&mut self) -> Vec<(u8, u8, Box<dyn I2cDevice>)> { Vec::new() }
    /// Give the board's camera a picture to look at (RGB888).
    fn set_camera_picture(&mut self, _p: crate::picture::Picture) -> Result<(), &'static str> { Err("no camera on this board") }
    /// Next camera frame in the sensor's register-selected wire format, with its size.
    /// Boards requiring an external clock use `camera_frame_with_clock` instead.
    fn camera_frame(&mut self) -> Option<(u32, u32, std::sync::Arc<Vec<u8>>)> { None }

    /// Small RGB preview of what the camera is looking at (for the UI), if a picture is loaded.
    fn camera_preview(&self, _w: u32, _h: u32) -> Option<Vec<u8>> { None }
    /// A complete frame from the LCD_CAM RGB interface (RGB565 little-endian, `w`x`h`).
    fn lcd_frame(&mut self, _w: u32, _h: u32, _rgb565: &[u8]) {}
    /// The board's display for the UI/PNG: (width, height, RGB565 pixels, change counter).
    fn display(&self) -> Option<(u32, u32, Vec<u16>, u64)> { None }
    /// Panels besides `display()`: a bare board can carry several at once and the page lays them
    /// out side by side. Preset boards, whose peripherals are fixed, keep the default of none.
    fn extra_displays(&self) -> usize { 0 }
    /// The `i`-th extra panel (0 = the first one after `display()`), skipped when its change
    /// counter already equals `since` so an idle panel costs nothing but the counter read.
    fn extra_display(&self, _i: usize, _since: u64) -> Option<(u32, u32, Vec<u16>, u64)> { None }
    /// Caption for display slot `i` (0 = `display()`, then the extra panels in order).
    fn display_label(&self, _i: usize) -> &'static str { self.name() }
    /// Completed display frames (for the UI's statistics line).
    fn display_frames(&self) -> u64 { 0 }
    /// Cheap change counter of the display (`display().3` without building the frame).
    fn display_version(&self) -> u64 { 0 }
    /// Prefer waiting one push interval for a quiet pixel stream. The UI still publishes on
    /// the next opportunity during continuous changes so animation cannot starve.
    fn display_quiet_push(&self) -> bool { false }
    /// Page pushes (display, audio, input poll) per emulated second. A panel that is drawn on
    /// interactively wants more than the default; one that rescans a large frame does not.
    fn display_push_hz(&self) -> u64 { 50 }
    /// Opt in to interactive host publication before execution. Returns false if unsupported.
    fn set_smooth_display(&mut self, _on: bool) -> bool { false }
    /// Select a scripted IMU motion (0 = still) before execution. Returns false if unsupported.
    fn set_imu_motion(&mut self, _mode: u32) -> bool { false }
    /// The scripted IMU motion selected by `set_imu_motion` (0 when unsupported).
    fn imu_motion(&self) -> u32 { 0 }
    /// Raw display memory for a debug PNG: (pixels, columns, rows).
    fn gram(&self) -> Option<(Vec<u16>, usize, usize)> { None }
    /// LED ring / strip: colours and a change counter.
    fn leds(&self) -> Option<(&[[u8; 3]], u64)> { None }
    /// Addressable LED modules besides `leds()`, each with the port it sits in and a change
    /// counter: (id, colours, updates). The UI draws one square grid per entry.
    fn led_grids(&self) -> Vec<(&'static str, &[[u8; 3]], u64)> { Vec::new() }
    /// Touch input from the UI (panel coordinates).
    fn touch(&mut self, _x: u16, _y: u16, _down: bool) {}
    /// Touch input observed at a specific bus horizon. Untimed boards use the ordinary input path.
    fn touch_at(&mut self, _cycle: VirtualCycle, x: u16, y: u16, down: bool) { self.touch(x, y, down); }
    /// The (pin, level) a touch controller's interrupt line takes after the last `touch` call, for
    /// boards whose controller pulls an input low while touched (CYD: XPT2046 T_IRQ on GPIO 36).
    fn touch_irq(&self) -> Option<(u8, bool)> { None }
    /// Current board-driven GPIO input levels, used to reconnect a persistent board after reset.
    fn input_levels(&self) -> Vec<(u8, bool)> { Vec::new() }
    /// GPIO pins the board pulls low (bit n = GPIO n), so an undriven pad reads low there rather
    /// than high: a motor or LED driver whose gate has a pull-down. Chips that resolve undriven
    /// pads for the board (the S31) use it.
    fn pull_downs(&self) -> u64 { 0 }
    /// Whether GPIO edge polling is needed. Cached by `attach_board_devices`.
    fn uses_gpio_edges(&self) -> bool { true }
    /// Use instruction-sized, unmodeled quanta for GPIO pulse decoding, overriding timing models.
    fn uses_gpio_waveform(&self) -> bool { false }
    /// Earliest autonomous transition strictly after the board's current cycle.
    fn next_deadline(&self) -> Option<VirtualCycle> { None }
    /// Advance monotonically through every board transition due by `cycle`.
    fn advance_to(&mut self, _cycle: VirtualCycle) {}
    /// Host GPIO drives to release after advancing the board.
    fn released_inputs(&mut self) -> Vec<u8> { Vec::new() }
    /// GPIO edges due by the last advance, including synchronous output responses.
    /// GPIO reads drain these even without elapsed cycles; retain future edges until due.
    fn take_edges(&mut self) -> Vec<BoardEdge> { Vec::new() }
    /// A pin by the name scripts and the UI use (`btn1`, `sw`, ...).
    fn named_pin(&self, _name: &str) -> Option<u8> { None }
    /// The rotary encoder's (CLK, DT) pins, if there is one.
    fn encoder(&self) -> Option<(u8, u8)> { None }
    /// Lines for the end-of-run report.
    fn report(&self) -> String { String::new() }
    /// Opt-in software drive after GPIO/IO_MUX writes and board attachment (including reset).
    /// Unlike raw latch changes, this observes route, mux and enable-only transitions.
    /// `uses_gpio_waveform` enables this callback as well as instruction-sized scheduling.
    /// Peripheral-generated waveforms remain on their existing callbacks.
    fn gpio_waveform_at(&mut self, _cycle: VirtualCycle, _routes: crate::pins::PinRoutes<'_>) {}
    /// Lazy physical clock query; called by the capture adapter, never polled per tick.
    /// The callback returns routed PWM frequency and duty (0..=65535) at a GPIO.
    /// Appended to preserve the existing board vtable's hot-call offsets.
    fn camera_frame_with_clock(&mut self, _clock: &dyn Fn(u8) -> Option<(f64, u32)>) -> Option<(u32, u32, std::sync::Arc<Vec<u8>>)> { self.camera_frame() }
    /// Advance scheduled inputs, querying a physical clock only on boards that need it.
    /// This replaces the existing advance call, not an additional producer poll.
    fn advance_inputs_with_clock(&mut self, cycle: VirtualCycle, _clock: &dyn Fn(u8) -> Option<(f64, u32)>) { self.advance_to(cycle); }
    /// Parallel camera wiring: D0..D7, PCLK, HREF, VSYNC. None on camera-free boards.
    fn camera_dvp_pins(&self) -> Option<[u8; 11]> { None }
    /// Most recent unclaimed scheduled camera falling edge, never a forced-low edge.
    /// Claiming prevents restart from replaying the same physical frame.
    fn camera_frame_start(&mut self, _claim: bool) -> Option<VirtualCycle> { None }
    /// Physical route changes invalidate the fresh-frame opportunity.
    fn invalidate_camera_frame_start(&mut self) {}
    /// Give the board's camera already-encoded baseline JPEG, delivered verbatim while the
    /// sensor outputs JPEG. Refusal (malformed, wrong size) keeps the previous input.
    fn set_camera_jpeg(&mut self, _jpeg: crate::picture::Jpeg) -> Result<(), crate::picture::JpegError> { Err(crate::picture::JpegError::Unsupported) }
    /// Whether `camera_frame_with_clock` currently returns one whole JPEG file (its length
    /// is the file's) rather than `w`x`h` raw pixels. Queried only by capture adapters.
    fn camera_frame_is_jpeg(&self) -> bool { false }
}

pub type Board = Box<dyn BoardModel>;

/// A bare module: nothing on the pins, console only.
pub struct NoBoard;
impl BoardModel for NoBoard {
    fn name(&self) -> &'static str { "none" }
    fn uses_gpio_edges(&self) -> bool { false }
}

/// One device the user wired onto a bare board. Each variant carries only what its bus really
/// needs: an I2C part takes a bus and a 7-bit address, an addressable LED strip a data pin and a
/// count, a bit-banged panel its SPI and D/C pins, a QSPI panel a host and its geometry. Addresses
/// belong to I2C alone — the other buses are not forced into that shape.
///
/// Parsed from `--device SPEC`, where the trailing field is always the model:
/// `BUS:ADDR:MODEL` (I2C, addr `0x76` or decimal `118`), `PIN:LEDS:ws2812`,
/// `SCLK:MOSI:CS:DC:st7735`, `HOST:WIDTH:HEIGHT:XOFF:co5300`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceSpec {
    I2c { bus: u8, addr: u8, model: String },
    Ws2812 { pin: u8, leds: usize },
    /// A 132×162 ST7735-family panel behind bit-banged SPI.
    St7735 { sclk: u8, mosi: u8, cs: u8, dc: u8 },
    /// A 240×320 ILI9341 panel behind bit-banged SPI (the common 2.4" module).
    Ili9341 { sclk: u8, mosi: u8, cs: u8, dc: u8 },
    /// A 128×64 monochrome NT7534 LCD behind bit-banged SPI; RS high selects command or data.
    Nt7534 { sclk: u8, mosi: u8, cs: u8, rs: u8 },
    Co5300 { host: u8, width: usize, height: usize, x_offset: u16 },
}

impl DeviceSpec {
    pub fn parse(s: &str) -> Result<Self, String> {
        let parts: Vec<&str> = s.split(':').map(str::trim).collect();
        let kind = parts.last().copied().unwrap_or("").to_ascii_lowercase();
        let fields = &parts[..parts.len().saturating_sub(1)];
        let numbers = |need: usize| -> Result<Vec<u32>, String> {
            if fields.len() != need {
                return Err(format!("--device: {kind} needs {need} fields before it, got {s:?}"));
            }
            fields.iter().map(|v| v.parse::<u32>()
                .map_err(|_| format!("--device: {v:?} is not a number in {s:?}"))).collect()
        };
        let byte = |v: u32, what: &str| -> Result<u8, String> {
            u8::try_from(v).map_err(|_| format!("--device: {what} {v} outside 0..255"))
        };
        match kind.as_str() {
            "bmp280" | "bmi270" | "bmm150" | "ft5x06" | "aht20" | "lsm6ds3" | "sh1106" => {
                if fields.len() != 2 {
                    return Err(format!("--device: I2C needs BUS:ADDR:{kind}, got {s:?}"));
                }
                let bus = fields[0].parse::<u32>()
                    .map_err(|_| format!("--device: bus {:?} is not a number in {s:?}", fields[0]))?;
                // The S3 has two I2C controllers; a third index would index past the bus table.
                if bus > 1 {
                    return Err(format!("--device: bus {bus} outside 0..1 in {s:?}"));
                }
                let a = fields[1];
                let addr = if let Some(hex) = a.strip_prefix("0x").or_else(|| a.strip_prefix("0X")) {
                    u8::from_str_radix(hex, 16).ok()
                } else {
                    a.parse::<u32>().ok().and_then(|d| u8::try_from(d).ok())
                }.ok_or_else(|| format!("--device: bad I2C address {a:?} (0x08..0x77)"))?;
                // 0x00-0x07 and 0x78-0x7f are the reserved 7-bit addresses; no part sits there.
                if !(0x08..=0x77).contains(&addr) {
                    return Err(format!("--device: I2C address {a:?} outside the usable 7-bit range 0x08..0x77"));
                }
                Ok(DeviceSpec::I2c { bus: byte(bus, "bus")?, addr, model: kind })
            }
            "ws2812" | "sk6812" | "neopixel" => {
                let n = numbers(2)?;
                if n[1] == 0 || n[1] > 2048 {
                    return Err(format!("--device: led count {} outside 1..2048", n[1]));
                }
                Ok(DeviceSpec::Ws2812 { pin: byte(n[0], "pin")?, leds: n[1] as usize })
            }
            "st7735" | "dcs-panel" | "lcd" | "ili9341" | "nt7534" | "st7565" => {
                let n = numbers(4)?;
                let (sclk, mosi, cs, fourth) =
                    (byte(n[0], "sclk")?, byte(n[1], "mosi")?, byte(n[2], "cs")?, byte(n[3], "dc/rs")?);
                Ok(match kind.as_str() {
                    "ili9341" => DeviceSpec::Ili9341 { sclk, mosi, cs, dc: fourth },
                    "nt7534" | "st7565" => DeviceSpec::Nt7534 { sclk, mosi, cs, rs: fourth },
                    _ => DeviceSpec::St7735 { sclk, mosi, cs, dc: fourth },
                })
            }
            "co5300" => {
                let n = numbers(4)?;
                if n[1] == 0 || n[1] > 1024 || n[2] == 0 || n[2] > 1024 {
                    return Err(format!("--device: panel {}x{} outside 1..1024", n[1], n[2]));
                }
                let x_offset = u16::try_from(n[3]).map_err(|_| format!("--device: x offset {} too large", n[3]))?;
                Ok(DeviceSpec::Co5300 { host: byte(n[0], "host")?, width: n[1] as usize, height: n[2] as usize, x_offset })
            }
            _ => Err(format!("--device: unknown model {kind:?} (bmp280, bmi270, bmm150, ft5x06, aht20, lsm6ds3, sh1106, ws2812, st7735, ili9341, nt7534, co5300)")),
        }
    }
}

/// Reject specs that would make the board ambiguous: two I2C parts on one address, or a second
/// panel of a kind the board has one slot for. Without this the later spec silently replaced the
/// earlier device or shared its framebuffer.
pub fn validate_specs(specs: &[DeviceSpec]) -> Result<(), String> {
    let mut addrs: Vec<(u8, u8)> = Vec::new();
    let mut panels: Vec<&'static str> = Vec::new();
    for spec in specs {
        let panel = match spec {
            DeviceSpec::St7735 { .. } | DeviceSpec::Ili9341 { .. } => Some("a DCS panel (st7735/ili9341)"),
            DeviceSpec::Nt7534 { .. } => Some("an nt7534 panel"),
            DeviceSpec::Co5300 { .. } => Some("a co5300 panel"),
            DeviceSpec::Ws2812 { .. } => None,
            DeviceSpec::I2c { bus, addr, model } => {
                if addrs.contains(&(*bus, *addr)) {
                    return Err(format!("--device: another device already answers on bus {bus} at address {addr:#04x}"));
                }
                addrs.push((*bus, *addr));
                if model == "sh1106" { Some("an sh1106 panel") } else { None }
            }
        };
        if let Some(label) = panel {
            if panels.contains(&label) {
                return Err(format!("--device: this board already carries {label}"));
            }
            panels.push(label);
        }
    }
    Ok(())
}

/// A user-defined bare board: only the SoC, with the devices listed in `specs` wired on their real
/// buses. Created by the CLI for `--board custom --device ...`. No preset peripherals are attached.
pub struct ConfigurableBoard {
    specs: Vec<DeviceSpec>,
    motion: std::sync::Arc<crate::devices::imu_motion::ImuMotion>,
    /// A DCS panel behind bit-banged SPI, with its D/C line kept out of the SPI model.
    tft: Option<crate::devices::DcsPanel>,
    /// What that panel is, for the page's caption: ST7735 and ILI9341 share the pixel path.
    tft_label: &'static str,
    bitbang: Option<crate::devices::SpiBitBang>,
    dc_pin: Option<u8>,
    /// A QSPI panel driven by a GP-SPI host.
    panel: Option<crate::devices::co5300::Co5300>,
    panel_host: Option<u8>,
    /// A 128×64 monochrome LCD on I2C; the RAM is shared so `display()` can publish it.
    sh1106: Option<MonoRam>,
    /// The same monochrome panel behind SPI (NT7534): its RAM, driver, bus and RS pin.
    nt7534: Option<MonoRam>,
    nt7534_driver: Option<crate::devices::Nt7534>,
    nt7534_spi: Option<crate::devices::SpiBitBang>,
    nt7534_rs: Option<u8>,
    /// Addressable LED strips, each on the data pin its RMT channel drives.
    strips: Vec<(u8, crate::devices::Ws2812Chain)>,
    gpio_events: u64,
    /// GP-SPI master transactions, recorded for `--buslog` (host, bytes). Capped.
    spi_tx_log: Vec<(u8, Vec<u8>)>,
    /// Ticks per second of a guest cycle, so the scripted motion is sampled in guest time.
    clock_hz: u64,
    /// Whether `--buslog` asked for the decoded bus activity. Off means nothing is recorded and
    /// nothing is printed, which is what keeps a plain run's memory and stderr flat.
    buslog: bool,
}

/// Monochrome panel RAM shared between a device (which writes it) and the board (which shows it).
pub type MonoRam = std::sync::Arc<std::sync::Mutex<crate::devices::MonoPanel>>;

/// A display a bare board can show, in the order the page lays the panels out.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PanelSlot { Co5300, Tft, Nt7534, Sh1106 }

fn mono_ram() -> MonoRam {
    std::sync::Arc::new(std::sync::Mutex::new(crate::devices::MonoPanel::new()))
}

fn mono_lock(ram: &MonoRam) -> std::sync::MutexGuard<'_, crate::devices::MonoPanel> {
    ram.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What a monochrome panel's glass shows, in the RGB565 shape `display()` promises.
fn mono_display(ram: &MonoRam) -> (u32, u32, Vec<u16>, u64) {
    let panel = mono_lock(ram);
    let (w, h, pixels) = panel.visible();
    (w, h, pixels, panel.pixels_written)
}

impl ConfigurableBoard {
    /// Keep the bit-banged panels' decoded transactions for `--buslog`. Off by default: a guest
    /// renders megabytes of pixels through these buses, and without the flag nothing reads them.
    pub fn record_buslog(&mut self) {
        self.buslog = true;
        if let Some(spi) = self.bitbang.as_mut() { spi.record(true); }
        if let Some(spi) = self.nt7534_spi.as_mut() { spi.record(true); }
    }

    /// `clock_hz` is the guest's CPU rate: the scripted motion is sampled at `cycle / clock_hz`
    /// seconds, so a sensor on this board moves on guest time like a preset board's does.
    pub fn new(specs: Vec<DeviceSpec>, clock_hz: u64) -> Self {
        let mut board = ConfigurableBoard {
            specs, motion: std::sync::Arc::new(crate::devices::imu_motion::ImuMotion::default()),
            tft: None, tft_label: "ST7735 (SPI)", bitbang: None, dc_pin: None, panel: None, panel_host: None,
            sh1106: None, nt7534: None, nt7534_driver: None, nt7534_spi: None, nt7534_rs: None,
            strips: Vec::new(), gpio_events: 0, spi_tx_log: Vec::new(), clock_hz, buslog: false,
        };
        for spec in board.specs.clone() {
            match spec {
                DeviceSpec::St7735 { sclk, mosi, cs, dc } => {
                    board.tft = Some(crate::devices::DcsPanel::st7735());
                    board.tft_label = "ST7735 160×80 (SPI)";
                    board.bitbang = Some(crate::devices::SpiBitBang::new(sclk, mosi, cs));
                    board.dc_pin = Some(dc);
                }
                DeviceSpec::Ili9341 { sclk, mosi, cs, dc } => {
                    // The 2.4" module's RAM is 240×320; the DCS pixel path is identical.
                    board.tft = Some(crate::devices::DcsPanel::new(240, 320));
                    board.tft_label = "ILI9341 240×320 (SPI)";
                    board.bitbang = Some(crate::devices::SpiBitBang::new(sclk, mosi, cs));
                    board.dc_pin = Some(dc);
                }
                DeviceSpec::Co5300 { host, width, height, x_offset } => {
                    board.panel = Some(crate::devices::co5300::Co5300::new(width, height, x_offset));
                    board.panel_host = Some(host);
                }
                DeviceSpec::Nt7534 { sclk, mosi, cs, rs } => {
                    let ram = mono_ram();
                    board.nt7534_driver = Some(crate::devices::Nt7534::new(ram.clone()));
                    board.nt7534 = Some(ram);
                    board.nt7534_spi = Some(crate::devices::SpiBitBang::new(sclk, mosi, cs));
                    board.nt7534_rs = Some(rs);
                }
                DeviceSpec::Ws2812 { pin, leds } => {
                    board.strips.push((pin, crate::devices::Ws2812Chain::new(leds)));
                }
                DeviceSpec::I2c { model, .. } => {
                    if model == "sh1106" {
                        board.sh1106 = Some(mono_ram());
                    }
                }
            }
        }
        board
    }
    pub fn specs(&self) -> &[DeviceSpec] { &self.specs }

    /// The panels wired on, in the order the page lays them out: slot 0 is whatever `display()`
    /// publishes, so a board with one panel keeps the layout it always had.
    fn panel_slots(&self) -> Vec<PanelSlot> {
        let mut slots = Vec::new();
        if self.panel.is_some() { slots.push(PanelSlot::Co5300); }
        if self.tft.is_some() { slots.push(PanelSlot::Tft); }
        if self.nt7534.is_some() { slots.push(PanelSlot::Nt7534); }
        if self.sh1106.is_some() { slots.push(PanelSlot::Sh1106); }
        slots
    }

    fn slot_label(&self, slot: PanelSlot) -> &'static str {
        match slot {
            PanelSlot::Co5300 => "CO5300 (QSPI)",
            PanelSlot::Tft => self.tft_label,
            PanelSlot::Nt7534 => "NT7534 3.5\" (SPI)",
            PanelSlot::Sh1106 => "SH1106 1.3\" (I2C)",
        }
    }

    /// One slot's frame. `full` false gives the geometry and change counter only, so a poll for
    /// "did this panel move" does not copy a framebuffer.
    fn slot_frame(&self, slot: PanelSlot, full: bool) -> Option<(u32, u32, Vec<u16>, u64)> {
        match slot {
            PanelSlot::Co5300 => self.panel.as_ref().map(|p| {
                (p.width() as u32, p.height() as u32, if full { p.frame.clone() } else { Vec::new() }, p.frames)
            }),
            PanelSlot::Tft => self.tft.as_ref().map(|t| {
                (t.cols as u32, t.rows as u32, if full { t.gram.clone() } else { Vec::new() }, t.pixels_written)
            }),
            PanelSlot::Nt7534 | PanelSlot::Sh1106 => {
                let ram = if slot == PanelSlot::Nt7534 { self.nt7534.as_ref() } else { self.sh1106.as_ref() }?;
                let panel = mono_lock(ram);
                let (w, h) = panel.size();
                let pixels = if full { panel.visible().2 } else { Vec::new() };
                Some((w, h, pixels, panel.pixels_written))
            }
        }
    }
}

impl BoardModel for ConfigurableBoard {
    fn name(&self) -> &'static str { "custom" }
    /// `--imu-motion N`: 0 holds every sensor still, 1 replays the handling script. A bare board is
    /// always wired with sensors, so the selector is accepted and shared with them.
    fn set_imu_motion(&mut self, mode: u32) -> bool {
        if mode > 1 { return false; }
        self.motion.mode.store(mode, std::sync::atomic::Ordering::Relaxed);
        true
    }
    fn imu_motion(&self) -> u32 { self.motion.mode.load(std::sync::atomic::Ordering::Relaxed) }
    /// Only the bit-banged panels read pin levels; an I2C-only board needs no edge polling.
    fn uses_gpio_edges(&self) -> bool { self.bitbang.is_some() || self.nt7534_spi.is_some() }

    fn advance_to(&mut self, cycle: VirtualCycle) {
        // The scripted motion is sampled in guest time. Storing the clock here is what makes an
        // I2C sensor on a bare board move like the same part on a preset board; without it the
        // clock stayed at 0 and mode 1 never left its first frame.
        self.motion.cycle.store(cycle, std::sync::atomic::Ordering::Relaxed);
    }

    fn gpio_changes(&mut self, changes: &[(u8, bool)]) {
        self.gpio_events += changes.len() as u64;
        for &(pin, level) in changes {
            // The DCS panel (ST7735/ILI9341) keeps its D/C line out of the SPI model.
            if let (Some(dc), Some(tft), Some(spi)) = (self.dc_pin, self.tft.as_mut(), self.bitbang.as_mut()) {
                if pin == dc { tft.dc = level; } else if let Some(b) = spi.pin(pin, level) { tft.byte(b); }
            }
            // The monochrome panel (NT7534) does the same with RS.
            if let (Some(rs), Some(drv), Some(spi)) = (self.nt7534_rs, self.nt7534_driver.as_mut(), self.nt7534_spi.as_mut()) {
                if pin == rs { drv.set_rs(level); } else if let Some(b) = spi.pin(pin, level) { drv.byte(b); }
            }
        }
    }

    fn rmt_frame(&mut self, pin: u8, bits: &[bool]) {
        for (strip_pin, chain) in &mut self.strips {
            if *strip_pin == pin { chain.from_bits(bits); }
        }
    }

    fn spi_tx(&mut self, host: u8, data: &[u8]) {
        // Record for `--buslog`: AI reads these transactions to verify a display's command
        // sequence without a waveform viewer. Cap both count and bytes per transaction, and keep
        // nothing at all unless the flag asked for it.
        if self.buslog && self.spi_tx_log.len() < 256 {
            let take = data.len().min(64);
            self.spi_tx_log.push((host, data[..take].to_vec()));
        }
        if self.panel_host == Some(host) {
            if let Some(panel) = self.panel.as_mut() { panel.transaction(data); }
        }
    }
    fn spi_transfer(&mut self, host: u8, tx: &[u8], rx_len: usize) -> Vec<u8> {
        self.spi_tx(host, tx);
        vec![0xff; rx_len]
    }

    fn i2c_devices(&mut self) -> Vec<(u8, u8, Box<dyn I2cDevice>)> {
        use crate::devices;
        let pts: devices::Ft5x06Points = std::sync::Arc::new(std::sync::Mutex::new([None; devices::ft5x06::FT5X06_POINTS]));
        self.specs.iter().filter_map(|s| match s {
            DeviceSpec::I2c { bus, addr, model } => {
                let dev: Box<dyn I2cDevice> = match model.as_str() {
                    "bmp280" => Box::new(devices::Bmp280::new()),
                    "bmi270" => Box::new(devices::Bmi270::new(self.motion.clone(), self.clock_hz)),
                    "bmm150" => Box::new(devices::Bmm150::with_motion(devices::Bmm150Trim::default(), self.motion.clone(), self.clock_hz)),
                    "ft5x06" => Box::new(devices::Ft5x06::new(pts.clone())),
                    "aht20" => Box::new(devices::Aht20::with_motion(self.motion.clone(), self.clock_hz)),
                    "lsm6ds3" => Box::new(devices::Lsm6ds3::with_motion(self.motion.clone(), self.clock_hz)),
                    "sh1106" => {
                        let ram = self.sh1106.clone().unwrap_or_else(mono_ram);
                        Box::new(devices::Sh1106::new(ram))
                    }
                    other => unreachable!("model {other} validated in DeviceSpec::parse"),
                };
                Some((*bus, *addr, dev))
            }
            _ => None,
        }).collect()
    }

    fn display(&self) -> Option<(u32, u32, Vec<u16>, u64)> {
        if let Some(panel) = &self.panel {
            Some((panel.width() as u32, panel.height() as u32, panel.frame.clone(), panel.frames))
        } else if let Some(tft) = &self.tft {
            Some((tft.cols as u32, tft.rows as u32, tft.gram.clone(), tft.pixels_written))
        } else if let Some(ram) = &self.nt7534 {
            Some(mono_display(ram))
        } else {
            self.sh1106.as_ref().map(mono_display)
        }
    }
    fn display_version(&self) -> u64 {
        if let Some(panel) = &self.panel {
            panel.frames
        } else if let Some(tft) = &self.tft {
            tft.pixels_written
        } else if let Some(ram) = self.nt7534.as_ref().or(self.sh1106.as_ref()) {
            mono_lock(ram).pixels_written
        } else {
            0
        }
    }
    fn display_label(&self, i: usize) -> &'static str {
        match self.panel_slots().get(i) {
            Some(slot) => self.slot_label(*slot),
            None => "custom",
        }
    }
    fn extra_displays(&self) -> usize { self.panel_slots().len().saturating_sub(1) }
    fn extra_display(&self, i: usize, since: u64) -> Option<(u32, u32, Vec<u16>, u64)> {
        let slot = *self.panel_slots().get(i + 1)?;
        let (_, _, _, version) = self.slot_frame(slot, false)?;
        if version == since { return None; }
        self.slot_frame(slot, true)
    }
    fn display_frames(&self) -> u64 {
        if let Some(panel) = &self.panel {
            panel.frames
        } else if let Some(tft) = &self.tft {
            tft.frames
        } else if let Some(ram) = self.nt7534.as_ref().or(self.sh1106.as_ref()) {
            // A whole 128x64 frame is 1024 bytes; pixels_written counts bits, eight per byte.
            mono_lock(ram).pixels_written / 8192
        } else {
            0
        }
    }
    fn leds(&self) -> Option<(&[[u8; 3]], u64)> {
        self.strips.first().map(|(_, chain)| (&chain.leds[..], chain.updates))
    }
    fn gpio_events(&self) -> u64 { self.gpio_events }
    fn report(&self) -> String {
        let mut out = String::new();
        // Hex of at most `take` bytes, with the real length when the frame is longer. Building it
        // from every byte is what put a 460 KB line in the report for one full-screen ILI9341 write.
        let hex = |bytes: &[u8], take: usize| -> String {
            let shown = &bytes[..bytes.len().min(take)];
            let mut s = shown.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
            if bytes.len() > take { s.push_str(&format!(" … ({} bytes)", bytes.len())); }
            s
        };
        // Decoded bit-bang SPI transactions — the readable form of the GPIO edges a
        // software-driven panel (ST7735/ILI9341/NT7534) puts on the wire. Neither section is
        // printed unless `--buslog` asked: a plain run must not grow a report per frame.
        if self.buslog && !self.spi_tx_log.is_empty() {
            out.push_str(&format!("[buslog] gpspi: {} master transactions (host, first 64 bytes)\n", self.spi_tx_log.len()));
            for (i, (host, bytes)) in self.spi_tx_log.iter().take(24).enumerate() {
                out.push_str(&format!("[buslog]   #{:<3} host{} {}\n", i + 1, host, hex(bytes, 64)));
            }
            if self.spi_tx_log.len() > 24 { out.push_str(&format!("[buslog]   … {} more\n", self.spi_tx_log.len() - 24)); }
        }
        if self.buslog {
            for (label, spi) in [("dcs", self.bitbang.as_ref()), ("nt7534", self.nt7534_spi.as_ref())] {
                let Some(spi) = spi else { continue };
                let frames = spi.frames();
                if frames.is_empty() { continue; }
                out.push_str(&format!("[buslog] {label}: {} CS transactions (MSB-first)\n", frames.len()));
                for (i, bytes) in frames.iter().take(24).enumerate() {
                    out.push_str(&format!("[buslog]   #{:<3} {}\n", i + 1, hex(bytes, 16)));
                }
                if frames.len() > 24 { out.push_str(&format!("[buslog]   … {} more transactions\n", frames.len() - 24)); }
            }
        }
        if let Some(tft) = &self.tft {
            out.push_str(&format!("[emu] tft: {} RAMWR, {} pixels, madctl={:#x}\n", tft.frames, tft.pixels_written, tft.madctl));
        }
        if let Some(panel) = &self.panel {
            out.push_str(&format!("[emu] panel: {} frames, {}x{}\n", panel.frames, panel.width(), panel.height()));
        }
        if let Some(ram) = &self.nt7534 {
            let p = mono_lock(ram);
            out.push_str(&format!("[emu] mono nt7534: {} bytes written, on={}, contrast={:#x}\n", p.pixels_written / 8, p.is_on(), p.contrast));
        }
        if let Some(ram) = &self.sh1106 {
            let p = mono_lock(ram);
            out.push_str(&format!("[emu] mono sh1106: {} bytes written, on={}, contrast={:#x}\n", p.pixels_written / 8, p.is_on(), p.contrast));
        }
        for (pin, chain) in &self.strips {
            out.push_str(&format!("[emu] ws2812 on pin {pin}: {} updates, {} leds\n", chain.updates, chain.leds.len()));
        }
        out
    }
}

#[cfg(test)]
mod configurable_tests {
    use super::*;

    #[test]
    fn parses_each_bus_into_its_own_shape() {
        assert_eq!(DeviceSpec::parse("0:0x76:bmp280").unwrap(),
                   DeviceSpec::I2c { bus: 0, addr: 0x76, model: "bmp280".into() });
        assert_eq!(DeviceSpec::parse("1:118:BMP280").unwrap(),
                   DeviceSpec::I2c { bus: 1, addr: 0x76, model: "bmp280".into() });
        assert_eq!(DeviceSpec::parse("8:12:ws2812").unwrap(),
                   DeviceSpec::Ws2812 { pin: 8, leds: 12 });
        assert_eq!(DeviceSpec::parse("2:1:41:40:st7735").unwrap(),
                   DeviceSpec::St7735 { sclk: 2, mosi: 1, cs: 41, dc: 40 });
        assert_eq!(DeviceSpec::parse("2:368:448:16:co5300").unwrap(),
                   DeviceSpec::Co5300 { host: 2, width: 368, height: 448, x_offset: 16 });
        assert_eq!(DeviceSpec::parse("0:0x38:aht20").unwrap(),
                   DeviceSpec::I2c { bus: 0, addr: 0x38, model: "aht20".into() });
        assert_eq!(DeviceSpec::parse("1:0x6a:lsm6ds3").unwrap(),
                   DeviceSpec::I2c { bus: 1, addr: 0x6a, model: "lsm6ds3".into() });
        assert_eq!(DeviceSpec::parse("2:1:41:40:ili9341").unwrap(),
                   DeviceSpec::Ili9341 { sclk: 2, mosi: 1, cs: 41, dc: 40 });
        assert_eq!(DeviceSpec::parse("0:0x3c:sh1106").unwrap(),
                   DeviceSpec::I2c { bus: 0, addr: 0x3c, model: "sh1106".into() });
        assert_eq!(DeviceSpec::parse("2:1:41:40:nt7534").unwrap(),
                   DeviceSpec::Nt7534 { sclk: 2, mosi: 1, cs: 41, rs: 40 });
    }

    #[test]
    fn rejects_bad_device_specs() {
        assert!(DeviceSpec::parse("0:0x76").is_err());
        assert!(DeviceSpec::parse("0:0x99:bmp280").is_err());
        assert!(DeviceSpec::parse("0:0x76:nope").is_err());
        assert!(DeviceSpec::parse("8:ws2812").is_err(), "ws2812 needs a pin and a led count");
        assert!(DeviceSpec::parse("2:1:41:st7735").is_err(), "st7735 needs four pins");
        assert!(DeviceSpec::parse("2:0:448:16:co5300").is_err(), "a panel needs a non-zero width");
    }

    #[test]
    fn configurable_board_returns_every_i2c_device() {
        let specs = vec![
            DeviceSpec::parse("0:0x76:bmp280").unwrap(),
            DeviceSpec::parse("0:0x69:bmi270").unwrap(),
            DeviceSpec::parse("1:0x10:bmm150").unwrap(),
            DeviceSpec::parse("1:0x38:ft5x06").unwrap(),
            DeviceSpec::parse("0:0x38:aht20").unwrap(),
            DeviceSpec::parse("1:0x6a:lsm6ds3").unwrap(),
        ];
        let mut board = ConfigurableBoard::new(specs, 1_000_000_000);
        let devs = board.i2c_devices();
        assert_eq!(devs.len(), 6);
        assert_eq!((devs[0].0, devs[0].1), (0, 0x76));
        assert_eq!((devs[3].0, devs[3].1), (1, 0x38));
        assert_eq!((devs[4].0, devs[4].1), (0, 0x38));
        assert_eq!((devs[5].0, devs[5].1), (1, 0x6a));
    }

    #[test]
    fn spi_and_gpio_devices_never_take_an_i2c_address() {
        let specs = vec![
            DeviceSpec::parse("8:12:ws2812").unwrap(),
            DeviceSpec::parse("2:1:41:40:st7735").unwrap(),
            DeviceSpec::parse("2:368:448:16:co5300").unwrap(),
        ];
        let mut board = ConfigurableBoard::new(specs, 1_000_000_000);
        assert!(board.i2c_devices().is_empty(), "SPI/GPIO devices must not show up on I2C");
        assert!(board.uses_gpio_edges(), "the bit-banged panel needs pin edges");

        board.rmt_frame(8, &[true, false, true]);
        assert!(board.leds().is_some(), "the strip wired to pin 8 is exposed for the UI");

        board.spi_transfer(2, &[0x02, 0x00, 0x2c, 0x00], 0);
        assert_eq!(board.display_frames(), 1, "a CO5300 RAMWR on host 2 is one frame");
        assert_eq!(board.display().map(|d| (d.0, d.1)), Some((368, 448)));
    }

    #[test]
    fn bit_banged_panel_decodes_dc_and_clocked_bytes() {
        let specs = vec![DeviceSpec::parse("2:1:41:40:st7735").unwrap()];
        let mut board = ConfigurableBoard::new(specs, 1_000_000_000);
        // CS low, then clock 0x29 (display on) out MSB first with D/C low.
        board.gpio_changes(&[(41, false)]);
        for bit in 0..8 {
            let level = 0x29u8 & (0x80 >> bit) != 0;
            board.gpio_changes(&[(1, level), (2, true), (2, false)]);
        }
        assert!(board.display().is_some(), "the DCS panel decodes the byte stream");
    }

    #[test]
    fn ili9341_panel_is_the_240x320_module() {
        let specs = vec![DeviceSpec::parse("2:1:41:40:ili9341").unwrap()];
        let mut board = ConfigurableBoard::new(specs, 1_000_000_000);
        assert_eq!(board.display().map(|d| (d.0, d.1)), Some((240, 320)));
        assert_eq!(board.display_frames(), 0, "no RAMWR yet");
        // CS low, then a clocked 0x11 (sleep out) with D/C low: the panel leaves sleep.
        board.gpio_changes(&[(41, false)]);
        for bit in 0..8 {
            let level = 0x11u8 & (0x80 >> bit) != 0;
            board.gpio_changes(&[(1, level), (2, true), (2, false)]);
        }
        assert_eq!(board.display().unwrap().3, 0, "still no pixels written");
    }

    #[test]
    fn mono_panels_are_wired_on_i2c_and_spi() {
        let specs = vec![
            DeviceSpec::parse("0:0x3c:sh1106").unwrap(),
            DeviceSpec::parse("2:1:41:40:nt7534").unwrap(),
        ];
        let mut board = ConfigurableBoard::new(specs, 1_000_000_000);
        assert_eq!(board.i2c_devices().len(), 1, "the SH1106 is the only I2C device");
        assert_eq!(board.display().map(|d| (d.0, d.1)), Some((128, 64)));
        assert!(board.uses_gpio_edges(), "the NT7534 needs pin edges");

        // CS low, RS low, then 0xAF (display on) clocked MSB first.
        board.gpio_changes(&[(41, false), (40, false)]);
        for bit in 0..8 {
            let level = 0xAFu8 & (0x80 >> bit) != 0;
            board.gpio_changes(&[(1, level), (2, true), (2, false)]);
        }
        // RS high: eight data bytes with every pixel lit.
        board.gpio_changes(&[(40, true)]);
        for _ in 0..8 {
            for bit in 0..8 {
                let level = (0x80 >> bit) != 0;
                board.gpio_changes(&[(1, level), (2, true), (2, false)]);
            }
        }
        let d = board.display().unwrap();
        assert!(d.2.iter().any(|&p| p != 0), "the glass must show the written pixels");
    }

    #[test]
    fn every_panel_gets_its_own_display_slot() {
        let specs = vec![
            DeviceSpec::parse("2:1:41:40:ili9341").unwrap(),
            DeviceSpec::parse("0:0x3c:sh1106").unwrap(),
        ];
        let board = ConfigurableBoard::new(specs, 1_000_000_000);
        assert_eq!(board.extra_displays(), 1, "one panel beyond what display() publishes");
        assert_eq!(board.display_label(0), "ILI9341 240×320 (SPI)");
        assert_eq!(board.display_label(1), "SH1106 1.3\" (I2C)");
        assert_eq!(board.display_label(2), "custom", "past the last panel");
    }

    #[test]
    fn an_idle_extra_panel_copies_no_pixels() {
        let specs = vec![
            DeviceSpec::parse("2:1:41:40:ili9341").unwrap(),
            DeviceSpec::parse("0:0x3c:sh1106").unwrap(),
        ];
        let mut board = ConfigurableBoard::new(specs, 1_000_000_000);
        let (w, h, pixels, version) = board.extra_display(0, u64::MAX).expect("the OLED is slot 1");
        assert_eq!((w, h), (128, 64));
        assert_eq!(pixels.len(), 128 * 64);
        assert!(board.extra_display(0, version).is_none(), "unchanged since the page last saw it");

        // Write one page of data through the SH1106 and the slot reports a new frame.
        {
            let classes = board.i2c_devices();
            let mut oled = classes.into_iter().find(|(_, addr, _)| *addr == 0x3c).expect("SH1106").2;
            oled.start(false);
            for byte in [0x00u8, 0xB0, 0x00, 0x10] { oled.write(byte); }
            oled.stop();
            oled.start(false);
            oled.write(0x40);
            for _ in 0..8 { oled.write(0xFF); }
            oled.stop();
        }
        let moved = board.extra_display(0, version).expect("the panel moved");
        assert_ne!(moved.3, version);
    }

    /// Two I2C controllers exist on the S3, and 0x00-0x07 / 0x78-0x7f are the reserved 7-bit
    /// addresses: a spec naming either used to index past the bus table or claim a part that
    /// cannot exist, and the message quoted a range the check did not enforce.
    #[test]
    fn rejects_out_of_range_buses_and_reserved_addresses() {
        assert!(DeviceSpec::parse("2:0x76:bmp280").is_err(), "bus 2 would index past the bus table");
        assert!(DeviceSpec::parse("0:0x00:bmp280").is_err(), "0x00 is reserved");
        assert!(DeviceSpec::parse("0:0x78:aht20").is_err(), "0x78 is reserved");
        assert!(DeviceSpec::parse("0:0x08:aht20").is_ok(), "0x08 is the first usable address");
        assert!(DeviceSpec::parse("0:0x77:bmp280").is_ok(), "0x77 is the last usable address");
    }

    /// Silently replacing the earlier device hid a wiring mistake: the board has one slot per
    /// (bus, address) and one framebuffer per panel kind.
    #[test]
    fn rejects_duplicate_addresses_and_second_panels() {
        let dup_addr = vec![
            DeviceSpec::parse("0:0x76:bmp280").unwrap(),
            DeviceSpec::parse("0:0x76:aht20").unwrap(),
        ];
        let e = validate_specs(&dup_addr).unwrap_err();
        assert!(e.contains("bus 0") && e.contains("0x76"), "{e}");

        let dup_dcs = vec![
            DeviceSpec::parse("2:1:41:40:st7735").unwrap(),
            DeviceSpec::parse("2:1:41:40:ili9341").unwrap(),
        ];
        assert!(validate_specs(&dup_dcs).is_err(), "st7735 and ili9341 share one DCS slot");

        let dup_oled = vec![
            DeviceSpec::parse("0:0x3c:sh1106").unwrap(),
            DeviceSpec::parse("1:0x3d:sh1106").unwrap(),
        ];
        assert!(validate_specs(&dup_oled).is_err(), "both would share one mono RAM");
    }

    /// Clock 40 bytes into the bit-banged panel, one CS pulse.
    fn clock_40_bytes(board: &mut ConfigurableBoard) {
        board.gpio_changes(&[(41, false)]);
        for byte in 0..40u8 {
            for bit in 0..8 {
                let level = byte & (0x80 >> bit) != 0;
                board.gpio_changes(&[(1, level), (2, true), (2, false)]);
            }
        }
    }

    /// `--buslog` is what turns the decoded transactions on: without it nothing is recorded, and
    /// with it the hex stops at 16 bytes instead of printing a whole frame (one full-screen
    /// ILI9341 write used to put 460 KB on a single line).
    #[test]
    fn the_bus_log_is_off_by_default_and_truncates_when_on() {
        let specs = vec![DeviceSpec::parse("2:1:41:40:st7735").unwrap()];
        let mut board = ConfigurableBoard::new(specs, 1_000_000_000);
        clock_40_bytes(&mut board);
        let quiet = board.report();
        assert!(!quiet.contains("[buslog]"), "a plain run prints no bus log:\n{quiet}");

        let specs = vec![DeviceSpec::parse("2:1:41:40:st7735").unwrap()];
        let mut board = ConfigurableBoard::new(specs, 1_000_000_000);
        board.record_buslog();
        clock_40_bytes(&mut board);
        let loud = board.report();
        assert!(loud.contains("[buslog] dcs: 1 CS transactions"), "{loud}");
        assert!(loud.contains("(40 bytes)"), "the suffix must say what was left out:\n{loud}");
        let line = loud.lines().find(|l| l.contains("#1")).expect("transaction line");
        assert!(line.len() < 40 * 3, "the hex must be truncated, not printed whole: {line}");
    }

    /// The board has to store the guest clock: without it `--imu-motion 1` never left its first
    /// frame on a bare board, because the sensor samples `cycle / clock_hz`.
    #[test]
    fn the_guest_clock_reaches_the_sensors() {
        let specs = vec![DeviceSpec::parse("0:0x6a:lsm6ds3").unwrap()];
        let mut board = ConfigurableBoard::new(specs, 1_000_000_000);
        assert!(board.set_imu_motion(1), "a custom board accepts the motion selector");
        let mut imu = board.i2c_devices().pop().expect("the LSM6DS3").2;
        let read_xl = |dev: &mut Box<dyn I2cDevice>| -> [u8; 6] {
            dev.start(false); dev.write(0x28); dev.start(true);
            let mut out = [0u8; 6];
            for b in out.iter_mut() { *b = dev.read(); }
            out
        };
        board.advance_to(4_000_000_000);
        let early = read_xl(&mut imu);
        board.advance_to(18_000_000_000);
        let late = read_xl(&mut imu);
        assert_ne!(early, late, "the script must advance with guest time: {early:?} vs {late:?}");
    }

    /// `display_frames` divides bits by eight and bytes by 1024; a whole 128x64 frame is one frame,
    /// not 128. Nothing caught the divisor before.
    #[test]
    fn a_whole_mono_frame_counts_as_one_frame() {
        let specs = vec![DeviceSpec::parse("0:0x3c:sh1106").unwrap()];
        let mut board = ConfigurableBoard::new(specs, 1_000_000_000);
        {
            let mut oled = board.i2c_devices().pop().expect("the SH1106").2;
            for page in 0..8u8 {
                oled.start(false);
                for b in [0x00u8, 0xB0 | page, 0x00, 0x10] { oled.write(b); }
                oled.stop();
                oled.start(false);
                oled.write(0x40);
                for _ in 0..128 { oled.write(0xFF); }
                oled.stop();
            }
        }
        assert_eq!(board.display_frames(), 1, "eight pages of 128 bytes is one 128x64 frame");
    }
}
