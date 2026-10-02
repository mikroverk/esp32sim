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
    /// S3 calls this immediately after a GPIO output/enable write; other chips may use
    /// `gpio_changes` until they implement timestamped output.
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
    fn gpio_events(&self) -> u64 { 0 }
    /// Devices on the I2C buses: (bus, 7-bit address, device).
    fn i2c_devices(&mut self) -> Vec<(u8, u8, Box<dyn I2cDevice>)> { Vec::new() }
    /// Give the board's camera a picture to look at (RGB888).
    fn set_camera_picture(&mut self, _p: crate::picture::Picture) {}
    /// Next camera frame as the sensor would put it on the DVP bus (YUYV), with its size. None = no camera / nothing to show.
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
    /// Current board-driven GPIO input levels, used to reconnect a persistent board after reset.
    fn input_levels(&self) -> Vec<(u8, bool)> { Vec::new() }
    /// Earliest autonomous transition strictly after the board's current cycle.
    fn next_deadline(&self) -> Option<VirtualCycle> { None }
    /// Advance monotonically through every board transition due by `cycle`.
    fn advance_to(&mut self, _cycle: VirtualCycle) {}
    /// Timestamped GPIO input edges emitted by the last advance.
    fn take_edges(&mut self) -> Vec<BoardEdge> { Vec::new() }
    /// A pin by the name scripts and the UI use (`btn1`, `sw`, ...).
    fn named_pin(&self, _name: &str) -> Option<u8> { None }
    /// The rotary encoder's (CLK, DT) pins, if there is one.
    fn encoder(&self) -> Option<(u8, u8)> { None }
    /// Lines for the end-of-run report.
    fn report(&self) -> String { String::new() }
}

pub type Board = Box<dyn BoardModel>;

/// A bare module: nothing on the pins, console only.
pub struct NoBoard;
impl BoardModel for NoBoard { fn name(&self) -> &'static str { "none" } }
