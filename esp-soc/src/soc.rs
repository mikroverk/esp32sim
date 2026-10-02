//! What a chip provides to `Machine`: its cores, and a bus that also answers the machine's
//! questions about console output, reset, boot, the board, audio and interrupt routing.
use crate::board::BoardModel;
use emu_core::{Bus, Core, LifecycleKind};
use esp_periph::Misc;

/// Why `Machine::run` returned.
#[derive(Clone, Debug)]
pub enum Stop {
    MaxInsns,
    Halted,
    Breakpoint(u32),
    Unimplemented(u32, u32),
    /// a chip reset was requested (software, watchdog): `reboot()` and run again
    SwReset,
    /// `simcall` — Xtensa semihosting
    Simcall(u32),
    /// `ebreak` with no handler installed — a panic or an assert in a RISC-V guest
    Ebreak(u32),
    /// `--watch`: a word changed value (addr, old, new)
    Watch(u32, u32, u32),
    Exceptions(u64),
    /// The model could not price an execution event. Its effects remain committed.
    CostModel { core: usize, pc: u32, reason: String },
    /// The model refused a reset event, which has no instruction pc.
    CostModelLifecycle { kind: LifecycleKind, reason: String },
}

/// Why `Machine::run_until_cycle` returned.
#[derive(Debug)]
pub enum RunUntil {
    /// device time is at (or, by the last instruction, just past) the target cycle
    Reached,
    /// the bus flagged a host event (`SocBus::take_host_event`): time stands at the instruction that caused it
    Yield,
    Stop(Stop),
}

/// A secondary core's state as its SoC registers say.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CoreState { Running, Held, Reset }

pub trait Soc: 'static {
    type Core: Core;
    type Bus: SocBus;
    const NAME: &'static str;
    /// The mask ROM ELF's file name in espressif/esp-rom-elfs.
    const ROM_ELF: &'static str;
    const CPU_HZ: u64;
    const CORES: usize;
    /// How far time jumps when every core sleeps (at most; deadlines and limits cut it).
    const IDLE_CHUNK: u64;
    /// Symbols, in order of preference, that start the ROM's RAM-initialiser table.
    const ROM_DATA_TABLE: &'static [&'static str];
    /// Symbols, in order of preference, that end it.
    const ROM_DATA_TABLE_END: &'static [&'static str] = &["_data_end"];
    /// Bytes per table entry: (dst_start, dst_end, rom_src[, 0]) — 16 on the S3 and C3, 12 on the C6.
    const ROM_DATA_TABLE_STRIDE: u32 = 16;
    fn new_core(i: usize) -> Self::Core;
    /// Connect core-local state to resources owned by this machine's bus.
    fn new_core_with_bus(i: usize, _bus: &Self::Bus) -> Self::Core { Self::new_core(i) }
    /// Bring core `i` back to its reset state (after a chip reset or a release from reset).
    fn reset_core(core: &mut Self::Core, i: usize);
    /// Set a core up to start the app image at `entry` as the 2nd-stage bootloader would have.
    fn boot_core(core: &mut Self::Core, entry: u32);
    /// The interrupt input of every core, from the bus's current source state.
    fn irqs(bus: &Self::Bus, out: &mut [<Self::Core as Core>::Irq]);
    fn core_state(_bus: &Self::Bus, _core: usize) -> CoreState { CoreState::Running }
}

pub trait SocBus: Bus {
    fn cycles(&self) -> u64;
    /// CPU cycles from the current device horizon to the next transition that may wake a core.
    fn next_deadline(&self) -> Option<u64> { None }
    fn irq_dirty(&mut self) -> &mut bool;
    /// Arm or disarm device-register deferral for a multi-quantum run (EX133); clears the flag.
    /// Whether `set_defer` is honoured; without it a multi-quantum run is never attempted.
    fn can_defer(&self) -> bool { false }
    fn set_defer(&mut self, on: bool) { let _ = on; }
    /// Whether the last dispatch stopped in front of a device-register access; clears it.
    fn take_deferred(&mut self) -> bool { false }
    /// Re-derive the interrupt lines after a device change; true if a core's input may differ.
    fn refresh_irq(&mut self) -> bool;
    /// Deliver deferred device time now (a bus that defers it).
    fn flush_ticks(&mut self) {}
    /// A device event the host must see at the instruction that caused it (a radio
    /// transmission starting): `Machine::run_until_cycle` stops its round there and returns
    /// `RunUntil::Yield`. Reading it clears it.
    fn take_host_event(&mut self) -> bool { false }
    fn misc(&mut self) -> &mut Misc;
    fn load_bytes(&mut self, addr: u32, data: &[u8]) -> Result<(), String>;
    fn write_flash(&mut self, offset: usize, data: &[u8]) -> Result<(), String>;
    /// Map and copy the app image at flash `app_off` as the bootloader would; returns the entry point.
    fn boot_app(&mut self, app_off: usize) -> Result<u32, String>;
    /// Chip reset: re-create the digital peripherals, keep what survives on silicon. Returns the cause.
    fn reboot(&mut self, mac: [u8; 6]) -> u32;
    fn sw_reset(&self) -> bool;
    /// The board's reset button: the chip resets with this cause at the next scheduling round,
    /// through the same path as a reset the firmware asked for.
    fn request_reset(&mut self, cause: u32);
    fn reset_cause(&self) -> u32;
    fn last_fault(&self) -> Option<(u32, bool)>;
    /// Console bytes since the last call: USB-Serial/JTAG, UART0, UART1, UART2.
    fn console_take(&mut self) -> [Vec<u8>; 4];
    /// Bytes from the host into the USB-Serial/JTAG console.
    fn serial_input(&mut self, data: &[u8]);
    /// Bytes from the host into UART `n`'s receive FIFO (a terminal on the chip's UART0 pins).
    fn uart_input(&mut self, n: usize, data: &[u8]);
    fn gpio_set_input(&mut self, pin: u8, level: bool);
    /// Drive an analog pad (script `adc` / `adcwave`); chips without an ADC model ignore it.
    fn analog_set(&mut self, _pin: u8, _src: esp_periph::AnalogSource) {}
    /// Touch or release a capacitive GPIO pad, separate from the board's touch panel.
    fn set_touch_input(&mut self, _pin: u8, _touched: bool) {}
    /// Deliver host touch at the bus's current time horizon.
    fn touch_input(&mut self, x: u16, y: u16, down: bool) { self.board().touch(x, y, down); }
    fn gpio_input(&self) -> u64;
    /// Averaged PWM at a GPIO-matrix output: frequency in Hz and high-time fraction scaled to 0..65535.
    fn pwm_output(&self, _pin: u32) -> Option<(f64, u32)> { None }
    /// Start/stop recording GPIO edges (outputs as they reach the board, inputs as they are set).
    fn observe_gpio(&mut self, on: bool);
    /// (cycle, pin, level) edges recorded since the last call.
    fn take_gpio_events(&mut self) -> Vec<(u64, u8, bool)>;
    fn board(&mut self) -> &mut dyn BoardModel;
    fn board_ref(&self) -> &dyn BoardModel;
    /// Captured audio so far (left channel) and its sample rate.
    fn audio(&self) -> (&[i16], u32);
    fn camera_frames(&self) -> u64 { 0 }
    /// Peripheral source numbers routed to CPU interrupt `line` of `core` (for the end-of-run report).
    fn irq_sources_of(&self, core: usize, line: u32) -> Vec<usize>;
    /// Apply the debug areas to the devices and to the bus's own logging.
    fn set_debug(&mut self, f: &crate::debug::DebugFlags);
    /// Resize the flash array (the JEDEC capacity follows).
    fn set_flash_size(&mut self, bytes: usize);
    /// Resize the PSRAM, on chips that have one.
    fn set_psram_size(&mut self, _bytes: usize) -> Result<(), String> { Err("this chip has no PSRAM".into()) }
    /// Strapping pins as the ROM reads them.
    fn set_strap(&mut self, v: u32);
    /// The reset cause the ROM will report (to reproduce a real board's boot).
    fn set_reset_cause(&mut self, cause: u32);
    /// Chip-specific end-of-run statistics (audio, WiFi, crypto, DMA engines).
    fn report(&self) -> String { String::new() }
}
