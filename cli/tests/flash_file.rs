//! `--flash-file` and `--flash-out`: the flash array as a file.
//!
//! These run the real binary on a hand-built app image, so no mask ROM and no firmware build is
//! needed: the guest only spins, which makes the file's contents exactly what the options put
//! there. What a *guest* writes is shown by the Annex receipt (`docs/evidence/persistent-flash`).
#[path = "../../tests/common.rs"]
mod common;
use common::*;

const BIN: &str = env!("CARGO_BIN_EXE_esp32sim-c6");
/// `--flash-mb 2` for every run.
const FLASH_BYTES: usize = 2 << 20;

/// A C6 app image: one segment in HP SRAM holding `j .`, entered by `--boot app`.
fn spin_image() -> Vec<u8> {
    let mut image = vec![0xe9, 1, 0, 0];
    image.extend_from_slice(&0x4080_0000u32.to_le_bytes());              // entry
    image.resize(24, 0);
    image.extend_from_slice(&0x4080_0000u32.to_le_bytes());              // the segment: load address, length, data
    image.extend_from_slice(&4u32.to_le_bytes());
    image.extend_from_slice(&0x0000_006fu32.to_le_bytes());              // j .
    image
}

/// The app image on disk; `--app` writes it at 0x10000 in every run below.
fn app_image() -> String {
    let path = tmp("spin.bin");
    std::fs::write(&path, spin_image()).unwrap();
    path.to_str().unwrap().to_string()
}

struct Outcome { status: i32, stderr: String }

fn run(args: &[&str]) -> Outcome {
    let out = std::process::Command::new(BIN).args(args).current_dir(root()).output().unwrap();
    Outcome { status: out.status.code().unwrap_or(-1), stderr: String::from_utf8_lossy(&out.stderr).to_string() }
}

/// The common arguments: boot the app image, 2 MB flash, a fifth of a second, no register dump.
fn args<'a>(app: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec!["--boot", "app", "--app", app, "--flash-mb", "2", "--console", "none", "--no-dump", "--max-seconds", "0.2"];
    args.extend_from_slice(extra);
    args
}

fn read(path: &std::path::Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn a_new_flash_file_is_seeded_from_the_image_padded_and_written_back() {
    let app = app_image();
    let seed = tmp("seed.bin");
    std::fs::write(&seed, vec![0xa5; 0x2000]).unwrap();
    let flash = tmp("flash.bin");
    let (flash_path, seed_path) = (flash.to_str().unwrap(), seed.to_str().unwrap());

    let r = run(&args(&app, &["--flash-image", seed_path, "--flash-file", flash_path]));
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert!(r.stderr.contains("has no image yet: seeded from --flash-image"), "{}", r.stderr);

    let image = read(&flash);
    assert_eq!(image.len(), FLASH_BYTES, "the file is exactly the configured flash size");
    assert_eq!(image[..0x2000], vec![0xa5; 0x2000][..], "the seed image is at offset 0");
    assert!(image[0x2000..0x10000].iter().all(|&b| b == 0xff), "a short image is padded with erased cells, not zeros");
    assert_eq!(image[0x10000..0x10000 + spin_image().len()], spin_image()[..], "--app is written on top");
}

#[test]
fn a_firmware_that_writes_nothing_leaves_the_file_byte_identical() {
    let app = app_image();
    let flash = tmp("flash.bin");
    let flash_path = flash.to_str().unwrap();

    assert_eq!(run(&args(&app, &["--flash-file", flash_path])).status, 0);
    let before = sha256_hex(&read(&flash));
    let r = run(&args(&app, &["--flash-file", flash_path]));
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert!(r.stderr.contains("(--flash-file)"), "the second run loads the file: {}", r.stderr);
    assert_eq!(sha256_hex(&read(&flash)), before, "a run that writes no flash changes no byte");
}

#[test]
fn a_flash_file_larger_than_the_flash_is_refused() {
    let app = app_image();
    let flash = tmp("too-large.bin");
    std::fs::write(&flash, vec![0x11; FLASH_BYTES + 1]).unwrap();
    let before = sha256_hex(&read(&flash));

    let r = run(&args(&app, &["--flash-file", flash.to_str().unwrap()]));
    assert_eq!(r.status, 2, "{}", r.stderr);
    assert!(r.stderr.contains("larger than the 2097152 byte flash"), "{}", r.stderr);
    assert_eq!(sha256_hex(&read(&flash)), before, "the refused file is left alone");
}

#[test]
fn flash_out_dumps_the_final_flash_without_loading_it() {
    let app = app_image();
    let dump = tmp("dump.bin");
    let r = run(&args(&app, &["--flash-out", dump.to_str().unwrap()]));
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert!(r.stderr.contains("(--flash-out)"), "{}", r.stderr);

    let image = read(&dump);
    assert_eq!(image.len(), FLASH_BYTES);
    assert_eq!(image[0x10000..0x10000 + spin_image().len()], spin_image()[..], "the app the run booted");
    assert!(image[0x10000 + spin_image().len()..0x20000].iter().all(|&b| b == 0xff), "everything else is erased");
}

/// A run that owns a flash file survives a signal long enough to write it: the last `--max-seconds`
/// wins, so this run would otherwise continue for five seconds.
#[cfg(unix)]
#[test]
fn a_handled_signal_still_writes_the_flash_file() {
    use std::process::{Command, Stdio};
    let app = app_image();
    let flash = tmp("flash.bin");
    let child = Command::new(BIN)
        .args(args(&app, &["--flash-file", flash.to_str().unwrap(), "--max-seconds", "5"]))
        .current_dir(root()).stdout(Stdio::null()).stderr(Stdio::piped())
        .spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let signal = Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap();
    assert!(signal.success(), "kill -TERM {}", child.id());
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    assert_eq!(out.status.code(), Some(130), "the run ends by the signal, not by its budget: {stderr}");
    assert!(stderr.contains("interrupted by a signal"), "{stderr}");
    assert_eq!(read(&flash).len(), FLASH_BYTES, "the image was written before the process left");
}

/// A flash file that already holds an image is the state of the previous run: it wins over
/// `--flash-image`, which only seeds a file that does not exist yet.
#[test]
fn a_flash_file_with_an_image_wins_over_flash_image() {
    let app = app_image();
    let flash = tmp("flash.bin");
    std::fs::write(&flash, vec![0x5a; 0x1000]).unwrap();
    let seed = tmp("seed.bin");
    std::fs::write(&seed, vec![0xa5; 0x2000]).unwrap();
    let dump = tmp("dump.bin");

    let r = run(&args(&app, &["--flash-image", seed.to_str().unwrap(), "--flash-file", flash.to_str().unwrap(), "--flash-out", dump.to_str().unwrap()]));
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert!(r.stderr.contains("--flash-image ignored"), "the source that won is printed: {}", r.stderr);

    let image = read(&dump);
    assert!(image[..0x1000].iter().all(|&b| b == 0x5a), "the flash file's image was loaded");
    assert!(image[0x1000..0x10000].iter().all(|&b| b == 0xff), "padded to the flash size");
}
