//! The observers that ship with the emulator: the classic debugging aids (trace, breakpoints,
//! watchpoints, register trace, pc histogram, register access statistics) and the analyses that
//! run at full speed on the block path (block profile, coverage, interrupt latency, VCD).
pub mod block_profile;
pub mod breakpoints;
pub mod buslog;
pub mod coverage;
pub mod irq_latency;
pub mod mmio_heat;
pub mod pc_hist;
pub mod regtrace;
pub mod trace;
pub mod vcd;
pub mod watch;

pub use block_profile::BlockProfile;
pub use breakpoints::Breakpoints;
pub use buslog::BusLog;
pub use coverage::Coverage;
pub use irq_latency::IrqLatency;
pub use mmio_heat::MmioHeat;
pub use pc_hist::PcHist;
pub use regtrace::RegTrace;
pub use trace::Trace;
pub use vcd::Vcd;
pub use watch::Watch;
