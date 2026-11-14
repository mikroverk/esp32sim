//! Scripted IMU motion shared between a board and its inertial sensor model (the S3's QMI8658,
//! the BMI270): the board page's motion selector stores a mode, the board stores the guest
//! clock, and the sensor samples the script as a function of guest time, so a benchmark that
//! tilts and shakes the board stays exact on every platform.
use std::sync::atomic::{AtomicU32, AtomicU64};

/// Board clock and motion selection shared between a board and its IMU. `cycle` is in whatever
/// unit the board's clock counts; the sensor model is told the rate.
#[derive(Default, Debug)]
pub struct ImuMotion { pub cycle: AtomicU64, pub mode: AtomicU32 }

/// Sine from + - * / and floor only, so every platform and build produces the same bits.
pub fn sin(x: f64) -> f64 {
    const TAU: f64 = std::f64::consts::TAU;
    let mut x = x - TAU * (x / TAU + 0.5).floor();
    if x > std::f64::consts::FRAC_PI_2 { x = std::f64::consts::PI - x; }
    if x < -std::f64::consts::FRAC_PI_2 { x = -std::f64::consts::PI - x; }
    let x2 = x * x;
    x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0 * (1.0 - x2 / 72.0 * (1.0 - x2 / 110.0)))))
}
fn smoothstep(a: f64, b: f64, t: f64) -> f64 { let x = ((t - a) / (b - a)).clamp(0.0, 1.0); x * x * (3.0 - 2.0 * x) }

/// The handling script's attitude at `t` seconds, in radians: roll about the screen normal (z)
/// and lean about the screen's x axis, both 0 upright at rest.
pub fn motion_attitude_screen(t: f64) -> (f64, f64) {
    use std::f64::consts::{PI, TAU};
    let on = smoothstep(3.0, 5.0, t);
    let flip = PI * (smoothstep(13.0, 15.0, t) - smoothstep(20.0, 22.0, t));
    let roll = on * (1.1 * sin(TAU * (t - 3.0) / 8.0) + 0.35 * sin(TAU * (t - 3.0) / 2.9)) + flip;
    let lean = on * 0.45 * sin(TAU * (t - 3.0) / 11.0);
    (roll, lean)
}

/// A vector fixed in the world (the geomagnetic field), as the board sees it at `t` seconds of
/// the handling script, given what it reads upright at rest: the same rotation that turns the
/// script's gravity, so a magnetometer and an accelerometer on the board agree.
pub fn rotate_screen(v: [f64; 3], t: f64) -> [f64; 3] {
    let (roll, lean) = motion_attitude_screen(t);
    let cos = |x: f64| sin(x + std::f64::consts::FRAC_PI_2);
    let (sr, cr, sl, cl) = (sin(roll), cos(roll), sin(lean), cos(lean));
    // lean about x, then roll about z: (0, 1, 0) goes to the script's "down" vector.
    let (x, y, z) = (v[0], v[1] * cl - v[2] * sl, v[1] * sl + v[2] * cl);
    [x * cr + y * sr, -x * sr + y * cr, z]
}

/// The 30 s handling script in the board's screen frame (x right, y down the screen, z into the
/// case): settle upright, then swing and lean, turn upside down and back, with three shakes.
/// Returns (accelerometer g, gyro dps); each sensor model maps them onto its own axes.
pub fn motion_script_screen(t: f64) -> ([f64; 3], [f64; 3]) {
    use std::f64::consts::{PI, TAU};
    let angles = motion_attitude_screen;
    let (roll, lean) = angles(t);
    let cos = |x: f64| sin(x + PI / 2.0);
    // The accelerometer reports the reaction to gravity: minus the unit "down" vector.
    let mut a = [-sin(roll) * cos(lean), -cos(roll) * cos(lean), -sin(lean)];
    for (start, axis) in [(10.0, 0), (18.0, 1), (25.0, 2)] {
        if (start..start + 0.8).contains(&t) {
            let window = sin(PI * (t - start) / 0.8);
            a[axis] += 2.0 * window * window * sin(TAU * 7.0 * (t - start));
        }
    }
    let h = 0.001;
    let ((r0, l0), (r1, l1)) = (angles(t - h), angles(t + h));
    let deg = 180.0 / PI / (2.0 * h);
    (a, [(l1 - l0) * deg, 0.0, (r1 - r0) * deg])
}
