//! The flash array as a file: the whole chip's flash, so what the guest stores there — NVS, otadata,
//! calibration, a filesystem — is still there when the next run starts.
//!
//! The file is always exactly the configured flash size. A file that is longer than the flash is
//! refused (it cannot be a dump of this chip), a shorter one is padded with [`ERASED`], the value an
//! unwritten flash cell reads as; padding with zeros would make the missing part look like data.
//!
//! Writing goes through a temporary file beside the target and a rename, so a run that is killed
//! while writing leaves the previous image intact instead of a half-written one.

use std::io::Write;
use std::path::{Path, PathBuf};

/// What a flash cell reads as that has never been written.
pub const ERASED: u8 = 0xff;

/// Read a flash image for a `size`-byte flash, padded with [`ERASED`] to exactly `size` bytes.
pub fn read(path: &Path, size: usize) -> Result<Vec<u8>, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if data.len() > size {
        return Err(format!("{}: {} bytes, larger than the {} byte flash (--flash-mb)", path.display(), data.len(), size));
    }
    let mut image = vec![ERASED; size];
    image[..data.len()].copy_from_slice(&data);
    Ok(image)
}

/// Write `image` to `path` atomically: a temporary file in the same directory, flushed, then renamed
/// over the target. `path`'s directory must exist; the temporary file is removed on failure.
pub fn write(path: &Path, image: &[u8]) -> Result<(), String> {
    let file_name = path.file_name().ok_or_else(|| format!("{}: not a file name", path.display()))?;
    let dir = match path.parent() { Some(d) if !d.as_os_str().is_empty() => d, _ => Path::new(".") };
    let tmp = temporary(dir, &file_name.to_string_lossy());
    let result = write_file(&tmp, image).and_then(|()| std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display())));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
        return result;
    }
    sync_dir(dir);
    Ok(())
}

fn temporary(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!(".{}.{}.tmp", name, std::process::id()))
}

fn write_file(path: &Path, image: &[u8]) -> Result<(), String> {
    let mut file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(image).map_err(|e| format!("{}: {e}", path.display()))?;
    file.sync_all().map_err(|e| format!("{}: {e}", path.display()))
}

/// Make the rename durable. Not every platform can open a directory; there the rename is still atomic.
fn sync_dir(dir: &Path) {
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("esp-soc-flash-{}-{name}", std::process::id()))
    }

    #[test]
    fn a_round_trip_returns_the_same_bytes_at_the_configured_size() {
        let dir = scratch("round-trip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("flash.bin");
        let mut image = vec![ERASED; 4096];
        image[0x100..0x109].copy_from_slice(b"persisted");
        write(&path, &image).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 4096);
        assert_eq!(read(&path, 4096).unwrap(), image);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_short_file_is_padded_with_erased_cells_not_zeros() {
        let dir = scratch("pad");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("short.bin");
        std::fs::write(&path, [0x01, 0x02, 0x03]).unwrap();
        let image = read(&path, 8).unwrap();
        assert_eq!(image, [0x01, 0x02, 0x03, 0xff, 0xff, 0xff, 0xff, 0xff]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_empty_file_is_a_fresh_flash() {
        let dir = scratch("empty");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("empty.bin");
        std::fs::write(&path, []).unwrap();
        assert_eq!(read(&path, 16).unwrap(), vec![ERASED; 16]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_larger_than_the_flash_is_refused() {
        let dir = scratch("too-large");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("large.bin");
        std::fs::write(&path, vec![0; 17]).unwrap();
        let error = read(&path, 16).unwrap_err();
        assert!(error.contains("larger than the 16 byte flash"), "{error}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_fresh_flash() {
        let dir = scratch("missing");
        std::fs::create_dir_all(&dir).unwrap();
        let error = read(&dir.join("absent.bin"), 16).unwrap_err();
        assert!(error.contains("absent.bin"), "{error}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn writing_replaces_the_old_image_and_leaves_no_temporary_file() {
        let dir = scratch("replace");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("flash.bin");
        write(&path, &[0x11; 4]).unwrap();
        write(&path, &[0x22; 4]).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), [0x22; 4]);
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left, [path.file_name().unwrap().to_owned()]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_temporary_file_from_a_killed_run_is_not_read_back() {
        let dir = scratch("stale-temp");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("flash.bin");
        write(&path, &[0x33; 4]).unwrap();
        // as a run killed mid-write would leave it
        std::fs::write(temporary(&dir, "flash.bin"), [0x44; 4]).unwrap();
        assert_eq!(read(&path, 4).unwrap(), [0x33; 4]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
