use crate::device::{Device, WriteEffect};
use crate::regram::RegRam;

pub const EFUSE_STATE_WORDS: usize = 84;
pub const EFUSE_STATE_BYTES: usize = EFUSE_STATE_WORDS * 4;
const BLOCK_WORDS: [usize; 11] = [6, 6, 8, 8, 8, 8, 8, 8, 8, 8, 8];
const BLOCK_BASE: [usize; 11] = [0, 6, 12, 20, 28, 36, 44, 52, 60, 68, 76];
const SHADOW_BASE: [u32; 11] = [0x2c, 0x44, 0x5c, 0x7c, 0x9c, 0xbc, 0xdc, 0xfc, 0x11c, 0x13c, 0x15c];

/// ESP32-S3/C3/C6 eFuse controller. `physical` is the one-time-programmable array; `ram` also
/// holds transient program registers and controller state. Read shadows are rebuilt from physical
/// data.
pub struct Efuse { pub ram: RegRam, physical: [u32; EFUSE_STATE_WORDS], dirty: bool }

impl Efuse {
    pub fn new(mac: [u8; 6]) -> Self {
        let mut e = Efuse { ram: RegRam::new(), physical: [0; EFUSE_STATE_WORDS], dirty: false };
        e.physical[BLOCK_BASE[1]] = u32::from_be_bytes([mac[2], mac[3], mac[4], mac[5]]);
        e.physical[BLOCK_BASE[1] + 1] = ((mac[0] as u32) << 8 | mac[1] as u32) | (2 << 18);
        e.physical[BLOCK_BASE[2] + 4] = 1;
        e.rebuild_shadows();
        e.ram.write(0x1cc, 0x8c);
        e
    }

    pub fn read(&self, off: u32) -> u32 { self.ram.read(off) }
    pub fn write(&mut self, off: u32, v: u32) {
        match off {
            0x1d4 => { if v & 2 != 0 { self.program((v >> 2) & 0xf); } }
            _ => self.ram.write(off, v),
        }
    }

    pub fn state_bytes(&self) -> Vec<u8> { self.physical.iter().flat_map(|word| word.to_le_bytes()).collect() }

    pub fn load_state(&mut self, data: &[u8]) -> Result<(), String> {
        if data.len() != EFUSE_STATE_BYTES { return Err(format!("eFuse state is {} bytes, expected {}", data.len(), EFUSE_STATE_BYTES)); }
        let (words, remainder) = data.as_chunks::<4>();
        debug_assert!(remainder.is_empty());
        for (word, bytes) in self.physical.iter_mut().zip(words) { *word = u32::from_le_bytes(*bytes); }
        self.rebuild_shadows(); self.dirty = false; Ok(())
    }

    /// Import a read-shadow register dump as initial physical state.
    pub fn import_shadow(&mut self, off: u32, value: u32) -> bool {
        for block in 0..11 {
            let start = SHADOW_BASE[block]; let end = start + (BLOCK_WORDS[block] * 4) as u32;
            if (start..end).contains(&off) && (off - start).is_multiple_of(4) {
                self.physical[BLOCK_BASE[block] + ((off - start) / 4) as usize] = value;
                self.rebuild_shadows(); return true;
            }
        }
        false
    }

    pub fn take_dirty(&mut self) -> bool { std::mem::take(&mut self.dirty) }

    /// A chip reset retains physical fuses but clears the program staging window and command.
    pub fn reset_controller(&mut self) {
        for offset in (0..0x20).step_by(4) { self.ram.write(offset, 0); }
        self.ram.write(0x1d4, 0);
        self.dirty = false;
        self.rebuild_shadows();
    }

    fn block_write_protected(&self, block: usize) -> bool {
        if block == 0 { return false; }
        let bit = match block { 1 => 20, 2 => 21, 3 => 22, 4..=9 => 19 + block, 10 => 29, _ => return true };
        self.physical[0] & (1 << bit) != 0
    }

    fn block_read_protected(&self, block: usize) -> bool {
        (4..=10).contains(&block) && self.physical[1] & (1 << (block - 4)) != 0
    }

    fn program(&mut self, block: u32) {
        let block = block as usize;
        if block >= BLOCK_WORDS.len() || self.block_write_protected(block) { return; }
        let mut changed = false;
        for word in 0..BLOCK_WORDS[block] {
            let staged = self.ram.read((word * 4) as u32);
            let idx = BLOCK_BASE[block] + word;
            let programmed = self.physical[idx] | staged;
            changed |= programmed != self.physical[idx];
            self.physical[idx] = programmed;
        }
        if changed { self.dirty = true; self.rebuild_shadows(); }
    }

    fn rebuild_shadows(&mut self) {
        for block in 0..11 {
            let hidden = self.block_read_protected(block);
            for word in 0..BLOCK_WORDS[block] {
                self.ram.write(SHADOW_BASE[block] + (word * 4) as u32, if hidden { 0 } else { self.physical[BLOCK_BASE[block] + word] });
            }
        }
    }
}

impl Device for Efuse {
    fn read(&mut self, off: u32) -> u32 { Efuse::read(self, off) }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect { Efuse::write(self, off, v); WriteEffect::NONE }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn program_command_burns_staged_words_into_the_selected_block() {
        let mut efuse = Efuse::new([0; 6]); efuse.write(0, 3); efuse.write(4, 0x1122_3344); efuse.write(0x1d4, (4 << 2) | 2);
        assert_eq!(efuse.read(0x9c), 3); assert_eq!(efuse.read(0xa0), 0x1122_3344); assert!(efuse.take_dirty());
    }
    #[test]
    fn programming_can_only_set_additional_bits_and_round_trips() {
        let mut efuse = Efuse::new([0; 6]); efuse.write(0, 1); efuse.write(0x1d4, (4 << 2) | 2); efuse.write(0, 2); efuse.write(0x1d4, (4 << 2) | 2);
        assert_eq!(efuse.read(0x9c), 3); let bytes = efuse.state_bytes(); let mut loaded = Efuse::new([0; 6]); loaded.load_state(&bytes).unwrap(); assert_eq!(loaded.read(0x9c), 3);
    }
    #[test]
    fn block_write_and_read_protection_are_enforced() {
        let mut efuse = Efuse::new([0; 6]); efuse.write(0, 1 << 23); efuse.write(0x1d4, 2);
        efuse.write(0, 0x55); efuse.write(0x1d4, (4 << 2) | 2); assert_eq!(efuse.read(0x9c), 0);
        let mut state = efuse.state_bytes(); state[4] |= 1; efuse.load_state(&state).unwrap(); assert_eq!(efuse.read(0x9c), 0);
    }
    #[test]
    fn serialized_block_offsets_match_espressif_qemu() {
        let mut data = Vec::with_capacity(EFUSE_STATE_BYTES);
        for word in 0..EFUSE_STATE_WORDS as u32 { data.extend_from_slice(&(if word == 1 { 0 } else { word }).to_le_bytes()); }
        let mut efuse = Efuse::new([0; 6]); efuse.load_state(&data).unwrap();
        for block in 0..11 {
            assert_eq!(efuse.read(SHADOW_BASE[block]), BLOCK_BASE[block] as u32, "block {block} start");
            assert_eq!(efuse.read(SHADOW_BASE[block] + (BLOCK_WORDS[block] as u32 - 1) * 4), (BLOCK_BASE[block] + BLOCK_WORDS[block] - 1) as u32, "block {block} end");
        }
        assert_eq!(efuse.state_bytes(), data);
    }
}
