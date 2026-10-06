//! Passive observation of the C3 full link controller.
use super::{Emu, MachineKind};

/// Enable the full controller before boot. Returns 1 for other chips or after boot.
/// # Safety
/// `e` must be a live emulator with exclusive access for this call.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_ble_full(e: *mut Emu) -> u32 {
    let e = unsafe { &mut *e };
    if e.booted { return 1 }
    let MachineKind::C3(m) = &mut e.m else { return 1 };
    m.bus.periph.ble_lc.enable();
    m.bus.periph.refresh_work();
    esp_periph::Dispatch::refresh_optional(&mut m.bus.periph, 0x31);
    0
}

/// Take one UTF-8 observation, returning its byte length (zero when empty).
/// # Safety
/// `e` must be a live emulator with exclusive access for this call.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_ble_take(e: *mut Emu) -> usize {
    let e = unsafe { &mut *e };
    e.ble_out = match &mut e.m {
        MachineKind::C3(m) => m.bus.periph.ble_lc.take_observation().unwrap_or_default(),
        _ => String::new(),
    };
    e.ble_out.len()
}

/// Bytes from `ble_take`, valid until the next take or emulator deletion.
/// # Safety
/// `e` must be live and readable. The returned bytes must not be modified.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_ble_ptr(e: *const Emu) -> *const u8 {
    unsafe { &*e }.ble_out.as_ptr()
}
