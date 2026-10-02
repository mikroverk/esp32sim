//! The machine around a chip, written once: the `Soc` implementations (ESP32-S3, ESP32-C3,
//! ESP32-C6) plug their cores, memory map, peripherals and interrupt controller into
//! `Machine<S>`, which owns the scheduler, device time, console, action scripts, the web UI
//! protocol, real-time pacing, the image loaders and the board model. `devices` holds the
//! chip-neutral models a board is built from.
pub mod ble;
pub mod board;
mod console;
pub mod debug;
pub mod devices;
pub mod elf;
pub mod host;
pub mod image;
pub mod json;
pub mod load;
pub mod machine;
pub mod nat;
pub mod net;
pub mod observe;
pub mod observers;
pub mod picture;
pub mod png;
pub mod soc;
pub mod web;
pub mod wifi;

pub use board::{Board, BoardModel, NoBoard};
pub use debug::DebugFlags;
pub use load::LoadKind;
pub use machine::{Console, Debug, Machine, Realtime, Script, ScriptAction};
pub use observe::{Ctx, Observer, Wants};
pub use soc::{CoreState, RunUntil, Soc, SocBus, Stop};
