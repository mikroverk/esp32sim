#[cfg(target_arch = "wasm32")]
use xtensa_lx7::{Bus, Core};
#[cfg(not(target_arch = "wasm32"))]
use emu_core::{Bus, Core};
use esp_soc::{board::BoardModel, devices::Ws2812Chain, Machine, Soc, SocBus};
use std::sync::{Arc, Mutex};

struct Capture {
    strip: Ws2812Chain,
    edges: Vec<(u64, bool)>,
}
struct Board {
    state: Arc<Mutex<Capture>>,
    pin: u8,
}
impl BoardModel for Board {
    fn name(&self) -> &'static str {
        "waveform-test"
    }
    fn uses_gpio_waveform(&self) -> bool { true }
    fn gpio_output_at(&mut self, cycle: u64, _: &[(u8, bool)], enabled: u64, output: u64) {
        let enabled = enabled & (1u64 << self.pin) != 0;
        let high = output & (1u64 << self.pin) != 0;
        let mut s = self.state.lock().unwrap();
        if enabled && s.edges.last().is_none_or(|&(_, last)| last != high) {
            s.edges.push((cycle, high));
        }
        s.strip.gpio_drive(cycle, enabled, high);
    }
    fn next_deadline(&self) -> Option<u64> {
        self.state.lock().unwrap().strip.gpio_deadline()
    }
    fn advance_to(&mut self, cycle: u64) {
        self.state
            .lock()
            .unwrap()
            .strip
            .advance_gpio(cycle);
    }
}
fn board(hz: u32, pin: u8) -> (Box<dyn BoardModel>, Arc<Mutex<Capture>>) {
    let state = Arc::new(Mutex::new(Capture {
        strip: Ws2812Chain::new(3).with_gpio_clock(hz),
        edges: Vec::new(),
    }));
    (
        Box::new(Board {
            state: state.clone(),
            pin,
        }),
        state,
    )
}

fn pulse_program(xtensa: bool, hz: u64) -> Vec<u8> {
    let mut code = Vec::new();
    for byte in [11u8, 7, 13, 19, 17, 23, 31, 29, 37] {
        for bit in (0..8).rev() {
            for (high, ns) in [
                (true, if byte & (1 << bit) != 0 { 800 } else { 400 }),
                (false, 800),
            ] {
                if xtensa {
                    code.extend([0x22, 0x61, if high { 0 } else { 1 }]);
                } else {
                    code.extend((0x0020_a023u32 | if high { 0 } else { 4 << 7 }).to_le_bytes());
                }
                for _ in 1..ns * hz / 1_000_000_000 {
                    if xtensa {
                        code.extend([0x3d, 0xf0]);
                    } else {
                        code.extend([1, 0]);
                    }
                }
            }
        }
    }
    if xtensa {
        code.extend([0x06, 0xff, 0xff]);
    } else {
        code.extend(0x0000_006fu32.to_le_bytes());
    }
    code
}
fn check<S: Soc>(
    mut m: Machine<S>,
    state: Arc<Mutex<Capture>>,
    code: Vec<u8>,
    routes: (u32, u32, u32),
    quantum: u64,
    core: usize,
) -> Machine<S> {
    let (gpio, mux, signal) = routes;
    m.quantum = quantum;
    m.console.capture = true;
    m.bus.write32(mux + 4 + 4 * 4, 1 << 12).unwrap();
    m.bus.write32(gpio + 0x554 + 4 * 4, signal).unwrap();
    m.bus.write32(gpio + 0x24, 1 << 4).unwrap();
    let pc = if gpio == 0x6009_1000 {
        0x4080_0000
    } else {
        0x4038_0000
    };
    m.bus.load_bytes(pc, &code).unwrap();
    m.cores[core].set_pc(pc);
    m.max_cycles = 100_000;
    assert_eq!(m.browser_external_block_budget(1024), None);
    if core == 0 && quantum == 1024 {
        m.run_until_cycle(100_000);
    } else {
        m.run(100_000);
    }
    assert_eq!(m.quantum, quantum, "board opt-in must preserve the configured quantum");
    let s = state.lock().unwrap();
    assert_eq!(
        s.strip.leds,
        [[7, 11, 13], [17, 19, 23], [29, 31, 37]],
        "q={quantum} edges={:?}",
        &s.edges[..s.edges.len().min(8)]
    );
    assert_eq!(s.edges.len(), 145);
    assert!(s.edges.windows(2).all(|e| e[0].0 <= e[1].0));
    let widths: Vec<_> = s.edges[1..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|e| e[1].0 - e[0].0)
        .collect();
    let expected: Vec<_> = [11u8, 7, 13, 19, 17, 23, 31, 29, 37].iter()
        .flat_map(|byte| (0..8).rev().map(move |bit| S::CPU_HZ * if byte & (1 << bit) == 0 { 400 } else { 800 } / 1_000_000_000))
        .collect();
    assert_eq!(widths, expected);
    for edges in s.edges[2..].windows(2).step_by(2) {
        assert_eq!(edges[1].0 - edges[0].0, S::CPU_HZ * 800 / 1_000_000_000);
    }
    m
}
fn timing_model_conflicts() {
    const ERROR: &str = "GPIO waveform decoding cannot be combined with a timing model";
    for approximate in [false, true] {
        let mut m = esp32s3::machine([0; 6]);
        let configure = |m: &mut Machine<esp32s3::S3>| {
            if approximate { m.set_approximate_jit_timing(2, 64) }
            else { m.set_cost_model(Box::new(esp32s3::ApproximateCostModel::default())) }
        };
        m.bus.board = board(240_000_000, 4).0;
        m.bus.attach_board_devices();
        assert_eq!(configure(&mut m).unwrap_err(), ERROR);
        assert!(!m.has_timing_model());
        m.bus.board = Box::new(esp_soc::NoBoard);
        m.bus.attach_board_devices();
        configure(&mut m).unwrap();
        assert!(m.has_timing_model());
        // Direct bus access can replace the board after timing configuration.
        m.bus.board = board(240_000_000, 4).0;
        m.bus.attach_board_devices();
        let quantum = m.quantum;
        assert!(matches!(m.run(1), esp_soc::Stop::CostModelLifecycle { reason, .. } if reason == ERROR));
        assert!(matches!(m.run_until_cycle(1), esp_soc::RunUntil::Stop(esp_soc::Stop::CostModelLifecycle { reason, .. }) if reason == ERROR));
        assert!(m.has_timing_model());
        assert_eq!(m.insns(), 0);
        assert_eq!(m.bus.cycles(), 0);
        assert_eq!(m.quantum, quantum);
    }
}
pub fn run() {
    timing_model_conflicts();
    let mut idle = esp32s3::machine([0; 6]);
    idle.bus.board = Box::new(esp_soc::NoBoard);
    idle.bus.attach_board_devices();
    idle.quantum = 64;
    idle.bus.load_bytes(0x4037_0000, &[0x06, 0xff, 0xff]).unwrap();
    idle.cores[0].pc = 0x4037_0000;
    idle.cores[0].ps = 0;
    assert_eq!(idle.browser_external_block_budget(64), Some(64));
    idle.bus.board = board(240_000_000, 4).0;
    idle.bus.attach_board_devices();
    assert_eq!(idle.browser_external_block_budget(64), None);
    idle.bus.board = Box::new(esp_soc::NoBoard);
    idle.bus.attach_board_devices();
    idle.run(1);
    assert_eq!(idle.bus.cycles(), 64);
    assert_eq!(idle.quantum, 64);
    for q in [1, 64, 256, 1024] {
        for jit in [false, true] {
            for core in [0, 1] {
                let mut m = esp32s3::machine([0; 6]);
                if core == 1 {
                    m.bus.load_bytes(0x4037_0000, &[0x06, 0xff, 0xff]).unwrap();
                    m.cores[0].pc = 0x4037_0000;
                    m.bus.write32(0x600c_0000, 2).unwrap();
                    m.run(64);
                    m.cores[0].waiting = true;
                }
                let (b, s) = board(240_000_000, 4);
                m.bus.board = b;
                m.bus.attach_board_devices();
                m.cores[core].set_jit(jit);
                m.cores[core].ps = 0;
                m.cores[core].set_ar(1, 0x6000_4008);
                m.cores[core].set_ar(2, 1 << 4);
                let m = check(
                    m,
                    s,
                    pulse_program(true, 240_000_000),
                    (0x6000_4000, 0x6000_9000, 256),
                    q,
                    core,
                );
                if jit && cfg!(all(target_arch = "aarch64", any(target_os = "macos", target_os = "linux"))) {
                    assert!(m.cores[core].blocks.jit_instructions > 0);
                }
            }
        }
        let mut m = esp32c3::machine([0; 6], 4 << 20);
        let (b, s) = board(160_000_000, 4);
        m.bus.board = b;
        m.bus.attach_board_devices();
        m.cores[0].x[1] = 0x6000_4008;
        m.cores[0].x[2] = 1 << 4;
        check(
            m,
            s,
            pulse_program(false, 160_000_000),
            (0x6000_4000, 0x6000_9000, 128),
            q,
            0,
        );
        let mut m = esp32c6::machine([0; 6], 4 << 20);
        let (b, s) = board(160_000_000, 4);
        m.bus.board = b;
        m.bus.attach_board_devices();
        m.cores[0].x[1] = 0x6009_1008;
        m.cores[0].x[2] = 1 << 4;
        check(
            m,
            s,
            pulse_program(false, 160_000_000),
            (0x6009_1000, 0x6009_0000, 128),
            q,
            0,
        );
    }
}
