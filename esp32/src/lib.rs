//! Classic ESP32 (D0WD ECO3): two LX6 cores, mask ROM, cache windows and the boot peripherals.
pub mod adc;
pub mod bus;
pub mod crypto;
pub mod i2c;
pub mod ledc;
pub mod periph;
pub mod rmt;
pub mod soc;
pub mod spi;
pub mod timers;
pub mod wifi;

pub use esp_soc::Stop;
pub use soc::{machine, Esp32, Machine};
