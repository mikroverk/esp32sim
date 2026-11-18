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
            "bmp280" | "bmi270" | "bmm150" | "ft5x06" | "aht20" | "lsm6ds3" => {
                if fields.len() != 2 {
                    return Err(format!("--device: I2C needs BUS:ADDR:{kind}, got {s:?}"));
                }
                let bus = fields[0].parse::<u32>()
                    .map_err(|_| format!("--device: bus {:?} is not a number in {s:?}", fields[0]))?;
                let a = fields[1];
                let addr = if let Some(hex) = a.strip_prefix("0x").or_else(|| a.strip_prefix("0X")) {
                    u8::from_str_radix(hex, 16).ok()
                } else {
                    a.parse::<u32>().ok().and_then(|d| u8::try_from(d).ok())
                }.ok_or_else(|| format!("--device: bad I2C address {a:?} (0x03..0x77)"))?;
                if addr == 0 || addr > 0x7f {
                    return Err(format!("--device: I2C address {a:?} outside 7-bit range 0x03..0x77"));
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
            "st7735" | "dcs-panel" | "lcd" | "ili9341" => {
                let n = numbers(4)?;
                let (sclk, mosi, cs, dc) =
                    (byte(n[0], "sclk")?, byte(n[1], "mosi")?, byte(n[2], "cs")?, byte(n[3], "dc")?);
                if kind == "ili9341" {
                    Ok(DeviceSpec::Ili9341 { sclk, mosi, cs, dc })
                } else {
                    Ok(DeviceSpec::St7735 { sclk, mosi, cs, dc })
                }
            }
            "co5300" => {
                let n = numbers(4)?;
                if n[1] == 0 || n[1] > 1024 || n[2] == 0 || n[2] > 1024 {
                    return Err(format!("--device: panel {}x{} outside 1..1024", n[1], n[2]));
                }
                let x_offset = u16::try_from(n[3]).map_err(|_| format!("--device: x offset {} too large", n[3]))?;
                Ok(DeviceSpec::Co5300 { host: byte(n[0], "host")?, width: n[1] as usize, height: n[2] as usize, x_offset })
            }
            _ => Err(format!("--device: unknown model {kind:?} (bmp280, bmi270, bmm150, ft5x06, aht20, lsm6ds3, ws2812, st7735, ili9341, co5300)")),
        }
    }
}

/// A user-defined bare board: only the SoC, with the devices listed in `specs` wired on their real
/// buses. Created by the CLI for `--board custom --device ...`. No preset peripherals are attached.
pub struct ConfigurableBoard {
    specs: Vec<DeviceSpec>,
    motion: std::sync::Arc<crate::devices::imu_motion::ImuMotion>,
    /// A DCS panel behind bit-banged SPI, with its D/C line kept out of the SPI model.
    tft: Option<crate::devices::DcsPanel>,
    bitbang: Option<crate::devices::SpiBitBang>,
    dc_pin: Option<u8>,
    /// A QSPI panel driven by a GP-SPI host.
    panel: Option<crate::devices::co5300::Co5300>,
    panel_host: Option<u8>,
    /// Addressable LED strips, each on the data pin its RMT channel drives.
    strips: Vec<(u8, crate::devices::Ws2812Chain)>,
    gpio_events: u64,
}

impl ConfigurableBoard {
    pub fn new(specs: Vec<DeviceSpec>) -> Self {
        let mut board = ConfigurableBoard {
            specs, motion: std::sync::Arc::new(crate::devices::imu_motion::ImuMotion::default()),
            tft: None, bitbang: None, dc_pin: None, panel: None, panel_host: None,
            strips: Vec::new(), gpio_events: 0,
        };
        for spec in board.specs.clone() {
            match spec {
                DeviceSpec::St7735 { sclk, mosi, cs, dc } => {
                    board.tft = Some(crate::devices::DcsPanel::st7735());
                    board.bitbang = Some(crate::devices::SpiBitBang::new(sclk, mosi, cs));
                    board.dc_pin = Some(dc);
                }
                DeviceSpec::Ili9341 { sclk, mosi, cs, dc } => {
                    // The 2.4" module's RAM is 240×320; the DCS pixel path is identical.
                    board.tft = Some(crate::devices::DcsPanel::new(240, 320));
                    board.bitbang = Some(crate::devices::SpiBitBang::new(sclk, mosi, cs));
                    board.dc_pin = Some(dc);
                }
                DeviceSpec::Co5300 { host, width, height, x_offset } => {
                    board.panel = Some(crate::devices::co5300::Co5300::new(width, height, x_offset));
                    board.panel_host = Some(host);
                }
                DeviceSpec::Ws2812 { pin, leds } => {
                    board.strips.push((pin, crate::devices::Ws2812Chain::new(leds)));
                }
                DeviceSpec::I2c { .. } => {}
            }
        }
        board
    }
    pub fn specs(&self) -> &[DeviceSpec] { &self.specs }
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
    /// Only the bit-banged panel reads pin levels; an I2C-only board needs no edge polling.
    fn uses_gpio_edges(&self) -> bool { self.bitbang.is_some() }

    fn gpio_changes(&mut self, changes: &[(u8, bool)]) {
        self.gpio_events += changes.len() as u64;
        let (Some(dc), Some(tft), Some(spi)) = (self.dc_pin, self.tft.as_mut(), self.bitbang.as_mut()) else { return };
        for &(pin, level) in changes {
            if pin == dc { tft.dc = level; } else if let Some(b) = spi.pin(pin, level) { tft.byte(b); }
        }
    }

    fn rmt_frame(&mut self, pin: u8, bits: &[bool]) {
        for (strip_pin, chain) in &mut self.strips {
            if *strip_pin == pin { chain.from_bits(bits); }
        }
    }

    fn spi_tx(&mut self, host: u8, data: &[u8]) {
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
                    "bmi270" => Box::new(devices::Bmi270::new(self.motion.clone(), 1_000_000)),
                    "bmm150" => Box::new(devices::Bmm150::new(devices::Bmm150Trim::default())),
                    "ft5x06" => Box::new(devices::Ft5x06::new(pts.clone())),
                    "aht20" => Box::new(devices::Aht20::with_motion(self.motion.clone())),
                    "lsm6ds3" => Box::new(devices::Lsm6ds3::with_motion(self.motion.clone())),
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
        } else {
            None
        }
    }
    fn display_version(&self) -> u64 {
        if let Some(panel) = &self.panel { panel.frames } else if let Some(tft) = &self.tft { tft.pixels_written } else { 0 }
    }
    fn display_frames(&self) -> u64 {
        if let Some(panel) = &self.panel { panel.frames } else if let Some(tft) = &self.tft { tft.frames } else { 0 }
    }
    fn leds(&self) -> Option<(&[[u8; 3]], u64)> {
        self.strips.first().map(|(_, chain)| (&chain.leds[..], chain.updates))
    }
    fn gpio_events(&self) -> u64 { self.gpio_events }
    fn report(&self) -> String {
        let mut out = String::new();
        if let Some(tft) = &self.tft {
            out.push_str(&format!("[emu] tft: {} RAMWR, {} pixels, madctl={:#x}\n", tft.frames, tft.pixels_written, tft.madctl));
        }
        if let Some(panel) = &self.panel {
            out.push_str(&format!("[emu] panel: {} frames, {}x{}\n", panel.frames, panel.width(), panel.height()));
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
        let mut board = ConfigurableBoard::new(specs);
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
        let mut board = ConfigurableBoard::new(specs);
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
        let mut board = ConfigurableBoard::new(specs);
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
        let mut board = ConfigurableBoard::new(specs);
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
}
