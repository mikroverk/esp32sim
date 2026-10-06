use std::io::{self, Read};

pub fn size(s: &str) -> Result<(u16, u16), String> {
    let (w, h) = s.split_once('x').ok_or("--cam-size requires WIDTHxHEIGHT")?;
    if !w.bytes().all(|b| b.is_ascii_digit()) || !h.bytes().all(|b| b.is_ascii_digit()) { return Err("--cam-size requires unsigned decimal WIDTHxHEIGHT".into()); }
    let (w, h) = (w.parse::<u16>().map_err(|_| "invalid camera width")?, h.parse::<u16>().map_err(|_| "invalid camera height")?);
    if esp_soc::picture::rgba_len(w.into(), h.into()).is_none() {
        return Err("camera frame must be nonempty and fit the 8 MiB host input limit".into());
    }
    Ok((w, h))
}

/// A blocking reader lives on its own thread. Only the latest complete frame is queued.
/// EOF between frames retains the last image; a partial final frame is an error.
pub fn read_frames(mut input: impl Read, w: u16, h: u16, mut push: impl FnMut(esp_soc::picture::Picture)) -> io::Result<()> {
    let mut rgb = vec![0; usize::from(w) * usize::from(h) * 3];
    loop {
        match input.read(&mut rgb[..1]) {
            Ok(0) => return Ok(()),
            Ok(_) => (),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
        input.read_exact(&mut rgb[1..])?;
        push(esp_soc::picture::Picture { w: w.into(), h: h.into(), rgb: rgb.clone() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stream_frames_and_truncation() {
        let mut frames = Vec::new();
        read_frames(&[1, 2, 3, 4, 5, 6][..], 1, 1, |f| frames.push(f.rgb)).unwrap();
        assert_eq!(frames, [vec![1, 2, 3], vec![4, 5, 6]]);
        assert_eq!(read_frames(&[1, 2][..], 1, 1, |_| panic!("partial frame")).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(size("640x480").unwrap(), (640, 480));
        for bad in ["", "0x1", "1x0", "65535x65535", "1", "1x-1", "+2x+2"] { assert!(size(bad).is_err()); }
    }
}
