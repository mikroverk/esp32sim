//! Feetech ST3215 (STS3215) serial bus servos on a half-duplex TTL line, as the SCServo library's
//! `SMS_STS` class (Feetech's Arduino library, `SCS.cpp`/`SMS_STS.h`, 2021.3.11) drives them.
//!
//! Packets (`SCS::writeBuf`, `SCS::syncWrite`, `SCS::Ack`/`Read`): `FF FF ID LEN INSTR PARAMS CHK`
//! from the host, `FF FF ID LEN ERR PARAMS CHK` back, with LEN = params + 2 and CHK the inverted
//! low byte of the sum from ID on. ID 0xFE broadcasts and is never answered (except PING). The
//! instructions (`INST.h`): PING 0x01, READ 0x02 (addr, n), WRITE 0x03 (addr, data), REG_WRITE
//! 0x04 (held until ACTION 0x05), SYNC_READ 0x82 (addr, n, IDs: each listed servo answers in
//! order) and SYNC_WRITE 0x83 (addr, n, then ID + n bytes per servo; no answer). Words are little
//! endian (`SMS_STS` sets `End` 0); positions and speeds are sign-magnitude with bit 15 the sign,
//! the load with bit 10 (`SMS_STS::ReadPos`, `ReadSpeed`, `ReadLoad`). Answers follow the
//! response level (address 8: 0 answers only PING and READ). A bad checksum drops the packet.
//!
//! The memory table is `SMS_STS.h`'s (and Feetech's STS3215 memory table; LeRobot's feetech
//! control table has the same addresses): EEPROM 3-4 model (777, as LeRobot lists the STS3215),
//! 5 ID, 6 baud (0 = 1 Mbaud: `INST.h` `_1M`..`_4800`), 7 return delay (2 us units), 8 response
//! level, 9-12 angle limits, 21-23 PID P/D/I (ugv_config.h `ST_PID_*_ADDR`), 26-27 dead bands,
//! 31-32 middle offset (sign bit 11), 33 mode (0 position, 1 constant speed); SRAM 40 torque
//! enable (128 = calibrate the middle: `SMS_STS::CalibrationOfs`), 41 acceleration (100 step/s^2
//! units, 0 = no ramp), 42-43 goal position, 44-45 goal time (ms), 46-47 goal speed (step/s, 0 =
//! the fastest), 48-49 torque limit, 55 EEPROM lock; read-only 56-57 present position, 58-59
//! speed, 60-61 load, 62 voltage (0.1 V), 63 temperature (degrees C), 65 status, 66 moving, 69-70
//! current. A full turn is 4096 steps (0..4095 = 0..360 degrees).
//!
//! Motion: the present position runs to the goal at the goal speed (capped at the ST3215's 45 rpm
//! no-load speed at 12 V, Waveshare's ST3215 page: 0.222 s/60 degrees = 3072 step/s) with the
//! acceleration ramping up and down (a trapezoid), or at once without one. Mode 1 runs at the
//! signed goal speed instead. No load, gravity or stall: the servo always gets where it is told.
//!
//! Guessed (not from Feetech's documents): the EEPROM defaults besides the ID and baud, that the
//! servo powers up with torque on and that a goal write turns it on, that while torque is off the
//! goal follows the present position (so torque off-on stops a move where it is, which
//! ugv_base_ros's `gimbalUserCtrl` relies on), that the goal time sets the speed when the goal
//! speed is 0, that calibration leaves torque as it was, that an ID write answers from the old ID,
//! that broadcast PINGs are answered by every servo one after another, the reply turnaround
//! (the return delay alone), a 5 ms idle gap resetting a half-received packet, the voltage
//! (whatever the board supplies), 35 degrees C, and zero load and current. EEPROM writes take
//! effect whatever the lock says (the lock only decides what survives a power cycle, which the
//! model does not have). Not modelled: protections and their status bits, multi-turn and
//! stepper modes, the PID (stored, unused), RESET and other instructions (ignored).
use std::collections::VecDeque;

pub const ST3215_MODEL: u16 = 777;
/// Steps per second at full speed (45 rpm * 4096 / 60).
pub const ST3215_MAX_SPEED: f64 = 3072.0;
pub const BROADCAST: u8 = 0xfe;
const TABLE: usize = 128;
/// The motion model's step.
const STEP_S: f64 = 0.001;

pub mod reg {
    pub const MODEL_L: u8 = 3; pub const ID: u8 = 5; pub const BAUD: u8 = 6; pub const RETURN_DELAY: u8 = 7;
    pub const RESPONSE_LEVEL: u8 = 8; pub const MIN_ANGLE: u8 = 9; pub const MAX_ANGLE: u8 = 11;
    pub const PID_P: u8 = 21; pub const PID_D: u8 = 22; pub const PID_I: u8 = 23; pub const OFFSET: u8 = 31; pub const MODE: u8 = 33;
    pub const TORQUE_ENABLE: u8 = 40; pub const ACC: u8 = 41; pub const GOAL_POSITION: u8 = 42; pub const GOAL_TIME: u8 = 44;
    pub const GOAL_SPEED: u8 = 46; pub const TORQUE_LIMIT: u8 = 48; pub const LOCK: u8 = 55;
    pub const PRESENT_POSITION: u8 = 56; pub const PRESENT_SPEED: u8 = 58; pub const PRESENT_LOAD: u8 = 60;
    pub const PRESENT_VOLTAGE: u8 = 62; pub const PRESENT_TEMPERATURE: u8 = 63; pub const STATUS: u8 = 65;
    pub const MOVING: u8 = 66; pub const PRESENT_CURRENT: u8 = 69;
}
pub mod inst {
    pub const PING: u8 = 0x01; pub const READ: u8 = 0x02; pub const WRITE: u8 = 0x03; pub const REG_WRITE: u8 = 0x04;
    pub const ACTION: u8 = 0x05; pub const SYNC_READ: u8 = 0x82; pub const SYNC_WRITE: u8 = 0x83;
}

/// The packet checksum: the inverted low byte of the sum of `bytes` (ID through the last parameter).
pub fn checksum(bytes: &[u8]) -> u8 { !bytes.iter().fold(0u8, |s, &b| s.wrapping_add(b)) }
/// A complete packet: header, `id`, length, `op` (instruction or error byte), `params`, checksum.
pub fn packet(id: u8, op: u8, params: &[u8]) -> Vec<u8> {
    let mut p = vec![0xff, 0xff, id, (params.len() + 2) as u8, op];
    p.extend_from_slice(params);
    p.push(checksum(&p[2..]));
    p
}
/// Baud rate of the baud register's code (`INST.h`).
pub fn baud_of(code: u8) -> Option<u32> {
    [1_000_000, 500_000, 250_000, 128_000, 115_200, 76_800, 57_600, 38_400, 19_200, 14_400, 9_600, 4_800].get(code as usize).copied()
}
fn sign_mag(v: f64, bit: u32) -> u16 {
    let m = (v.abs().round() as u32).min((1 << bit) - 1) as u16;
    if v < 0.0 && m != 0 { m | 1 << bit } else { m }
}
fn from_sign_mag(v: u16, bit: u32) -> f64 { let m = f64::from(v & ((1 << bit) - 1)); if v >> bit & 1 != 0 { -m } else { m } }

/// One servo: its memory table and its shaft.
#[derive(Clone, Debug)]
pub struct St3215 {
    regs: [u8; TABLE],
    /// Shaft position in steps, in the frame the present position reads (middle offset applied).
    pos: f64,
    /// Signed speed in steps/s.
    speed: f64,
    /// Top speed of the move under way (steps/s).
    vmax: f64,
    /// A REG_WRITE waiting for ACTION: address and data.
    pending: Option<(u8, Vec<u8>)>,
    /// Supply voltage, volts (the board's battery).
    pub supply_v: f64,
}
impl St3215 {
    /// A servo with ID `id` at the middle (2048), torque on, defaults otherwise.
    pub fn new(id: u8) -> Self {
        let mut regs = [0u8; TABLE];
        regs[0] = 3; regs[1] = 10;          // firmware version: guessed
        regs[3..5].copy_from_slice(&ST3215_MODEL.to_le_bytes());
        regs[reg::ID as usize] = id;
        regs[reg::RESPONSE_LEVEL as usize] = 1;
        regs[11..13].copy_from_slice(&4095u16.to_le_bytes());
        regs[13] = 70; regs[14] = 140; regs[15] = 40;   // temperature and voltage limits
        regs[16..18].copy_from_slice(&1000u16.to_le_bytes());
        regs[reg::PID_P as usize] = 32; regs[reg::PID_D as usize] = 32;
        regs[26] = 1; regs[27] = 1;
        regs[reg::TORQUE_ENABLE as usize] = 1;
        regs[48..50].copy_from_slice(&1000u16.to_le_bytes());
        regs[reg::LOCK as usize] = 1;
        let mut s = St3215 { regs, pos: 2048.0, speed: 0.0, vmax: ST3215_MAX_SPEED, pending: None, supply_v: 12.0 };
        s.set_word(reg::GOAL_POSITION, 2048);
        s.refresh();
        s
    }
    /// Puts the shaft at `steps` (where it was left at power-up), at rest and holding there.
    pub fn place(&mut self, steps: f64) {
        if steps.is_finite() { self.pos = steps.clamp(0.0, 4095.0); }
        self.speed = 0.0;
        self.hold();
        self.refresh();
    }
    pub fn id(&self) -> u8 { self.regs[reg::ID as usize] }
    pub fn baud(&self) -> Option<u32> { baud_of(self.regs[reg::BAUD as usize]) }
    pub fn torque(&self) -> bool { self.regs[reg::TORQUE_ENABLE as usize] != 0 }
    fn word(&self, a: u8) -> u16 { u16::from_le_bytes([self.regs[a as usize], self.regs[a as usize + 1]]) }
    fn set_word(&mut self, a: u8, v: u16) { self.regs[a as usize..a as usize + 2].copy_from_slice(&v.to_le_bytes()); }
    /// The present position register (0..4095 in position mode).
    pub fn position(&self) -> u16 { self.word(reg::PRESENT_POSITION) }
    /// The present position in steps, unrounded.
    pub fn position_steps(&self) -> f64 { self.pos }
    /// Signed present speed in steps/s.
    pub fn speed(&self) -> f64 { self.speed }
    /// The goal position the servo is heading for (sign-magnitude decoded).
    pub fn goal(&self) -> f64 { from_sign_mag(self.word(reg::GOAL_POSITION), 15) }
    pub fn moving(&self) -> bool { self.speed != 0.0 || (self.torque() && self.mode() == 0 && (self.goal_clamped() - self.pos).abs() >= 0.5) }
    fn mode(&self) -> u8 { self.regs[reg::MODE as usize] }
    pub fn reg(&self, a: u8) -> u8 { self.regs.get(a as usize).copied().unwrap_or(0) }
    fn goal_clamped(&self) -> f64 {
        let (lo, hi) = (self.word(reg::MIN_ANGLE), self.word(reg::MAX_ANGLE));
        let g = self.goal();
        if lo == 0 && hi == 0 { g.clamp(0.0, 4095.0) } else { g.clamp(f64::from(lo.min(hi)), f64::from(hi.max(lo))) }
    }
    /// The read-only registers from the shaft and the supply.
    fn refresh(&mut self) {
        let p = if self.mode() == 1 { self.pos.rem_euclid(4096.0) } else { self.pos };
        self.set_word(reg::PRESENT_POSITION, sign_mag(p, 15));
        self.set_word(reg::PRESENT_SPEED, sign_mag(self.speed, 15));
        self.set_word(reg::PRESENT_LOAD, 0);
        self.regs[reg::PRESENT_VOLTAGE as usize] = if self.supply_v.is_finite() { (self.supply_v * 10.0).round().clamp(0.0, 255.0) as u8 } else { 0 };
        self.regs[reg::PRESENT_TEMPERATURE as usize] = 35;
        self.regs[reg::STATUS as usize] = 0;
        self.regs[reg::MOVING as usize] = u8::from(self.moving());
        self.set_word(reg::PRESENT_CURRENT, 0);
    }
    /// `n` bytes from `addr` (zeros past the table), as READ answers them.
    pub fn read(&mut self, addr: u8, n: u8) -> Vec<u8> {
        self.refresh();
        (0..n as usize).map(|i| self.regs.get(addr as usize + i).copied().unwrap_or(0)).collect()
    }
    /// A WRITE (or the ACTION of a REG_WRITE): bytes into the table, then their effects.
    pub fn write(&mut self, addr: u8, data: &[u8]) {
        let (a0, a1) = (addr as usize, addr as usize + data.len());
        let touches = |lo: u8, hi: u8| a0 <= hi as usize && a1 > lo as usize;
        let torque_was = self.torque();
        for (i, &b) in data.iter().enumerate() {
            let a = a0 + i;
            // the model number and versions, and the present values, are read-only
            if a < TABLE && !(a <= 4 || (56..=70).contains(&a)) { self.regs[a] = b; }
        }
        if touches(reg::TORQUE_ENABLE, reg::TORQUE_ENABLE) {
            match self.regs[reg::TORQUE_ENABLE as usize] {
                128 => {
                    // middle calibration: the present position becomes 2048 (offset sign bit 11)
                    let ofs = from_sign_mag(self.word(reg::OFFSET), 11) + (self.pos.round() - 2048.0);
                    self.set_word(reg::OFFSET, sign_mag(ofs.clamp(-2047.0, 2047.0), 11));
                    self.pos = 2048.0;
                    self.hold();
                    self.regs[reg::TORQUE_ENABLE as usize] = u8::from(torque_was);
                }
                0 => { self.hold(); self.speed = 0.0; }
                _ => { self.regs[reg::TORQUE_ENABLE as usize] = 1; if !torque_was { self.hold(); } }
            }
        }
        if touches(reg::GOAL_POSITION, reg::GOAL_SPEED + 1) {
            self.regs[reg::TORQUE_ENABLE as usize] = 1;
            let spd = from_sign_mag(self.word(reg::GOAL_SPEED), 15).abs();
            let time_s = f64::from(self.word(reg::GOAL_TIME)) / 1000.0;
            self.vmax = if spd > 0.0 { spd.min(ST3215_MAX_SPEED) }
                else if time_s > 0.0 { ((self.goal_clamped() - self.pos).abs() / time_s).clamp(1.0, ST3215_MAX_SPEED) }
                else { ST3215_MAX_SPEED };
        }
        self.refresh();
    }
    /// Goal := present position, nothing to do.
    fn hold(&mut self) {
        let p = self.pos.round();
        self.set_word(reg::GOAL_POSITION, sign_mag(p, 15));
    }
    /// One motion step of `dt` seconds.
    fn step(&mut self, dt: f64) {
        if !self.torque() { self.speed = 0.0; self.hold(); return; }
        let acc = f64::from(self.regs[reg::ACC as usize]) * 100.0;
        let ramp = |v: f64, target: f64| if acc == 0.0 { target } else { v + (target - v).clamp(-acc * dt, acc * dt) };
        if self.mode() == 1 {
            let target = from_sign_mag(self.word(reg::GOAL_SPEED), 15).clamp(-ST3215_MAX_SPEED, ST3215_MAX_SPEED);
            self.speed = ramp(self.speed, target);
            self.pos = (self.pos + self.speed * dt).rem_euclid(4096.0);
            return;
        }
        let goal = self.goal_clamped();
        let dist = goal - self.pos;
        // slow down in time to stop at the goal
        let cruise = if acc == 0.0 { self.vmax } else { self.vmax.min((2.0 * acc * dist.abs()).sqrt()) };
        let v = ramp(self.speed, dist.signum() * cruise);
        let next = self.pos + v * dt;
        if dist.abs() < 0.5 || (goal - next).signum() != dist.signum() { self.pos = goal; self.speed = 0.0; } else { self.pos = next; self.speed = v; }
    }
    /// Advance the shaft by `dt` seconds; false when it was at rest throughout.
    pub fn advance(&mut self, dt: f64) -> bool {
        if !self.moving() { return false; }
        let mut t = dt;
        while t > 0.0 && self.moving() { self.step(t.min(STEP_S)); t -= STEP_S; }
        self.refresh();
        true
    }
    /// The present angle in degrees (360 per 4096 steps).
    pub fn degrees(&self) -> f64 { self.pos * 360.0 / 4096.0 }
}

/// Servos on one half-duplex line, with a packet parser and the line's timing: what the host sends
/// arrives at its line rate, and the answers come back byte by byte after the return delay.
pub struct St3215Bus {
    pub servos: Vec<St3215>,
    hz: u64,
    /// The packet being received, and the line rate it comes at.
    rx: Vec<u8>,
    rx_baud: u32,
    /// When the line is next free of the host's bytes (cycles).
    wire: u64,
    /// Answer bytes and the cycle each is complete at, with their line rate.
    out: VecDeque<(u64, u32, u8)>,
    /// Motion time (cycles).
    now: u64,
}
impl St3215Bus {
    /// Servos `ids` (fresh, at the middle) on a line whose times are cycles of an `hz` clock.
    pub fn new(hz: u64, ids: &[u8]) -> Self {
        St3215Bus { servos: ids.iter().map(|&id| St3215::new(id)).collect(), hz: hz.max(1), rx: Vec::new(), rx_baud: 0, wire: 0, out: VecDeque::new(), now: 0 }
    }
    pub fn servo(&self, id: u8) -> Option<&St3215> { self.servos.iter().find(|s| s.id() == id) }
    pub fn servo_mut(&mut self, id: u8) -> Option<&mut St3215> { self.servos.iter_mut().find(|s| s.id() == id) }
    fn byte_cycles(&self, baud: u32) -> u64 { (10 * self.hz).div_ceil(u64::from(baud.max(1))) }
    /// Moves the shafts to `cycle`; true if any moved.
    pub fn advance(&mut self, cycle: u64) -> bool {
        if cycle <= self.now { return false; }
        let dt = (cycle - self.now) as f64 / self.hz as f64;
        self.now = cycle;
        let mut moved = false;
        for s in &mut self.servos { moved |= s.advance(dt); }
        moved
    }
    /// A byte the host sent at `cycle` at `baud`.
    pub fn receive(&mut self, cycle: u64, baud: u32, byte: u8) {
        let bt = self.byte_cycles(baud);
        // a pause mid-packet (5 ms of idle line) or a change of line rate starts over
        if !self.rx.is_empty() && (cycle > self.wire.saturating_add(self.hz / 200) || baud != self.rx_baud) { self.rx.clear(); }
        self.wire = self.wire.max(cycle).saturating_add(bt);
        self.rx_baud = baud;
        self.rx.push(byte);
        // header FF FF (further FFs before the ID belong to it), ID, LEN, then LEN bytes
        match self.rx.len() {
            1 | 2 => if byte != 0xff { self.rx.clear(); },
            3 => if byte == 0xff { self.rx.pop(); },
            4 => if byte < 2 { self.rx.clear(); },
            n if n == 4 + self.rx[3] as usize => {
                let p = std::mem::take(&mut self.rx);
                if checksum(&p[2..n - 1]) == p[n - 1] {
                    // the shafts as they are now; the answer leaves once the packet is off the line
                    self.advance(cycle);
                    self.execute(self.wire, baud, p[2], p[4], &p[5..n - 1]);
                }
            }
            _ => {}
        }
    }
    fn answer(&mut self, at: u64, servo: usize, params: &[u8]) {
        let s = &self.servos[servo];
        let Some(baud) = s.baud() else { return };
        let delay = u64::from(s.reg(reg::RETURN_DELAY)) * 2 * self.hz / 1_000_000;
        let bt = self.byte_cycles(baud);
        let mut t = at.saturating_add(delay).max(self.out.back().map_or(0, |o| o.0));
        for b in packet(s.id(), 0, params) { t = t.saturating_add(bt); self.out.push_back((t, baud, b)); }
    }
    fn execute(&mut self, end: u64, baud: u32, id: u8, op: u8, params: &[u8]) {
        let hears = |s: &St3215| s.baud().is_some_and(|b| b.abs_diff(baud) as u64 * 100 <= u64::from(b) * 3);
        if op == inst::SYNC_WRITE || op == inst::SYNC_READ {
            if id != BROADCAST || params.len() < 2 { return; }
            let (addr, n) = (params[0], params[1] as usize);
            if op == inst::SYNC_READ {
                for &sid in &params[2..] {
                    let Some(i) = self.servos.iter().position(|s| s.id() == sid && hears(s)) else { continue };
                    let data = self.servos[i].read(addr, n as u8);
                    self.answer(end, i, &data);
                }
            } else {
                for chunk in params[2..].chunks_exact(n + 1) {
                    if let Some(s) = self.servos.iter_mut().find(|s| s.id() == chunk[0] && hears(s)) { s.write(addr, &chunk[1..]); }
                }
            }
            return;
        }
        for i in 0..self.servos.len() {
            let s = &mut self.servos[i];
            if !(s.id() == id || id == BROADCAST) || !hears(s) { continue; }
            let level = s.reg(reg::RESPONSE_LEVEL);
            let reply = match op {
                inst::PING => Some(Vec::new()),
                inst::READ if params.len() == 2 && id != BROADCAST => Some(s.read(params[0], params[1])),
                inst::WRITE if !params.is_empty() => { s.write(params[0], &params[1..]); (level != 0).then(Vec::new) }
                inst::REG_WRITE if !params.is_empty() => { s.pending = Some((params[0], params[1..].to_vec())); (level != 0).then(Vec::new) }
                inst::ACTION => { if let Some((a, d)) = s.pending.take() { s.write(a, &d); } (level != 0).then(Vec::new) }
                _ => None,
            };
            if let Some(r) = reply.filter(|_| id != BROADCAST || op == inst::PING) { self.answer(end, i, &r); }
        }
    }
    /// Answer bytes complete by `cycle`, grouped by line rate.
    pub fn take_due(&mut self, cycle: u64) -> Vec<(u32, Vec<u8>)> {
        let mut got: Vec<(u32, Vec<u8>)> = Vec::new();
        while let Some(&(t, baud, b)) = self.out.front() {
            if t > cycle { break; }
            self.out.pop_front();
            match got.last_mut() { Some((bd, v)) if *bd == baud => v.push(b), _ => got.push((baud, vec![b])) }
        }
        got
    }
    /// When the next answer byte is complete.
    pub fn next_due(&self) -> Option<u64> { self.out.front().map(|o| o.0) }
    pub fn any_moving(&self) -> bool { self.servos.iter().any(St3215::moving) }
}

#[cfg(test)]
mod tests {
    use super::*;
    const HZ: u64 = 1_000_000;   // one cycle per microsecond
    const BAUD: u32 = 1_000_000;

    fn send(bus: &mut St3215Bus, at: u64, p: &[u8]) { for &b in p { bus.receive(at, BAUD, b); } }
    /// Everything the servos answer by `until`.
    fn answers(bus: &mut St3215Bus, until: u64) -> Vec<u8> { bus.take_due(until).into_iter().flat_map(|(_, v)| v).collect() }

    #[test]
    fn checksum_and_packets_match_the_scservo_library() {
        // SCS::writeBuf for WritePosEx(1, 2048, 1000, 50): FF FF 01 0A 03 29 32 00 08 00 00 E8 03 CHK
        let p = packet(1, inst::WRITE, &[0x29, 50, 0x00, 0x08, 0, 0, 0xe8, 0x03]);
        assert_eq!(p[..5], [0xff, 0xff, 1, 10, 3]);
        let sum: u32 = [1u32, 10, 3, 0x29, 50, 0, 8, 0, 0, 0xe8, 3].iter().sum();
        assert_eq!(*p.last().unwrap(), !(sum as u8));
        assert_eq!(checksum(&[1, 2, 0]), !3);
    }

    #[test]
    fn ping_read_and_write_answer_after_the_packet_and_return_delay() {
        let mut bus = St3215Bus::new(HZ, &[1, 2]);
        send(&mut bus, 100, &packet(2, inst::PING, &[]));
        // the 6-byte ping takes 60 us on the line; the 6-byte answer 60 more
        assert_eq!(bus.next_due(), Some(170));
        assert_eq!(answers(&mut bus, 219), packet(2, 0, &[])[..5]);
        assert_eq!(answers(&mut bus, 220), packet(2, 0, &[])[5..]);
        // READ the model number
        send(&mut bus, 1000, &packet(1, inst::READ, &[reg::MODEL_L, 2]));
        assert_eq!(answers(&mut bus, 2000), packet(1, 0, &777u16.to_le_bytes()));
        // WRITE the return delay (250 = 500 us) is acknowledged; a later PING then waits for it
        send(&mut bus, 3000, &packet(1, inst::WRITE, &[reg::RETURN_DELAY, 250]));
        assert_eq!(answers(&mut bus, 4000), packet(1, 0, &[]));
        send(&mut bus, 5000, &packet(1, inst::PING, &[]));
        assert_eq!(bus.next_due(), Some(5000 + 60 + 500 + 10));
        // a bad checksum or another ID: no answer
        answers(&mut bus, 8000);
        let mut bad = packet(1, inst::PING, &[]); *bad.last_mut().unwrap() ^= 1;
        send(&mut bus, 9000, &bad);
        send(&mut bus, 9100, &packet(7, inst::PING, &[]));
        assert!(answers(&mut bus, 20_000).is_empty());
        // broadcast WRITE and response level 0: no acknowledgement, READ still answers
        send(&mut bus, 30_000, &packet(BROADCAST, inst::WRITE, &[reg::RESPONSE_LEVEL, 0]));
        send(&mut bus, 31_000, &packet(2, inst::WRITE, &[reg::PID_P, 16]));
        assert!(answers(&mut bus, 40_000).is_empty());
        send(&mut bus, 41_000, &packet(2, inst::READ, &[reg::PID_P, 3]));
        assert_eq!(answers(&mut bus, 50_000), packet(2, 0, &[16, 32, 0]));
    }

    #[test]
    fn sync_write_moves_both_servos_at_the_commanded_speed() {
        let mut bus = St3215Bus::new(HZ, &[2, 1]);
        // SMS_STS::SyncWritePosEx: ACC, position, time 0, speed for each ID; 1000 step/s, no ramp
        let mut params = vec![reg::ACC, 7];
        params.extend([2, 0, 0x00, 0x0c, 0, 0, 0xe8, 0x03]);           // ID 2 to 3072
        params.extend([1, 0, 0x00, 0x84, 0, 0, 0xe8, 0x03]);           // ID 1 to -1024: clamps at 0
        send(&mut bus, 0, &packet(BROADCAST, inst::SYNC_WRITE, &params));
        assert_eq!(bus.next_due(), None, "SYNC_WRITE is not answered");
        bus.advance(500_000);
        assert!((bus.servo(2).unwrap().position_steps() - 2548.0).abs() < 2.0);
        assert!((bus.servo(1).unwrap().position_steps() - 1548.0).abs() < 2.0);
        // present speed reads sign-magnitude: ID 1 runs backwards
        send(&mut bus, 500_000, &packet(1, inst::READ, &[reg::PRESENT_POSITION, 4]));
        let a = answers(&mut bus, 600_000);
        assert_eq!(u16::from_le_bytes([a[7], a[8]]), 1000 | 0x8000);
        assert_eq!(a[4], 0, "no error");
        bus.advance(3_000_000);
        assert_eq!((bus.servo(2).unwrap().position(), bus.servo(1).unwrap().position()), (3072, 0));
        assert!(!bus.any_moving());
    }

    #[test]
    fn acceleration_ramps_and_speed_zero_is_the_fastest() {
        let mut s = St3215::new(1);
        // 10 * 100 step/s^2, speed 0 (= 3072 step/s): 0.1 s in, the shaft has done a*t^2/2 = 5 steps
        s.write(reg::ACC, &[10, 0x00, 0x0c, 0, 0, 0, 0]);
        s.advance(0.1);
        assert!((s.position_steps() - 2053.0).abs() < 1.0, "{}", s.position_steps());
        assert!((s.speed() - 100.0).abs() < 1.0);
        // and it stops at the goal, not past it
        s.advance(5.0);
        assert_eq!((s.position(), s.speed()), (3072, 0.0));
        // no ramp: full speed at once
        s.write(reg::ACC, &[0, 0x00, 0x08, 0, 0, 0, 0]);
        s.advance(0.1);
        assert!((s.position_steps() - (3072.0 - 307.2)).abs() < 1.0);
    }

    #[test]
    fn torque_off_stops_the_move_and_reg_write_waits_for_action() {
        let mut bus = St3215Bus::new(HZ, &[1]);
        send(&mut bus, 0, &packet(1, inst::WRITE, &[reg::ACC, 0, 0x00, 0x0c, 0, 0, 0xf4, 0x01]));   // 3072 at 500 step/s
        bus.advance(1_000_000);
        send(&mut bus, 1_000_000, &packet(1, inst::WRITE, &[reg::TORQUE_ENABLE, 0]));
        send(&mut bus, 1_005_000, &packet(1, inst::WRITE, &[reg::TORQUE_ENABLE, 1]));
        bus.advance(3_000_000);
        let p = bus.servo(1).unwrap().position();
        assert!((2545..=2551).contains(&p), "held where torque went off: {p}");
        // REG_WRITE holds the goal until ACTION
        send(&mut bus, 3_000_000, &packet(1, inst::REG_WRITE, &[reg::GOAL_POSITION, 0x00, 0x04]));
        bus.advance(4_000_000);
        assert_eq!(bus.servo(1).unwrap().position(), p);
        send(&mut bus, 4_000_000, &packet(BROADCAST, inst::ACTION, &[]));
        bus.advance(9_000_000);
        assert_eq!(bus.servo(1).unwrap().position(), 1024);
        // middle calibration: the present position reads 2048 and the offset keeps the difference
        send(&mut bus, 9_000_000, &packet(1, inst::WRITE, &[reg::TORQUE_ENABLE, 128]));
        let s = bus.servo(1).unwrap();
        assert_eq!((s.position(), s.reg(reg::OFFSET), s.reg(reg::OFFSET + 1), s.torque()), (2048, 0x00, 0x0c, true));
    }

    #[test]
    fn garbage_and_split_packets_do_not_panic_or_confuse_the_parser() {
        let mut bus = St3215Bus::new(HZ, &[1]);
        send(&mut bus, 0, &[0x00, 0xff, 0x12, 0xff, 0xff, 0xff, 0x01, 0x00]);   // LEN 0: dropped
        send(&mut bus, 100, &[0xff, 0xff, 0x01, 0x04, 0x02, 0xff]);           // READ cut short ...
        send(&mut bus, 20_000, &packet(1, inst::PING, &[]));                   // ... and forgotten
        assert_eq!(answers(&mut bus, 30_000), packet(1, 0, &[]));
        // reads past the table, short or empty parameters, odd sync chunks
        send(&mut bus, 40_000, &packet(1, inst::READ, &[120, 20]));
        assert_eq!(answers(&mut bus, 50_000).len(), 26);
        for p in [packet(1, inst::READ, &[]), packet(1, inst::WRITE, &[]), packet(BROADCAST, inst::SYNC_WRITE, &[42, 0, 1]),
                  packet(BROADCAST, inst::SYNC_WRITE, &[]), packet(1, 0x99, &[1, 2]), packet(1, inst::WRITE, &[127, 1, 2, 3])] {
            send(&mut bus, 60_000, &p);
        }
        assert_eq!(answers(&mut bus, 69_000), packet(1, 0, &[]), "only the last WRITE is acknowledged");
        // the wrong line rate is not heard
        for b in packet(1, inst::PING, &[]) { bus.receive(70_000, 115_200, b); }
        assert!(answers(&mut bus, 100_000).is_empty());
    }
}
