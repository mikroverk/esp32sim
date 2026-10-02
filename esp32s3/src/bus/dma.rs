//! GDMA transfer engines and descriptor access.
use super::*;
use std::collections::HashSet;

pub(super) const SPI2_DMA_DESCRIPTOR_STEP_BUDGET: usize = 1024;
/// Maximum descriptor reads in one pump call. This bounds zero-progress rings and crypto
/// allocation (each descriptor carries at most 4095 bytes).
const GDMA_DESCRIPTOR_STEP_BUDGET: usize = 4096;

/// Which end of a memory-to-memory copy faulted.
pub(super) enum M2mFault { Source, Destination }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmaDescriptorWord {
    Control,
    Buffer,
    Next,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmaDescriptorFault {
    Read { descriptor: u32, word: DmaDescriptorWord, fault: Fault },
    BufferRead { descriptor: u32, address: u32, fault: Fault },
    Writeback { descriptor: u32, fault: Fault },
    NotOwned { descriptor: u32 },
    Cycle { descriptor: u32 },
    StepBudgetExceeded { budget: usize },
    PayloadTooShort { expected: usize, actual: usize },
}

pub(super) struct Spi2DmaCompletion {
    pub(super) channel: usize,
    desc: u32,
    buf_pos: u32,
    running: bool,
    eof_desc: Option<u32>,
    raised_interrupts: u32,
    descriptor_writebacks: Vec<(u32, u32)>,
    payload: Vec<u8>,
}


/// Bound work even when guest descriptors make no progress. Streaming rings are valid: the
/// bound applies to one pump call, not the lifetime of an I2S or LCD channel.
struct DescriptorWalk { remaining: usize, budget: usize }
impl DescriptorWalk {
    fn new(budget: usize) -> Self { Self { remaining: budget, budget } }
    fn read(&mut self, bus: &mut SocBus, addr: u32) -> Result<(u32, crate::periph::DmaDesc), DmaDescriptorFault> {
        if self.remaining == 0 { return Err(DmaDescriptorFault::StepBudgetExceeded { budget: self.budget }); }
        self.remaining -= 1;
        bus.try_dma_desc(addr)
    }
}

impl SocBus {
    pub(super) fn complete_spi2_dma(&mut self) {
        if let Some((deadline, _)) = &self.spi2_scheduled {
            if self.cycles < *deadline { return; }
            let (_, completion) = self.spi2_scheduled.take().unwrap();
            self.commit_spi2_dma(completion);
            return;
        }
        if self.periph.spi2.dma_tx_pending.is_none() {
            return;
        }
        let channel = self.periph.gdma.out_channel_for(0);
        if channel.is_none() && !self.periph.gdma.out.iter().any(|c| c.peri_sel == 0 && c.desc != 0) {
            // A DMA command without a bound channel cannot progress. Abort it just
            // like a descriptor failure so firmware can issue a later CPU command.
            // A stopped channel with a descriptor remains bound and can be restarted.
            self.periph.spi2.fail_dma_tx();
            self.irq_dirty = true;
            return;
        }
        match self.spi2_dma_completion() {
            Ok(Some(completion)) => {
                if self.spi2_timing {
                    // Rough wire-time floor, assuming the normal 80 MHz SPI source.
                    // Payload is snapshotted at submission; no progressive SRAM reads yet.
                    let cycles = self.periph.spi2.wire_source_cycles() * (crate::periph::CPU_HZ / 80_000_000);
                    self.spi2_scheduled = Some((self.cycles.saturating_add(cycles), completion));
                    // Collection reads descriptor words through the bus, and one may be another
                    // device's register (mmio-s1 compares only SPI2/GDMA sources; review blocker).
                    self.irq_dirty = true;
                } else {
                    self.commit_spi2_dma(completion);
                }
            }
            Ok(None) => {}
            Err(fault) => {
                if let Some(channel) = channel {
                    self.fail_spi2_dma(channel, fault);
                }
            }
        }
    }

    fn commit_spi2_dma(&mut self, completion: Spi2DmaCompletion) {
        for (descriptor, control) in completion.descriptor_writebacks {
            if let Err(fault) = self.write32_unpriced(descriptor, control) {
                self.fail_spi2_dma(completion.channel, DmaDescriptorFault::Writeback { descriptor, fault });
                return;
            }
        }
        // Publish only this transfer's effects, not a stale copy of writable registers
        // or interrupt status that software may have cleared while it was on the wire.
        let channel = &mut self.periph.gdma.out[completion.channel];
        channel.desc = completion.desc;
        channel.buf_pos = completion.buf_pos;
        // Descriptor progress publishes as captured, but an OUT_LINK STOP during the
        // wire delay must keep the channel stopped: completion never resurrects it.
        channel.running = channel.running && completion.running;
        if let Some(descriptor) = completion.eof_desc { channel.eof_desc = descriptor; }
        channel.int_raw |= completion.raised_interrupts;
        self.periph.spi2.complete_dma_tx(&completion.payload);
        self.irq_dirty = true;
    }

    fn fail_spi2_dma(&mut self, channel: usize, fault: DmaDescriptorFault) {
        if self.periph.spi2.log { eprintln!("[spi2] DMA descriptor fault: {fault:?}"); }
        self.spi2_dma_fault = Some(fault);
        let gdma = &mut self.periph.gdma.out[channel];
        gdma.running = false;
        gdma.int_raw |= 1 << 2;                                         // OUT_DSCR_ERR
        self.periph.spi2.fail_dma_tx();
        self.irq_dirty = true;
    }

    pub(super) fn deliver_spi2_transfer(&mut self) {
        if self.periph.spi2.dma_tx_pending.is_some() {
            return;
        }
        let Some(transfer) = self.periph.spi2.take_transfer() else { return };
        // Chip select and command/data lines are GPIOs. The board must see their preceding edges
        // before it receives the transaction.
        if !self.periph.gpio.changes.is_empty() {
            let changes = std::mem::take(&mut self.periph.gpio.changes);
            if let Some(events) = &mut self.gpio_events {
                for &(pin, level) in &changes {
                    events.push((self.cycles, pin, level));
                }
            }
            self.board.gpio_output_at(self.cycles, &changes, self.periph.gpio.enable, self.periph.gpio.out);
        }
        let rx = if let Some(pins) = self.spi2_pins.take() {
            self.board.spi_transfer_pins(2, pins, &transfer.tx, transfer.rx_len)
        } else {
            self.board.spi_transfer(2, &transfer.tx, transfer.rx_len)
        };
        self.periph.spi2.finish_transfer(transfer, &rx);
    }

    /// Collect one GP-SPI2 data phase and its GDMA completion without partially committing a
    /// malformed descriptor chain.
    fn spi2_dma_completion(&mut self) -> Result<Option<Spi2DmaCompletion>, DmaDescriptorFault> {
        let Some(bits) = self.periph.spi2.dma_tx_pending else { return Ok(None) };
        let Some(channel_index) = self.periph.gdma.out_channel_for(0) else { return Ok(None) };
        let wanted = (bits as usize).div_ceil(8);
        let mut payload = Vec::with_capacity(wanted);
        let mut visited = HashSet::new();
        let mut channel = self.periph.gdma.out[channel_index];
        channel.int_raw = 0; // Accumulate only new completion events.
        let mut descriptor_writebacks = Vec::new();
        let mut walk = DescriptorWalk::new(SPI2_DMA_DESCRIPTOR_STEP_BUDGET);
        while payload.len() < wanted {
            let current = channel;
            if !current.running || current.desc == 0 {
                break;
            }
            let (control, descriptor) = walk.read(self, current.desc)?;
            if !visited.insert(current.desc) {
                return Err(DmaDescriptorFault::Cycle { descriptor: current.desc });
            }
            let length = descriptor.length;
            if !descriptor.owner_dma {
                return Err(DmaDescriptorFault::NotOwned { descriptor: current.desc });
            }
            let (buffer, next, eof) = (descriptor.buf, descriptor.next, descriptor.eof);
            let remaining = length.saturating_sub(current.buf_pos) as usize;
            if remaining != 0 {
                let count = remaining.min(wanted - payload.len());
                let address = buffer.wrapping_add(current.buf_pos);
                self.append_mapped_bytes(address, count, &mut payload).map_err(|(address, fault)| DmaDescriptorFault::BufferRead {
                    descriptor: current.desc,
                    address,
                    fault,
                })?;
                channel.buf_pos += remaining as u32;                     // GDMA drains the descriptor
            }
            if channel.conf0 & (1 << 2) != 0 {
                let writable = current.desc.checked_add(4).is_some_and(|end| {
                    self.lookup(current.desc).is_some_and(|entry| entry.writable != 0 && end <= entry.hi)
                });
                if !writable {
                    return Err(DmaDescriptorFault::Writeback { descriptor: current.desc, fault: Fault::Prohibited });
                }
                descriptor_writebacks.push((current.desc, control & !(1 << 31)));
            }
            channel.int_raw |= 1 << 0;
            if eof {
                channel.int_raw |= 1 << 1;
                channel.eof_desc = current.desc;
            }
            if next == 0 {
                channel.running = false;
                channel.desc = 0;
                channel.int_raw |= 1 << 3;
            } else {
                channel.desc = next;
                channel.buf_pos = 0;
            }
            if eof {
                break;
            }
            if payload.len() == wanted {
                break;
            }
        }
        if payload.len() != wanted {
            return Err(DmaDescriptorFault::PayloadTooShort { expected: wanted, actual: payload.len() });
        }
        Ok(Some(Spi2DmaCompletion {
            channel: channel_index, desc: channel.desc, buf_pos: channel.buf_pos,
            running: channel.running,
            eof_desc: (channel.int_raw & (1 << 1) != 0).then_some(channel.eof_desc),
            raised_interrupts: channel.int_raw, descriptor_writebacks, payload,
        }))
    }

    /// Append a memory range a mapping at a time. Peripheral and unmapped addresses use the
    /// ordinary byte read so their access semantics and first-fault bookkeeping stay unchanged.
    fn append_mapped_bytes(&mut self, mut address: u32, mut count: usize, out: &mut Vec<u8>) -> Result<(), (u32, Fault)> {
        while count != 0 {
            if !Self::is_periph(address) {
                if let Some(entry) = self.lookup(address) {
                    let take = count.min(entry.hi.wrapping_sub(address) as usize);
                    if take != 0 {
                        let offset = entry.off as usize + address.wrapping_sub(entry.lo) as usize;
                        if let Some(bytes) = self.buf(entry.src as u8).get(offset..offset + take) {
                            out.extend_from_slice(bytes);
                            address = address.wrapping_add(take as u32);
                            count -= take;
                            continue;
                        }
                    }
                }
            }
            match self.read8_unpriced(address) {
                Ok(byte) => out.push(byte),
                Err(fault) => return Err((address, fault)),
            }
            address = address.wrapping_add(1);
            count -= 1;
        }
        Ok(())
    }

    /// Move I2S TX data out of DMA descriptors at the sample rate.
    pub(super) fn dma_i2s_step(&mut self, cycles: u64) {
        self.dma_i2s_one(cycles, 0);
        self.dma_i2s_one(cycles, 1);
    }

    /// Move I2S TX data for controller `which` (0 = I2S0 on GDMA trigger 3, 1 = I2S1 on trigger 4).
    fn dma_i2s_one(&mut self, cycles: u64, which: usize) {
        let (frames, bpf, sample_bytes) = { let i2s = if which == 0 { &mut self.periph.i2s0 } else { &mut self.periph.i2s1 }; (i2s.frames_due(cycles), i2s.bytes_per_frame as usize, i2s.sample_bytes()) };
        if frames == 0 || bpf == 0 { return; }
        let Some(ch) = self.periph.gdma.out_channel_for(if which == 0 { 3 } else { 4 }) else { return };
        let mut need = frames as usize * bpf;
        let mut samples: Vec<i16> = Vec::new();
        let mut walk = DescriptorWalk::new(GDMA_DESCRIPTOR_STEP_BUDGET);
        'transfer: while need > 0 {
            let c = self.periph.gdma.out[ch];
            if !c.running || c.desc == 0 { break; }
            let Ok((_, d)) = walk.read(self, c.desc) else { self.fail_dma_out(ch); break };
            let remaining = d.length.saturating_sub(c.buf_pos) as usize;
            if remaining == 0 {
                // descriptor complete: hand back to software, raise EOF/DONE, advance
                let ch_ref = &mut self.periph.gdma.out[ch];
                if ch_ref.conf0 & (1 << 2) != 0 { let dw0 = self.read32_unpriced(d.addr).unwrap_or(0) & !(1 << 31); let _ = self.write32_unpriced(d.addr, dw0); }   // AUTO_WRBACK: owner -> cpu
                let ch_ref = &mut self.periph.gdma.out[ch];
                self.irq_dirty = true;
                ch_ref.int_raw |= 1 << 0;                                                     // OUT_DONE
                if d.eof { ch_ref.int_raw |= 1 << 1; ch_ref.eof_desc = d.addr; }             // OUT_EOF
                if d.next == 0 { ch_ref.running = false; ch_ref.desc = 0; ch_ref.int_raw |= 1 << 3; break; }   // OUT_TOTAL_EOF
                ch_ref.desc = d.next; ch_ref.buf_pos = 0;
                continue;
            }
            let take = remaining.min(need);
            let start = d.buf.wrapping_add(c.buf_pos);
            // Keep the first DMA channel, scaling its signed PCM sample to the 16-bit host sink.
            let mut i = 0usize;
            while i + bpf <= take {
                let addr = start.wrapping_add(i as u32);
                let sample = if sample_bytes == 1 {
                    self.read8_unpriced(addr).map(|v| (v as i8 as i16) << 8)
                } else {
                    self.read16_unpriced(addr.wrapping_add((sample_bytes - 2) as u32)).map(|v| v as i16)
                };
                let Ok(sample) = sample else { self.fail_dma_out(ch); break 'transfer };
                samples.push(sample);
                i += bpf;
            }
            self.periph.gdma.out[ch].buf_pos += take as u32;
            need -= take;
        }
        if !samples.is_empty() { let i2s = if which == 0 { &mut self.periph.i2s0 } else { &mut self.periph.i2s1 }; i2s.frames_out += samples.len() as u64; i2s.pcm.extend_from_slice(&samples); }
    }

    /// One DMA descriptor as the engines see it, with its first word, or the fault reading it.
    fn try_dma_desc(&mut self, addr: u32) -> Result<(u32, crate::periph::DmaDesc), DmaDescriptorFault> {
        let mut word = |offset, word| self.read32_unpriced(addr.wrapping_add(offset))
            .map_err(|fault| DmaDescriptorFault::Read { descriptor: addr, word, fault });
        let dw0 = word(0, DmaDescriptorWord::Control)?;
        let (buf, next) = (word(4, DmaDescriptorWord::Buffer)?, word(8, DmaDescriptorWord::Next)?);
        Ok((dw0, crate::periph::DmaDesc { addr, size: dw0 & 0xfff, length: (dw0 >> 12) & 0xfff, eof: dw0 & (1 << 30) != 0, owner_dma: dw0 & (1 << 31) != 0, buf, next }))
    }

    fn fail_dma_out(&mut self, ch: usize) {
        self.periph.gdma.out[ch].running = false;
        self.periph.gdma.out[ch].int_raw |= 1 << 2; // OUT_DSCR_ERR
        self.irq_dirty = true;
    }

    fn fail_dma_in(&mut self, ch: usize) {
        self.periph.gdma.inp[ch].running = false;
        self.periph.gdma.inp[ch].int_raw |= 1 << 3; // IN_DSCR_ERR
        self.irq_dirty = true;
    }

    /// Copy `n` guest bytes for the memory-to-memory engine, a word at a time where both ends are aligned.
    pub(super) fn dma_copy(&mut self, src: u32, dst: u32, n: u32) -> Result<(), M2mFault> {
        // EX170: whole mapping runs with one memmove while both ends are plain host-backed memory.
        // Anything else (MMIO, unmapped, read-only destination, forward-overlapping ranges) leaves
        // the rest to the word loop below, which continues from `i` with its own fault position.
        let mut i = self.dma_copy_runs(src, dst, n);
        if i == n { return Ok(()); }
        if (src | dst) & 3 == 0 {
            while i + 4 <= n {
                let v = self.read32_unpriced(src.wrapping_add(i)).map_err(|_| M2mFault::Source)?;
                self.write32_unpriced(dst.wrapping_add(i), v).map_err(|_| M2mFault::Destination)?;
                i += 4;
            }
        }
        while i < n {
            let v = self.read8_unpriced(src.wrapping_add(i)).map_err(|_| M2mFault::Source)?;
            self.write8_unpriced(dst.wrapping_add(i), v).map_err(|_| M2mFault::Destination)?;
            i += 1;
        }
        Ok(())
    }

    /// The plain per-word copy that `dma_copy` must stay indistinguishable from.
    #[cfg(test)]
    pub(super) fn dma_copy_reference(&mut self, src: u32, dst: u32, n: u32) -> Result<(), M2mFault> {
        let mut i = 0u32;
        if (src | dst) & 3 == 0 {
            while i + 4 <= n {
                let v = self.read32_unpriced(src.wrapping_add(i)).map_err(|_| M2mFault::Source)?;
                self.write32_unpriced(dst.wrapping_add(i), v).map_err(|_| M2mFault::Destination)?;
                i += 4;
            }
        }
        while i < n {
            let v = self.read8_unpriced(src.wrapping_add(i)).map_err(|_| M2mFault::Source)?;
            self.write8_unpriced(dst.wrapping_add(i), v).map_err(|_| M2mFault::Destination)?;
            i += 1;
        }
        Ok(())
    }

    /// Copy the leading part of an M2M span a mapping run at a time and return how many bytes were
    /// done. A run is taken only when the word loop would have produced the same bytes: both ends
    /// inside one mapping each, destination writable, and the host ranges either disjoint or with
    /// the destination below the source (where a forward copy equals memmove). Page versions get
    /// exactly the bumps the per-word (aligned) or per-byte writes would have made.
    fn dma_copy_runs(&mut self, src: u32, dst: u32, n: u32) -> u32 {
        let words = (src | dst) & 3 == 0;
        let mut i = 0u32;
        while i < n {
            let (s, d) = (src.wrapping_add(i), dst.wrapping_add(i));
            if Self::is_periph(s) || Self::is_periph(d) { break; }
            let Some(se) = self.lookup(s) else { break };
            let Some(de) = self.lookup(d) else { break };
            if de.writable == 0 { break; }
            let mut take = (n - i).min(se.hi - s).min(de.hi - d);
            // The word loop hands the unaligned tail to byte writes, which bump differently: keep
            // runs word-sized here and let the last 1..3 bytes go through the byte loop below.
            if words { take &= !3; }
            if take == 0 { break; }
            let (so, dof) = (se.off as usize + (s - se.lo) as usize, de.off as usize + (d - de.lo) as usize);
            let len = take as usize;
            if so + len > self.buf(se.src as u8).len() || dof + len > self.buf(de.src as u8).len() { break; }
            if se.src == de.src && dof > so && dof < so + len { break; }      // forward copy would re-read its own output
            let sp = self.buf(se.src as u8).as_ptr();
            let dp = self.buf_mut(de.src as u8).as_mut_ptr();
            // SAFETY: both ranges were bounds-checked against their buffers above; `copy` is memmove.
            unsafe { std::ptr::copy(sp.add(so), dp.add(dof), len); }
            // EX110: the per-word reference path bumps through `write*_access`, which skips a
            // destination no decoded code depends on; the run copy must skip exactly the same.
            if de.code != 0 { self.bump_run(de.vbase, (d - de.lo) as usize, len, words); }
            i += take;
        }
        i
    }

    /// The page-version bumps of `len` bytes written at `off` as aligned words or as single bytes:
    /// one per write on the write's page, and one on the previous page for each write that starts
    /// in the first three bytes of a page (see `bump`).
    fn bump_run(&mut self, vbase: u32, off: usize, len: usize, words: bool) {
        const SHIFT: usize = xtensa_lx7::bus::VPAGE_SHIFT as usize;
        const SIZE: usize = 1 << SHIFT;
        let (mut at, end) = (off, off + len);
        while at < end {
            let page_end = ((at >> SHIFT) + 1) << SHIFT;
            let stop = end.min(page_end);
            let p = vbase as usize + (at >> SHIFT);
            let in_page = at & (SIZE - 1);
            let (writes, early) = if words { ((stop - at) / 4, usize::from(in_page == 0)) }
                                  else { (stop - at, (emu_core::bus::PREV_PAGE_BYTES as usize).saturating_sub(in_page).min(stop - at)) };
            self.page_ver[p] = self.page_ver[p].wrapping_add(writes as u32);
            if early != 0 && p > 0 { self.page_ver[p - 1] = self.page_ver[p - 1].wrapping_add(early as u32); }
            self.touched(p.saturating_sub(1), p);
            at = stop;
        }
    }

    /// Hand a filled or EOF-ended IN descriptor back to the CPU (its length, owner, SUC_EOF) and
    /// move the channel to the next one. False when the write-back faults.
    fn dma_close_in(&mut self, r: &mut crate::periph::GdmaInCh, dw0: u32, next: u32, eof: bool) -> bool {
        let v = (dw0 & !(0xfff << 12) & !(3 << 30)) | (r.buf_pos << 12) | if eof { 1 << 30 } else { 0 };
        if self.write32_unpriced(r.desc, v).is_err() { return false; }
        r.int_raw |= 1 << 0;                                                  // IN_DONE
        if eof { r.int_raw |= 1 << 1; r.eof_desc = r.desc; }                  // IN_SUC_EOF
        r.desc = next; r.buf_pos = 0;
        true
    }

    /// Memory-to-memory GDMA: a channel pair whose IN side has MEM_TRANS_EN set copies its OUT
    /// descriptor chain into its IN chain — the transaction-based `esp_async_memcpy` of IDF v5.4,
    /// which starts both channels for each copy. The copy lands in one scheduling round, no
    /// transfer timing, and the descriptors are written back the way the engine does it: the IN
    /// side gets each buffer's length, owner back to the CPU and SUC_EOF where the OUT chain's
    /// EOF fell, so the driver's EOF callback finds its transaction through IN_SUC_EOF_DES_ADDR.
    ///
    /// A descriptor the CPU still owns parks that side with its DSCR_ERR raised until software
    /// hands it over, and the copy resumes where it stopped. A fault reading a descriptor,
    /// copying or writing back stops that side with DSCR_ERR and writes nothing more back; so
    /// does an unproductive walk longer than `GDMA_DESCRIPTOR_STEP_BUDGET`. Productive copies
    /// yield at the budget and resume from their descriptor positions on the next pump.
    /// Interrupt inputs are marked for re-evaluation only when a channel's state changed.
    pub(super) fn dma_m2m_step(&mut self) {
        use crate::periph::{GdmaInCh, GdmaOutCh};
        const OUT_DONE: u32 = 1 << 0;
        const OUT_EOF: u32 = 1 << 1;
        const OUT_DSCR_ERR: u32 = 1 << 2;
        const OUT_TOTAL_EOF: u32 = 1 << 3;
        const IN_DSCR_ERR: u32 = 1 << 3;
        const IN_DSCR_EMPTY: u32 = 1 << 4;
        const AUTO_WRBACK: u32 = 1 << 2;
        const MEM_TRANS_EN: u32 = 1 << 4;
        let out_state = |c: &GdmaOutCh| (c.int_raw, c.desc, c.buf_pos, c.running, c.eof_desc);
        let in_state = |c: &GdmaInCh| (c.int_raw, c.desc, c.buf_pos, c.running, c.eof_desc);
        for ch in 0..crate::periph::GDMA_CHANNELS {
            let (mut r, mut o) = (self.periph.gdma.inp[ch], self.periph.gdma.out[ch]);
            if !(r.running && o.running && r.conf0 & MEM_TRANS_EN != 0 && r.desc != 0 && o.desc != 0) { continue; }
            let (in_before, out_before) = (in_state(&r), out_state(&o));
            let mut copied = false;
            let mut walk = DescriptorWalk::new(GDMA_DESCRIPTOR_STEP_BUDGET);
            loop {
                // Resume a legal long copy on the next pump instead of faulting at the work budget.
                if walk.remaining < 2 {
                    if !copied { o.int_raw |= OUT_DSCR_ERR; o.running = false; }
                    break;
                }
                let Ok((out_dw0, od)) = walk.read(self, o.desc) else { o.int_raw |= OUT_DSCR_ERR; o.running = false; break };
                if !od.owner_dma { o.int_raw |= OUT_DSCR_ERR; break; }                 // parked until software hands it over
                let remaining = od.length.saturating_sub(o.buf_pos);
                if remaining > 0 || (od.eof && r.desc != 0) {
                    if r.desc == 0 { r.int_raw |= IN_DSCR_EMPTY; break; }
                    let Ok((in_dw0, id)) = walk.read(self, r.desc) else { r.int_raw |= IN_DSCR_ERR; r.running = false; break };
                    if !id.owner_dma || (remaining > 0 && id.size <= r.buf_pos) { r.int_raw |= IN_DSCR_ERR; break; }
                    let n = remaining.min(id.size.saturating_sub(r.buf_pos));
                    if n > 0 {
                        match self.dma_copy(od.buf.wrapping_add(o.buf_pos), id.buf.wrapping_add(r.buf_pos), n) {
                            Ok(()) => {}
                            Err(M2mFault::Source) => { o.int_raw |= OUT_DSCR_ERR; o.running = false; break; }
                            Err(M2mFault::Destination) => { r.int_raw |= IN_DSCR_ERR; r.running = false; break; }
                        }
                        copied = true;
                        o.buf_pos += n;
                        r.buf_pos += n;
                    }
                    let eof_now = o.buf_pos == od.length && od.eof;
                    if (r.buf_pos == id.size || eof_now) && !self.dma_close_in(&mut r, in_dw0, id.next, eof_now) {
                        r.int_raw |= IN_DSCR_ERR; r.running = false; break;
                    }
                    if o.buf_pos < od.length { continue; }                                 // the IN buffer filled first
                }
                if o.conf0 & AUTO_WRBACK != 0 && self.write32_unpriced(od.addr, out_dw0 & !(1 << 31)).is_err() {
                    o.int_raw |= OUT_DSCR_ERR; o.running = false; break;
                }
                o.int_raw |= OUT_DONE;
                if od.eof { o.int_raw |= OUT_EOF; o.eof_desc = od.addr; }
                if od.next == 0 { o.int_raw |= OUT_TOTAL_EOF; o.running = false; o.desc = 0; o.buf_pos = 0; break; }
                o.desc = od.next;
                o.buf_pos = 0;
            }
            if r.desc == 0 { r.running = false; }
            let changed = in_state(&r) != in_before || out_state(&o) != out_before;
            self.periph.gdma.inp[ch] = r;
            self.periph.gdma.out[ch] = o;
            self.irq_dirty |= changed;
        }
    }

    /// Camera engine: when a sensor frame is due, push it through the GDMA IN channel bound to CAM (trigger 5).
    pub(super) fn dma_cam_step(&mut self, cycles: u64) {
        if !self.periph.lcd_cam.frame_due(cycles) { return; }
        let Some(ch) = self.periph.gdma.in_channel_for(5) else { self.periph.lcd_cam.dropped += 1; return };
        let Some((_w, _h, frame)) = self.board.camera_frame() else { self.periph.lcd_cam.dropped += 1; return };
        if self.scatter_dma_in(ch, &frame).is_err() {
            self.fail_dma_in(ch);
            self.periph.lcd_cam.dropped += 1;
            return;
        }
        self.periph.lcd_cam.int_raw |= 1 << 2;                                                  // CAM_VSYNC_INT
        self.periph.lcd_cam.frames += 1;
        self.irq_dirty = true;
    }

    /// LCD RGB output: consume the GDMA out-channel bound to LCD (trigger 5) at the panel's pixel rate,
    /// assemble frames, publish each completed frame to the board and raise LCD_VSYNC.
    /// LCD RGB output. The LCD engine's async FIFO (16 words) is kept full ahead of the pixel clock,
    /// so a DMA link restart mid-frame (the RGB driver skips LCD_FIFO_PRESERVE_SIZE_PX pixels then)
    /// behaves as on silicon. Frames are published to the board and raise LCD_VSYNC.
    pub(super) fn dma_lcd_step(&mut self, cycles: u64) {
        if !self.periph.lcd_cam.lcd_running() { return; }
        let (ha, va, bpp, frame_cycles) = self.periph.lcd_cam.lcd_geometry();
        let frame_bytes = (ha * va * bpp) as usize;
        if frame_bytes == 0 { return; }
        const FIFO_BYTES: usize = 17 * 2;
        self.periph.lcd_cam.lcd_acc += cycles;
        let due = (self.periph.lcd_cam.lcd_acc as u128 * frame_bytes as u128 / frame_cycles as u128) as usize;
        if due < 512 { return; }
        self.periph.lcd_cam.lcd_acc = 0;
        let log = self.periph.lcd_cam.lcd_log;
        // 1) top the FIFO up from DMA so that it holds `due` + lookahead bytes
        if let Some(ch) = self.periph.gdma.out_channel_for(5) {
            let mut want = (due + FIFO_BYTES).saturating_sub(self.periph.lcd_cam.lcd_fifo.len());
            let mut walk = DescriptorWalk::new(GDMA_DESCRIPTOR_STEP_BUDGET);
            while want > 0 {
                let c = self.periph.gdma.out[ch];
                if !c.running || c.desc == 0 { break; }
                let Ok((_, d)) = walk.read(self, c.desc) else { self.fail_dma_out(ch); break };
                let (length, eof, buf, next) = (d.length, d.eof, d.buf, d.next);
                let remaining = length.saturating_sub(c.buf_pos) as usize;
                if remaining == 0 {
                    if log { eprintln!("[lcd] desc {:#010x} done (buf {:#010x} len {} eof {}) -> next {:#010x}", c.desc, buf, length, eof, next); }
                    let ch_ref = &mut self.periph.gdma.out[ch];
                    self.irq_dirty = true;
                    ch_ref.int_raw |= 1 << 0;
                    if eof { ch_ref.int_raw |= 1 << 1; ch_ref.eof_desc = c.desc; }
                    if next == 0 { ch_ref.running = false; ch_ref.desc = 0; ch_ref.int_raw |= 1 << 3; break; }
                    ch_ref.desc = next; ch_ref.buf_pos = 0;
                    continue;
                }
                let take = remaining.min(want);
                let mut bytes = Vec::with_capacity(take);
                if self.append_mapped_bytes(buf.wrapping_add(c.buf_pos), take, &mut bytes).is_err() {
                    self.fail_dma_out(ch); break;
                }
                self.periph.lcd_cam.lcd_fifo.extend(bytes);
                self.periph.gdma.out[ch].buf_pos += take as u32;
                want -= take;
            }
        }
        // 2) the panel consumes `due` bytes from the FIFO
        let n = due.min(self.periph.lcd_cam.lcd_fifo.len());
        for _ in 0..n { let b = self.periph.lcd_cam.lcd_fifo.pop_front().unwrap(); self.periph.lcd_cam.lcd_line.push(b); }
        while self.periph.lcd_cam.lcd_line.len() >= frame_bytes {
            let frame = std::mem::take(&mut self.periph.lcd_cam.lcd_line);
            self.board.lcd_frame(ha, va, &frame[..frame_bytes]);
            if frame.len() > frame_bytes { self.periph.lcd_cam.lcd_line.extend_from_slice(&frame[frame_bytes..]); }
            self.periph.lcd_cam.lcd_frames += 1;
            self.periph.lcd_cam.int_raw |= 1 << 0;                                    // LCD_VSYNC_INT
            self.irq_dirty = true;
        }
    }

    /// Gather a finite crypto transaction. Descriptor visits bound both runtime and allocation
    /// (4096 * 4095 bytes maximum); a visited set rejects cycles independently of owner checking.
    fn gather_dma_out(&mut self, ch: usize, limit: usize) -> Result<Vec<u8>, DmaDescriptorFault> {
        let mut input = Vec::new();
        let mut desc = self.periph.gdma.out[ch].desc;
        let mut visited = HashSet::new();
        let mut walk = DescriptorWalk::new(GDMA_DESCRIPTOR_STEP_BUDGET);
        while desc != 0 && input.len() < limit {
            if !visited.insert(desc) { return Err(DmaDescriptorFault::Cycle { descriptor: desc }); }
            let (control, d) = walk.read(self, desc)?;
            if self.periph.gdma.out[ch].conf1 & (1 << 12) != 0 && !d.owner_dma { return Err(DmaDescriptorFault::NotOwned { descriptor: desc }); }
            let take = (d.length as usize).min(limit - input.len());
            self.append_mapped_bytes(d.buf, take, &mut input).map_err(|(address, fault)|
                DmaDescriptorFault::BufferRead { descriptor: desc, address, fault })?;
            self.write32_unpriced(desc, control & !(1 << 31)).map_err(|fault|
                DmaDescriptorFault::Writeback { descriptor: desc, fault })?;
            self.periph.gdma.out[ch].int_raw |= 1 << 0;
            if d.eof {
                self.periph.gdma.out[ch].int_raw |= 1 << 1;
                self.periph.gdma.out[ch].eof_desc = desc;
                break;
            }
            desc = d.next;
        }
        Ok(input)
    }

    /// Shared camera/crypto receive path. A malformed destination never reports successful EOF.
    fn scatter_dma_in(&mut self, ch: usize, data: &[u8]) -> Result<(), ()> {
        let mut pos = 0usize;
        let mut walk = DescriptorWalk::new(GDMA_DESCRIPTOR_STEP_BUDGET);
        while pos < data.len() {
            let mut r = self.periph.gdma.inp[ch];
            if r.desc == 0 { return Err(()); }
            let (control, d) = walk.read(self, r.desc).map_err(|_| ())?;
            if (r.conf1 & (1 << 12) != 0 && !d.owner_dma) || d.size == 0 { return Err(()); }
            let n = (d.size as usize).min(data.len() - pos);
            let end = d.buf.checked_add(n as u32).ok_or(())?;
            // DMA buffers are memory, never MMIO. The word-copy fast path must preserve the
            // byte bus's rejection of peripheral destinations instead of triggering devices.
            if Self::is_periph(d.buf) || Self::is_periph(end - 1) { return Err(()); }
            let mut i = 0;
            while i + 4 <= n {
                let word = u32::from_le_bytes(data[pos + i..pos + i + 4].try_into().unwrap());
                self.write32_unpriced(d.buf.wrapping_add(i as u32), word).map_err(|_| ())?;
                i += 4;
            }
            while i < n {
                self.write8_unpriced(d.buf.wrapping_add(i as u32), data[pos + i]).map_err(|_| ())?;
                i += 1;
            }
            pos += n;
            r.buf_pos = n as u32;
            if !self.dma_close_in(&mut r, control, d.next, pos == data.len()) { return Err(()); }
            if r.desc == 0 { r.running = false; }
            self.periph.gdma.inp[ch] = r;
        }
        Ok(())
    }

    /// Feed SHA from its GDMA out channel (peripheral 7).
    pub(super) fn sha_dma_step(&mut self) {
        self.periph.sha.dma_pending = false;
        let want = (self.periph.sha.block_num as usize).saturating_mul(self.periph.sha.block_bytes());
        let Some(ch) = self.periph.gdma.out_channel_for(7) else { self.periph.sha.busy = false; return };
        // Direct users can set block_num as well as firmware. Reject oversized work before allocating.
        if want > GDMA_DESCRIPTOR_STEP_BUDGET * 4095 {
            self.fail_dma_out(ch); self.periph.sha.busy = false; return;
        }
        let Ok(input) = self.gather_dma_out(ch, want) else {
            self.fail_dma_out(ch); self.periph.sha.busy = false; return;
        };
        if input.len() != want {
            self.fail_dma_out(ch); self.periph.sha.busy = false; return;
        }
        let bs = self.periph.sha.block_bytes();
        let mut first = self.periph.sha.dma_first;
        for block in input.chunks(bs) {
            self.periph.sha.hash_block(block, first);
            first = false;
        }
        self.periph.sha.busy = false;
        self.irq_dirty = true;
    }

    pub(super) fn aes_dma_step(&mut self) {
        self.periph.aes.dma_pending = false;
        let (Some(out_ch), Some(in_ch)) = (self.periph.gdma.out_channel_for(6), self.periph.gdma.in_channel_for(6)) else {
            self.periph.aes.state = 2; self.periph.aes.int_raw |= 1; self.irq_dirty = true; return;
        };
        let Ok(input) = self.gather_dma_out(out_ch, usize::MAX) else {
            self.fail_dma_out(out_ch); self.periph.aes.state = 0; return;
        };
        if self.debug.has("aes") {
            eprintln!("[aes] dma block_mode={} num_blocks={} mode={} bytes={}", self.periph.aes.block_mode, self.periph.aes.num_blocks, self.periph.aes.mode, input.len());
        }
        let output = self.periph.aes.transform_blocks(&input);
        if self.scatter_dma_in(in_ch, &output).is_err() {
            self.fail_dma_in(in_ch); self.periph.aes.state = 0; return;
        }
        self.periph.aes.state = 2;                                              // DONE
        self.periph.aes.int_raw |= 1;
        self.irq_dirty = true;
    }

}
