//! Classic I2S0 parallel-camera FIFO packing, separate from DMA and GPIO.
//!
//! Derived from esp32-camera v2.1.4 (b817e9b8), target/esp32/ll_cam.c:
//! dma_elem_t and sampling modes at 62-85, filters at 123-209. On the
//! little-endian guest sample1 is byte 2 and sample2 byte 0. Not silicon-tested.

/// An unsupported FIFO mode must not fall back to the PCM producer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnsupportedCameraFifoMode(pub u8);

/// Stateful byte-to-word adapter. The capture owner supplies row boundaries and
/// resets; this object does not observe registers, advance clocks or write DMA.
/// Only complete words are appended to caller-owned output. In mode 1 an odd
/// row tail is padded with a zero sample2; the caller must retain the true row
/// length. This deterministic tail policy is inferred, not a silicon claim.
#[derive(Debug)]
pub struct CameraFifo {
    mode: u8,
    pending: Option<u8>,
}

impl CameraFifo {
    pub fn new(mode: u8) -> Result<Self, UnsupportedCameraFifoMode> {
        match mode {
            1 | 3 => Ok(Self { mode, pending: None }),
            _ => Err(UnsupportedCameraFifoMode(mode)),
        }
    }

    /// Append sensor wire bytes, preserving position across arbitrary chunks.
    /// Mode 1: [s2, 0, s1, 0]. Mode 3: [0, 0, s1, 0].
    pub fn push(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        for &sample in bytes {
            if self.mode == 3 {
                out.extend_from_slice(&[0, 0, sample, 0]);
            } else if let Some(first) = self.pending.take() {
                out.extend_from_slice(&[sample, 0, first, 0]);
            } else {
                self.pending = Some(sample);
            }
        }
    }

    /// Finish one row without pairing its tail with the next row's first byte.
    /// Even-length mode-1 rows and every mode-3 row need no padding.
    pub fn end_row(&mut self, out: &mut Vec<u8>) {
        if let Some(first) = self.pending.take() {
            out.extend_from_slice(&[0, 0, first, 0]);
        }
    }

    /// Discard an unfinished word, e.g. on capture/FIFO reset or new frame.
    pub fn reset(&mut self) {
        self.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(mode: u8, input: &[u8]) -> Vec<u8> {
        let mut fifo = CameraFifo::new(mode).unwrap();
        let mut out = Vec::new();
        fifo.push(input, &mut out);
        fifo.end_row(&mut out);
        out
    }
    fn extract(mode: u8, packed: &[u8]) -> Vec<u8> {
        packed.as_chunks::<4>().0.iter().flat_map(|w| {
            if mode == 1 { vec![w[2], w[0]] } else { vec![w[2]] }
        }).collect()
    }
    #[test]
    fn camera_fifo_exact_little_endian_words() {
        let wire = [0x12, 0xa5, 0x34, 0xfe];
        assert_eq!(pack(1, &wire), [0xa5, 0, 0x12, 0, 0xfe, 0, 0x34, 0]);
        assert_eq!(pack(3, &wire), [0, 0, 0x12, 0, 0, 0, 0xa5, 0, 0, 0, 0x34, 0, 0, 0, 0xfe, 0]);
    }
    #[test]
    fn camera_fifo_rgb565_yuv_and_grayscale() {
        // RGB565 wire byte order is retained, not converted to host pixel order.
        for wire in [[0xf8, 0, 7, 0xe0, 0, 0x1f, 0xff, 0xff],
                     [16, 128, 32, 64, 48, 192, 64, 32]] {
            for mode in [1, 3] {
                let recovered = extract(mode, &pack(mode, &wire));
                assert_eq!(recovered, wire);
                assert_eq!(recovered.iter().step_by(2).copied().collect::<Vec<_>>(),
                    [wire[0], wire[2], wire[4], wire[6]]);
            }
        }
    }
    #[test]
    fn camera_fifo_arbitrary_splits_and_empty_chunks() {
        let wire: Vec<_> = (0..=255).collect();
        for mode in [1, 3] {
            let expected = pack(mode, &wire);
            for a in 0..=wire.len() {
                for b in a..=wire.len() {
                    let mut fifo = CameraFifo::new(mode).unwrap();
                    let mut out = Vec::new();
                    fifo.push(&wire[..a], &mut out);
                    fifo.push(&[], &mut out);
                    fifo.push(&wire[a..b], &mut out);
                    fifo.push(&wire[b..], &mut out);
                    fifo.end_row(&mut out);
                    assert_eq!(out, expected, "mode {mode}, splits {a}/{b}");
                }
            }
        }
    }
    #[test]
    fn camera_fifo_row_tail_is_not_next_row() {
        for mode in [1, 3] {
            let mut fifo = CameraFifo::new(mode).unwrap();
            let mut out = vec![0xcc]; // append, do not overwrite caller output
            fifo.push(&[1, 2, 3], &mut out);
            fifo.end_row(&mut out);
            fifo.end_row(&mut out); // idempotent
            fifo.push(&[4, 5], &mut out);
            fifo.end_row(&mut out);
            let mut expected = vec![0xcc];
            expected.extend(pack(mode, &[1, 2, 3]));
            expected.extend(pack(mode, &[4, 5]));
            assert_eq!(out, expected);
            if mode == 1 { assert_eq!(&out[5..9], &[0, 0, 3, 0]); }
            assert_eq!(extract(mode, &pack(mode, &[1, 2, 3]))[..3], [1, 2, 3]);
        }
    }
    #[test]
    fn camera_fifo_reset_discards_partial_word() {
        let mut fifo = CameraFifo::new(1).unwrap();
        let mut out = Vec::new();
        fifo.push(&[99], &mut out);
        assert!(out.is_empty());
        fifo.reset();
        fifo.push(&[1, 2], &mut out);
        fifo.end_row(&mut out);
        assert_eq!(out, [2, 0, 1, 0]);
        fifo.push(&[88], &mut out);
        fifo.reset();
        fifo.end_row(&mut out);
        assert_eq!(out, [2, 0, 1, 0]);
        for mode in [1, 3] {
            let mut fifo = CameraFifo::new(mode).unwrap();
            let mut out = Vec::new();
            fifo.push(&[7, 8], &mut out);
            fifo.reset();
            fifo.push(&[9, 10], &mut out);
            fifo.end_row(&mut out);
            assert_eq!(extract(mode, &out), [7, 8, 9, 10]);
        }
    }
    #[test]
    fn camera_fifo_rejects_every_unsupported_mode() {
        for mode in 0..=u8::MAX {
            assert_eq!(CameraFifo::new(mode).is_ok(), mode == 1 || mode == 3, "mode {mode}");
        }
    }
}
