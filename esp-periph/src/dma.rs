//! Checked DMA descriptors shared by controller-local and GDMA engines.
use emu_core::bus::Fault;

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

/// Defines `DescriptorWalk` and `try_dma_desc` on one chip bus, which reads descriptor words with
/// `$read(&mut self, addr) -> Result<u32, Fault>`. Engines that forbid MMIO return a fault there.
/// A macro so each bus gets its own monomorphic reader, the code S3's DMA engines always ran.
#[macro_export]
macro_rules! dma_descriptor_reader {
    ($bus:ty, $read:ident) => {
        /// Bound work even when guest descriptors make no progress. Streaming rings are valid: the
        /// bound applies to one pump call, not the lifetime of an I2S or LCD channel.
        struct DescriptorWalk { remaining: usize, budget: usize }
        impl DescriptorWalk {
            fn new(budget: usize) -> Self { Self { remaining: budget, budget } }
            fn read(&mut self, bus: &mut $bus, addr: u32) -> Result<(u32, $crate::DmaDesc), $crate::dma::DmaDescriptorFault> {
                if self.remaining == 0 { return Err($crate::dma::DmaDescriptorFault::StepBudgetExceeded { budget: self.budget }); }
                self.remaining -= 1;
                bus.try_dma_desc(addr)
            }
        }
        impl $bus {
            /// One DMA descriptor as the engines see it, with its first word, or the fault reading it.
            fn try_dma_desc(&mut self, addr: u32) -> Result<(u32, $crate::DmaDesc), $crate::dma::DmaDescriptorFault> {
                let mut word = |offset, word| self.$read(addr.wrapping_add(offset))
                    .map_err(|fault| $crate::dma::DmaDescriptorFault::Read { descriptor: addr, word, fault });
                let dw0 = word(0, $crate::dma::DmaDescriptorWord::Control)?;
                let (buf, next) = (word(4, $crate::dma::DmaDescriptorWord::Buffer)?, word(8, $crate::dma::DmaDescriptorWord::Next)?);
                Ok((dw0, $crate::DmaDesc { addr, size: dw0 & 0xfff, length: (dw0 >> 12) & 0xfff, eof: dw0 & (1 << 30) != 0, owner_dma: dw0 & (1 << 31) != 0, buf, next }))
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::bus::Fault;

    /// Descriptor words at 4 (owned, one byte, next = itself); everything else faults.
    struct Mem { reads: u32 }
    impl Mem {
        fn word(&mut self, addr: u32) -> Result<u32, Fault> {
            self.reads += 1;
            match addr { 4 => Ok(1 | (1 << 12) | (1 << 31)), 8 => Ok(0x100), 12 => Ok(4), _ => Err(Fault::Unmapped) }
        }
    }
    crate::dma_descriptor_reader!(Mem, word);

    #[test]
    fn descriptor_reader_decodes_words_bounds_visits_and_names_faulting_word() {
        let mut mem = Mem { reads: 0 };
        let mut walk = DescriptorWalk::new(1);
        let (control, d) = walk.read(&mut mem, 4).unwrap();
        assert_eq!((control >> 31, d.size, d.length, d.owner_dma, d.buf, d.next), (1, 1, 1, true, 0x100, 4));
        assert_eq!(walk.read(&mut mem, 4).err(), Some(DmaDescriptorFault::StepBudgetExceeded { budget: 1 }));
        assert_eq!(mem.reads, 3);
        assert_eq!(mem.try_dma_desc(8).err(), Some(DmaDescriptorFault::Read { descriptor: 8, word: DmaDescriptorWord::Next, fault: Fault::Unmapped }));
        assert_eq!(mem.try_dma_desc(0).err(), Some(DmaDescriptorFault::Read { descriptor: 0, word: DmaDescriptorWord::Control, fault: Fault::Unmapped }));
    }
}
