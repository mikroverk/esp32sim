//! Scheduled classic camera transport. Uses the native I2S channel and its one
//! internal-DRAM adapter; no S3 GDMA registers or second descriptor walker.
use crate::{board::aithinker::{ARM_WINDOW, FRAME_CYCLES, VSYNC_CYCLES}, bus::SocBus, camera_fifo::CameraFifo};
use std::sync::Arc;

type Frame = Arc<Vec<u8>>;

pub(crate) struct Capture {
    armed_at: u64,
    saw_vsync: bool,
    packed: Vec<u8>,
    pos: usize,
    frame_at: Option<u64>,
    qualified: bool,
    // Cold capture-only cache: no new field in the bus or ordinary PCM path.
    inverted: Option<(Frame, u8, Frame)>,
}
impl Capture {
    pub(crate) fn qualified(&self) -> bool { self.qualified }
    pub(crate) fn armed(armed_at: u64) -> Self {
        Self { armed_at, saw_vsync: false, packed: Vec::new(), pos: 0, frame_at: None, qualified: false, inverted: None }
    }
    pub(crate) fn inverted_frame(&mut self, source: Frame, mask: u8) -> Frame {
        match &self.inverted {
            Some((old, old_mask, frame)) if *old_mask == mask && Arc::ptr_eq(old, &source) => return frame.clone(),
            _ => {}
        }
        let frame = Arc::new(source.iter().map(|byte| byte ^ mask).collect::<Vec<_>>());
        self.inverted = Some((source, mask, frame.clone()));
        frame
    }
}
impl SocBus {
    // Only entered behind the existing clock + running-DMA gate. Keeping this
    // out of line prevents camera packing from expanding the classic tick body.
    #[inline(never)]
    pub(crate) fn camera_dma_data(&mut self, _cycles: u64) -> Option<Vec<u8>> {
        let rx = self.periph.i2s[0].camera_rx()?;
        if !rx.started { return Some(Vec::new()); }
        let Some((width, height, wire)) = self.camera_routed_frame() else {
            // Recording a start is not yet a route-qualified capture. Preserve
            // C7's wait on initially unavailable routes, then make loss terminal
            // once this receiver has actually observed a valid source.
            if self.periph.i2s[0].camera.as_ref().is_some_and(|c| c.qualified) {
                self.periph.i2s[0].abort_camera_dma();
                self.irq_dirty = true;
            }
            return Some(Vec::new());
        };
        let high = self.periph.gpio.signal_input(191) == Some(true);
        let edge = self.board.camera_frame_start(false);
        let capture = self.periph.i2s[0].camera.get_or_insert_with(|| Box::new(Capture::armed(self.cycles)));
        capture.qualified = true;
        let fresh = edge.is_some_and(|at| at <= capture.armed_at && capture.armed_at - at <= ARM_WINDOW);
        if high && (!capture.packed.is_empty() || !fresh) {
            // End the previous active interval at the next VSYNC. Flush its
            // final whole words before beginning a new frame. EOF is a native
            // chunk interrupt, never an invented whole-frame completion flag.
            let tail = capture.packed[capture.pos..].to_vec();
            capture.packed.clear(); capture.pos = 0; capture.frame_at = None;
            capture.saw_vsync = true;
            return Some(tail);
        }
        if capture.packed.is_empty() {
            let Some(at) = edge.filter(|&at| capture.saw_vsync ||
                (at <= capture.armed_at && capture.armed_at - at <= ARM_WINDOW)) else { return Some(Vec::new()); };
            capture.saw_vsync = false;
            let Some(row_bytes) = (width as usize).checked_mul(2).filter(|n| *n != 0) else { return Some(Vec::new()) };
            if wire.len() != row_bytes.saturating_mul(height as usize) { return Some(Vec::new()); }
            let Ok(mut fifo) = CameraFifo::new(rx.fifo_mode) else { return Some(Vec::new()) };
            for row in wire.chunks_exact(row_bytes) { fifo.push(row, &mut capture.packed); fifo.end_row(&mut capture.packed); }
            capture.frame_at = Some(at);
            // A consumed opportunity survives I2S stop/reset in the board, so a
            // second start cannot replay even the first word of this frame.
            self.board.camera_frame_start(true);
        }
        // Both pulse-armed and delayed starts use the physical falling edge.
        // Catch up on the first tick after start, independent of tick splitting.
        let elapsed = self.cycles.saturating_sub(capture.frame_at?).min(FRAME_CYCLES - VSYNC_CYCLES);
        // Approximate uniform word delivery over the existing 10Hz frame. This
        // is not PCLK timing. Whole words survive arbitrary guest tick splits.
        let words = capture.packed.len() / 4;
        let end = ((elapsed as u128 * words as u128) / u128::from(FRAME_CYCLES - VSYNC_CYCLES)) as usize * 4;
        let data = capture.packed[capture.pos..end].to_vec();
        capture.pos = end;
        if high {
            capture.packed.clear(); capture.pos = 0; capture.frame_at = None;
            capture.saw_vsync = true;
        }
        Some(data)
    }
}
