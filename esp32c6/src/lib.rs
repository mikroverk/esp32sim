//! ESP32-C6: the third chip in this workspace — one RV32IMAC core at 160 MHz, 512 KB of unified
//! HP SRAM, 16 KB of LP SRAM, a single 16 MB flash cache window shared by code and data.
//!
//! The digital peripherals are mostly the same IP as the C3's (UART, USB-Serial/JTAG, systimer,
//! timer groups, GPIO, SPI flash controller, SHA/AES/RSA), so their models come from
//! `esp-periph`. What is new is the system side: PCR (peripheral clocks and resets) and PMU
//! replace SYSTEM and RTC_CNTL, the always-on LP blocks (LP_CLKRST, LP_AON, LP_TIMER, LP_WDT)
//! hold what used to be in RTC_CNTL, a PLIC front-end sits on the interrupt matrix, and the L1
//! cache has its own register block.
pub mod board;
pub mod bus;
pub mod net;
pub mod periph;
pub mod radio;
pub mod soc;
pub mod wifi;

pub use esp_soc::Stop;
pub use soc::{machine, Machine, C6};

mod ble;

mod ecc;
