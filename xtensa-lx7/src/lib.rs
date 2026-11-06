//! Xtensa LX7 core emulator (ESP32-S3 configuration: windowed ABI, loops,
//! booleans, MUL/DIV32, single-precision FPU, MAC16, 32 interrupts / 6 levels).
pub mod bus { pub use emu_core::bus::*; }
pub mod core;
pub mod decode;
pub mod disasm;
pub mod exec;
pub mod lx106;
pub mod operands;
pub mod state;
pub mod pie;
pub mod pie_timing;
pub mod pie_table;

#[cfg(feature = "wasm-jit-profile")]
pub mod census;
pub mod block;
pub mod jit;
pub use emu_core::{Bus, Core, Fault, FlatRam};
pub use decode::{decode, Insn, Op};
pub use exec::{step, Trap};
pub use state::Cpu;
pub use lx106::Lx106;
