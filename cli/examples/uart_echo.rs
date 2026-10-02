//! Run the UART evidence sketch with a board-owned 9600-baud echo endpoint on GPIO5/4.
//! Usage: cargo run --release -p esp32sim --example uart_echo -- s3|c3|c6 BUILD_DIR ROM_ELF
use esp_soc::{
    uart::{UartInput, UartRoute},
    BoardModel, LoadKind, Machine, Soc, Stop,
};
use std::{cell::RefCell, path::Path, rc::Rc};

#[derive(Default)]
struct Counts {
    received: usize,
    mismatches: usize,
    wrong_pin: usize,
}
struct Echo {
    counts: Rc<RefCell<Counts>>,
    pending: Vec<UartInput>,
}
impl BoardModel for Echo {
    fn name(&self) -> &'static str {
        "uart-echo"
    }
    fn uart_tx(&mut self, route: UartRoute, byte: u8) {
        let mut counts = self.counts.borrow_mut();
        if route.transmits_on(7) && route.matches_baud(9600) {
            counts.wrong_pin += 1;
        }
        if !route.transmits_on(5) {
            return;
        }
        if !route.matches_baud(9600) {
            counts.mismatches += 1;
            return;
        }
        counts.received += 1;
        self.pending.push(UartInput {
            pin: 4,
            baud: 9600,
            data: vec![byte],
        });
    }
    fn uart_rx(&mut self) -> Vec<UartInput> {
        std::mem::take(&mut self.pending)
    }
}

fn run<S: Soc>(mut machine: Machine<S>, build: &Path, rom: &Path, counts: Rc<RefCell<Counts>>) {
    machine.console.capture = true;
    machine
        .load_input(LoadKind::Rom, &std::fs::read(rom).unwrap())
        .unwrap();
    for (kind, file) in [
        (LoadKind::Bootloader, "bootloader.bin"),
        (LoadKind::Partitions, "partitions.bin"),
        (LoadKind::App, "firmware.bin"),
    ] {
        machine
            .load_input(kind, &std::fs::read(build.join(file)).unwrap())
            .unwrap();
    }
    machine.boot_rom();
    machine.max_cycles = S::CPU_HZ * 10;
    let mut resets = 0;
    for _ in 0..2 {
        match machine.run(u64::MAX) {
            Stop::SwReset => {
                resets += 1;
                if resets == 2 {
                    break;
                }
                machine.reboot();
            }
            stop => panic!("unexpected stop: {stop:?}"),
        }
    }
    machine.drain_console();
    let output = String::from_utf8_lossy(&machine.console.uart0);
    for line in output.lines().filter(|line| line.starts_with("UART ")) {
        println!("{line}");
    }
    let counts = counts.borrow();
    println!(
        "chip={} received={} mismatches={} wrong_pin={} resets={}",
        S::NAME,
        counts.received,
        counts.mismatches,
        counts.wrong_pin,
        resets
    );
    assert_eq!(resets, 2, "firmware did not reset twice");
    assert_eq!(
        output.matches("UART echo=PASS bytes=10").count(),
        4,
        "{output}"
    );
    assert_eq!(output.matches("UART PASS reset").count(), 2);
    assert!(!output.contains("UART FAIL"));
    assert_eq!(
        (counts.received, counts.mismatches, counts.wrong_pin),
        (40, 2, 0)
    );
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args.len(), 4, "expected CHIP BUILD_DIR ROM_ELF");
    let counts = Rc::new(RefCell::new(Counts::default()));
    let board = Box::new(Echo {
        counts: counts.clone(),
        pending: Vec::new(),
    });
    let (build, rom) = (Path::new(&args[2]), Path::new(&args[3]));
    match args[1].as_str() {
        "s3" => {
            let mut m = esp32s3::machine([0; 6]);
            m.bus.board = board;
            run(m, build, rom, counts);
        }
        "c3" => {
            let mut m = esp32c3::machine([0; 6], 4 * 1024 * 1024);
            m.bus.board = board;
            run(m, build, rom, counts);
        }
        "c6" => {
            let mut m = esp32c6::machine([0; 6], 4 * 1024 * 1024);
            m.bus.board = board;
            run(m, build, rom, counts);
        }
        _ => panic!("expected s3, c3 or c6"),
    }
}
