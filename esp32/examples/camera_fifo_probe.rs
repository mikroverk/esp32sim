//! Emit deterministic sensor/FIFO vectors for the pinned public C driver filters.
use esp32::camera_fifo::CameraFifo;
use std::{fs, path::PathBuf};
fn main() {
    let out = PathBuf::from(std::env::args_os().nth(1).expect("output directory"));
    fs::create_dir_all(&out).unwrap();
    for (name, wire) in [
        ("rgb565", vec![0xf8, 0, 7, 0xe0, 0, 0x1f, 0xff, 0xff]),
        ("yuv", vec![16, 128, 32, 64, 48, 192, 64, 32]),
        ("row12", (1..=12).collect()),
        ("row16", (33..=48).collect()),
    ] {
        fs::write(out.join(format!("{name}.wire")), &wire).unwrap();
        for mode in [1, 3] {
            let mut fifo = CameraFifo::new(mode).unwrap();
            let mut packed = Vec::new();
            for sample in &wire { fifo.push(&[*sample], &mut packed); }
            fifo.end_row(&mut packed);
            fs::write(out.join(format!("{name}.{mode}.fifo")), packed).unwrap();
        }
    }
}
