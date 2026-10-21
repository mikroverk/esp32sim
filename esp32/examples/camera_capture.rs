//! Run the public esp32-camera fixture with a deterministic host image.
#[path = "../tests/support/camera_fixture.rs"]
mod fixture;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let build = std::path::PathBuf::from(args.next().expect("fixture build directory"));
    let rom = std::path::PathBuf::from(args.next().expect("classic ROM ELF"));
    fixture::capture(&build, &rom);
}
