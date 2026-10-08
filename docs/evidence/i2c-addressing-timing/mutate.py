#!/usr/bin/env python3
"""Run EX216 rule-removal checks from the repository root; restore every source."""
import json
import pathlib
import subprocess

I2C = "esp-periph/src/i2c.rs"
cases = [
    ("idle deadline gate", I2C, "fn has_deadline(&self) -> bool { self.active }", "fn has_deadline(&self) -> bool { true }", "controller_leaves"),
    ("active deadline gate", I2C, "fn has_deadline(&self) -> bool { self.active }", "fn has_deadline(&self) -> bool { false }", "controller_leaves"),
    ("Pocket Tank byte rate", I2C, "+ extra) * 9", "+ extra) * 10", "pocket_tank_amoled"),
    ("default address", I2C, "self.address(configured) == address", "configured == address", "default_address_match"),
    ("address update", I2C, "*addr = device.address(*addr);", "", "shared_address_change"),
    ("address predicate", I2C, "device.start_address(*configured, addr, rd)", "*configured == addr", "shared_programmable"),
    ("start rejection", I2C, "&& device.start_address(*configured, addr, rd)", "&& { device.start_address(*configured, addr, rd); true }", "shared_general"),
    ("pin filter", I2C, "(device.pins().is_none() || device.pins() == self.pins)", "true", "shared_general"),
    ("pins before callback", I2C, "(device.pins().is_none() || device.pins() == self.pins) && device.start_address(*configured, addr, rd)", "device.start_address(*configured, addr, rd) && (device.pins().is_none() || device.pins() == self.pins)", "shared_programmable"),
    ("all recipients", I2C, "self.cur.push(k);", "self.cur.push(k); break;", "shared_general"),
    ("ACK aggregation", I2C, "ack |= device.write(b);", "ack = device.write(b) && ack;", "shared_general"),
    ("wired AND reads", I2C, "b &= self.devices[k].1.read();", "b = self.devices[k].1.read();", "shared_programmable"),
    ("STOP callbacks", I2C, "for &k in &self.cur { self.devices[k].1.stop(); }", "", "shared_general"),
    ("END selection", I2C, "4 => { self.int_raw", "4 => { self.cur.clear(); self.int_raw", "shared_general"),
    ("detach reindex", I2C, "*cur -= usize::from(*cur > index);", "", "shared_general"),
    ("route invalidation", I2C, "if self.pins != pins { self.cur.clear(); }", "", "shared_general"),
    ("delayed transfer", I2C, "self.remaining = self.clock_ticks(cycles.into());", "self.remaining = 1; let _ = cycles;", "byte_deadlines"),
    ("byte ACK clock", I2C, "+ extra) * 9", "+ extra) * 8", "byte_deadlines"),
    ("low period offset", I2C, "& mask) + 1 + (high", "& mask) + (high", "byte_deadlines"),
    ("wait high", I2C, "let extra = (high >> 9) & 127;", "let extra = 0;", "wait_high"),
    ("period mask", I2C, "let mask = 0x1ff;", "let mask = u32::MAX;", "wait_high"),
    ("fractional divider", I2C, "if a > 0 { b } else { 0 }", "0 * b", "dividers_and"),
    ("RC source", I2C, "17_500_000", "40_000_000", "dividers_and"),
    ("round up", I2C, "(cycles * divisor * 80_000_000).div_ceil(hz * denominator)", "(cycles * divisor * 80_000_000) / (hz * denominator)", "dividers_and"),
    ("FSM cancellation", I2C, "if reset { self.active = false;", "if reset {", "timed_nack"),
    ("busy start guard", I2C, "if self.active { return; }", "", "busy_start"),
    ("command bound", I2C, "self.command_index >= 8", "self.command_index >= 7", "busy_start"),
    ("idle clock", I2C, "self.active.then_some(ClockDomain::Apb)", "Some(ClockDomain::Apb)", "controller_leaves"),
    ("C6 PCR clock", "esp32c6/src/periph.rs", "self.i2c.external_clock_config = Some(", "self.i2c.external_clock_config = None; let _ = Some(", "timed_callbacks"),
    ("C6 fractional A", "esp32c6/src/periph.rs", "((clock & 63) << 8)", "0", "timed_callbacks"),
    ("C6 fractional B", "esp32c6/src/periph.rs", "(((clock >> 6) & 63) << 14)", "0", "timed_callbacks"),
    ("C6 source", "esp32c6/src/periph.rs", "(clock & (1 << 20))", "0", "timed_callbacks"),
    ("S3 clock dispatch", "esp32s3/src/periph.rs", '"I2C0" optional', '"I2C0" alias', "timed_callbacks"),
    ("C6 clock dispatch", "esp32c6/src/periph.rs", '"I2C0" optional', '"I2C0" alias', "timed_callbacks"),
    ("C3 clock dispatch", "esp32c3/src/periph.rs", '"I2C0" optional', '"I2C0" alias', "timed_callbacks"),
    ("C3 edge delivery", "esp32c3/src/bus.rs", "if self.board_edges && advance_board {", "if self.board_edges && self.board.next_deadline().is_some() {", "c3_i2c_tick"),
    ("C3 board time", "esp32c3/src/bus.rs", "if advance_board { self.board.advance_to(self.cycles); }", "", "timed_callbacks"),
    # Board order: restore main's advance-after-tick (one deliver_board_inputs call after the peripheral tick).
    ("S3 board order", "esp32s3/src/bus.rs", "if self.board_edges { self.board.advance_to(self.cycles); }\n", "", "timed_callbacks",
     ("gpio::drain_board_inputs", "gpio::deliver_board_inputs")),
    ("C6 board order", "esp32c6/src/bus.rs", "if self.board_edges { self.board.advance_to(self.cycles); }\n", "", "timed_callbacks",
     ("gpio::drain_board_inputs", "gpio::deliver_board_inputs")),
]
results = []
for label, name, old, new, test, *extra in cases:
    path = pathlib.Path(name)
    source = path.read_text()
    mutated = source
    for before, after in [(old, new), *extra]:
        assert mutated.count(before) == 1, (label, before, mutated.count(before))
        mutated = mutated.replace(before, after)
    target = "i2c_address"
    try:
        path.write_text(mutated)
        run = subprocess.run(["cargo", "+1.99.0", "test", "-p", "esp32sim",
                              "--test", target, test, "--", "--nocapture"],
                             stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        killed = run.returncode != 0 and "test result: FAILED" in run.stdout
        tests = [line.split()[1] for line in run.stdout.splitlines()
                 if line.startswith("test ") and line.endswith(" ... FAILED")]
        results.append({"mutation": label, "killed": killed, "tests": tests})
        print(json.dumps(results[-1]), flush=True)
        if not killed:
            print(run.stdout, flush=True)
            raise SystemExit("mutation survived or failed to compile")
    finally:
        path.write_text(source)
pathlib.Path("docs/evidence/i2c-addressing-timing/mutations.json").write_text(
    json.dumps(results, indent=2) + "\n")
