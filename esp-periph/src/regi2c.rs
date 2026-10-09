//! Inferred analog I2C control words shared by chip adapters; no hardware validation.
//! Bits 23:16 carry data, bit 24 selects a write and bit 25 is busy.
//! Inlined, so each chip crate keeps its own map code inside its I2C master as before.
#[derive(Default)]
pub struct Regi2c(std::collections::HashMap<u32, u8>);
impl Regi2c {
    /// `default(key)` supplies the value of a register never written.
    #[inline(always)]
    pub fn read(&self, host: u32, control: u32, default: impl FnOnce(u32) -> &'static u8) -> u32 {
        if control & (1 << 24) != 0 { return control & !(1 << 25); }
        let key = (host << 16) | (control & 0xffff);
        let data = *self.0.get(&key).unwrap_or(default(key)) as u32;
        (control & !(0xff << 16) & !(1 << 25)) | (data << 16)
    }
    #[inline(always)]
    pub fn write(&mut self, host: u32, control: u32) {
        if control & (1 << 24) != 0 {
            self.0.insert((host << 16) | (control & 0xffff), (control >> 16) as u8);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controls_preserve_hosts_defaults_and_read_write_direction() {
        let mut r = Regi2c::default();
        r.write(4, (1 << 24) | (0xa5 << 16) | 0x0762);
        assert_eq!(r.read(0, 0x0762 | (1 << 25), |_| &0x5b), 0x005b0762);
        assert_eq!(r.read(4, 0x0762 | (1 << 25), |_| &0), 0x00a50762);
        r.write(4, 0x00ff0762);
        assert_eq!(r.read(4, 0x0762, |_| &0), 0x00a50762);
        assert_eq!(r.read(4, 0x03ff0762, |_| &0), 0x01ff0762);
    }
}
