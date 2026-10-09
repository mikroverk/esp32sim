//! Classic ESP32 (D0WD ECO3): two LX6 cores, mask ROM, cache windows and the boot peripherals.
pub mod bus;
pub mod periph;
pub mod soc;
pub mod timers;

pub use esp_soc::Stop;
pub use soc::{machine, Esp32, Machine};
