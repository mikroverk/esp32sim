//! The machine around a chip, written once: the `Soc` implementations (ESP32-S3, ESP32-C3,
//! ESP32-C6) plug their cores, memory map, peripherals and interrupt controller into
//! `Machine<S>`, which owns the scheduler, device time, console, action scripts, the web UI
//! protocol, real-time pacing, the image loaders and the board model. `devices` holds the
//! chip-neutral models a board is built from.
pub mod board;
pub mod debug;
pub mod devices;
pub mod elf;
pub mod host;
pub mod image;
pub mod machine;
pub mod observe;
pub mod observers;
pub mod picture;
pub mod png;
pub mod soc;
pub mod storage;
#[cfg(not(target_arch = "wasm32"))]
pub mod uart_tcp;
pub mod web;

pub use board::{Board, BoardModel, NoBoard, PanelControl};
pub use debug::DebugFlags;
pub use machine::{Console, Debug, Machine, Realtime, Script, ScriptAction};
pub use observe::{Ctx, Observer, Wants};
pub use soc::{CoreState, RunUntil, Soc, SocBus, Stop};
pub use storage::StateFile;
#[cfg(not(target_arch = "wasm32"))]
pub use uart_tcp::UartTcp;
