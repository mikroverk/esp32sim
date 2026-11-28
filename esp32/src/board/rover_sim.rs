//! The `waveshare-ros-driver` board's link to a physics simulation (`--board
//! waveshare-ros-driver,sim=HOST:PORT`): the board connects to HOST:PORT over TCP, retrying
//! while nobody listens, and exchanges one JSON object per line in wall-clock time (the run is
//! `--realtime`-paced when a host drives it, as with `--uart-tcp`).
//!
//! Board to simulation, every 10 ms: `{"duty":[l,r],"speed":[l,r],"travel":[l,r],"pt":[pan,tilt],"led":[io4,io5]}`,
//! the signed H-bridge duty (-1..1) per side, the wheel speeds (m/s) and travel (m) the encoders
//! report, and the pan-tilt gimbal's present angles in degrees from its servos' shafts, in the
//! firmware's convention (`ros_driver::GimbalShared`: T:133's X and Y, T:1001's pan and tilt; 0, 0
//! at the servos' middle, pan = (pos - 2047) * 360 / 4095, tilt = (2047 - pos) * 360 / 4095),
//! and the two LED lights' brightness 0..1 (`led`: IO4 by the depth camera, IO5 by the USB camera,
//! as the firmware's T:132 sets their PWM).
//! Simulation to board, any subset of the keys in one line:
//! - `"speed":[l,r]`: wheel surface speeds in m/s; the encoders follow them instead of the built-in
//!   motor model while the link is up.
//! - `"q":[w,x,y,z]`, `"w":[x,y,z]`, `"a":[x,y,z]`, `"mag":[x,y,z]`: the IMU's pose (body to world),
//!   body angular rate (rad/s), world linear acceleration without gravity (m/s^2) and the world
//!   magnetic field (uT), as `Icm20948Input` takes them.
//! - `"bat":[volts,amps]`: what the INA219 measures.
//!
//! Values that are not finite numbers, or arrays of the wrong length, are ignored. When the
//! simulation disconnects the board returns to its own motor model and the link waits for the next.
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;
use esp_soc::devices::{Icm20948Input, Ina219Shared};
use super::ros_driver::{DriveShared, GimbalShared};

/// The numbers of `"key":[...]` in a flat JSON line, if it holds exactly `n` finite ones.
pub fn numbers(line: &str, key: &str, n: usize) -> Option<Vec<f64>> {
    let at = line.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = line[at..].trim_start().strip_prefix(':')?.trim_start().strip_prefix('[')?;
    let body = &rest[..rest.find(']')?];
    let v: Vec<f64> = body.split(',').map(|s| s.trim().parse::<f64>()).collect::<Result<_, _>>().ok()?;
    (v.len() == n && v.iter().all(|x| x.is_finite())).then_some(v)
}

/// Applies one line from the simulation to the board's shared state.
pub fn apply(line: &str, drive: &DriveShared, imu: &Icm20948Input, battery: &Ina219Shared) {
    if let Some(s) = numbers(line, "speed", 2) {
        let mut d = drive.lock().unwrap_or_else(|e| e.into_inner());
        d.speed_mps = [s[0].clamp(-10.0, 10.0), s[1].clamp(-10.0, 10.0)];
        d.external = true;
    }
    if let Some(q) = numbers(line, "q", 4) {
        let n = q.iter().map(|x| x * x).sum::<f64>().sqrt();
        if n > 1e-9 { imu.set_orientation([q[0] / n, q[1] / n, q[2] / n, q[3] / n]); }
    }
    if let Some(w) = numbers(line, "w", 3) { imu.set_angular_rate([w[0], w[1], w[2]]); }
    if let Some(a) = numbers(line, "a", 3) { imu.set_linear_accel([a[0], a[1], a[2]]); }
    if let Some(m) = numbers(line, "mag", 3) { imu.set_mag_field([m[0], m[1], m[2]]); }
    if let Some(b) = numbers(line, "bat", 2) {
        let mut i = battery.lock().unwrap_or_else(|e| e.into_inner());
        i.bus_v = b[0].clamp(0.0, 32.0);
        i.current_a = b[1].clamp(-50.0, 50.0);
    }
}

/// The board's line to the simulation.
pub fn line(d: &super::ros_driver::Drive, pt: [f64; 2]) -> String {
    format!("{{\"duty\":[{:.4},{:.4}],\"speed\":[{:.4},{:.4}],\"travel\":[{:.5},{:.5}],\"pt\":[{:.2},{:.2}],\"led\":[{:.3},{:.3}]}}\n",
            d.duty[0], d.duty[1], d.speed_mps[0], d.speed_mps[1], d.travel_m[0], d.travel_m[1], pt[0], pt[1], d.lights[0], d.lights[1])
}

/// Starts the link thread; it lives as long as the process.
pub fn spawn(addr: String, drive: DriveShared, imu: Icm20948Input, battery: Ina219Shared, gimbal: GimbalShared) {
    std::thread::Builder::new().name("rover-sim".into()).spawn(move || {
        let mut said = false;
        loop {
            let Ok(s) = TcpStream::connect(&addr) else {
                if !said { eprintln!("[emu] ros-driver: waiting for the simulation at {addr}"); said = true; }
                std::thread::sleep(Duration::from_millis(500));
                continue;
            };
            eprintln!("[emu] ros-driver: simulation connected at {addr}");
            let _ = s.set_nodelay(true);
            let Ok(reader) = s.try_clone() else { continue };
            let (d2, i2, b2) = (drive.clone(), imu.clone(), battery.clone());
            let rx = std::thread::spawn(move || {
                for line in BufReader::new(reader).lines() {
                    let Ok(line) = line else { break };
                    apply(&line, &d2, &i2, &b2);
                }
            });
            let mut w = s;
            while !rx.is_finished() {
                let d = *drive.lock().unwrap_or_else(|e| e.into_inner());
                let msg = line(&d, *gimbal.lock().unwrap_or_else(|e| e.into_inner()));
                if w.write_all(msg.as_bytes()).is_err() { break; }
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = w.shutdown(std::net::Shutdown::Both);
            let _ = rx.join();
            drive.lock().unwrap_or_else(|e| e.into_inner()).external = false;
            eprintln!("[emu] ros-driver: simulation disconnected");
            said = false;
        }
    }).expect("spawn the rover-sim thread");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use esp_soc::devices::Ina219Input;

    #[test]
    fn lines_set_speeds_pose_and_battery_and_bad_values_are_ignored() {
        let drive: DriveShared = Default::default();
        let imu = Icm20948Input::default();
        let bat = Arc::new(Mutex::new(Ina219Input { bus_v: 11.8, current_a: 0.25, shunt_ohm: 0.01 }));
        apply(r#"{"speed":[0.25,-0.5],"q":[0,0,0,2],"w":[0,0,1.5],"bat":[11.1, 2.5]}"#, &drive, &imu, &bat);
        let d = *drive.lock().unwrap();
        assert!(d.external && d.speed_mps == [0.25, -0.5]);
        let p = imu.get();
        assert_eq!((p.orientation, p.angular_rate), ([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.5]));
        let b = *bat.lock().unwrap();
        assert_eq!((b.bus_v, b.current_a), (11.1, 2.5));
        // wrong lengths, non-numbers, missing brackets: no change, no panic
        apply(r#"{"speed":[1],"w":[0,0,"x"],"a":[1e999,0,0],"q":[0,0,0,0],"bat":5}"#, &drive, &imu, &bat);
        assert_eq!(drive.lock().unwrap().speed_mps, [0.25, -0.5]);
        assert_eq!(imu.get().angular_rate, [0.0, 0.0, 1.5]);
        assert_eq!(imu.get().linear_accel, [0.0; 3]);
        assert_eq!(imu.get().orientation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(numbers(r#"{"a" : [ 1, 2.5 ,-3 ]}"#, "a", 3), Some(vec![1.0, 2.5, -3.0]));
    }

    #[test]
    fn the_board_line_carries_wheels_gimbal_and_lights() {
        let d = super::super::ros_driver::Drive { lights: [0.5, 1.0], ..Default::default() };
        let l = line(&d, [44.923, -19.956]);
        assert_eq!(numbers(&l, "led", 2), Some(vec![0.5, 1.0]));
        assert_eq!(numbers(&l, "pt", 2), Some(vec![44.92, -19.96]));
        assert_eq!(numbers(&l, "travel", 2), Some(vec![0.0, 0.0]));
        assert!(l.ends_with("}\n"));
    }
}
