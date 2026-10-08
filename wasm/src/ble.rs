//! Passive observation of the C3 full link controller.
use super::{log, Emu, MachineKind};
use esp_soc::SocBus;

/// Enable the full controller before boot. Returns 1 for other chips or after boot.
/// # Safety
/// `e` must be a live emulator with exclusive access for this call.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_ble_full(e: *mut Emu) -> u32 {
    let e = unsafe { &mut *e };
    if e.booted { log("[emu] full BLE must be enabled before boot"); return 1 }
    let MachineKind::C3(m) = &mut e.m else { log("[emu] full BLE requires C3"); return 1 };
    if let Err(reason) = m.bus.enable_ble_full(true) { log(&format!("[emu] {reason}")); return 1 }

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


/// Toggle the virtual active scanner. Returns 1 unless C3 full mode is enabled.
/// # Safety
/// `e` must be a live emulator with exclusive access for this call.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_ble_scan(e: *mut Emu, enabled: u32) -> u32 {
    let e = unsafe { &mut *e };
    let MachineKind::C3(m) = &mut e.m else { return 1 };
    if !m.bus.periph.ble_lc.enabled() { return 1 }
    m.bus.periph.ble_lc.scan(enabled != 0);
    0
}


/// Send connect or read-uuid SERVICE CHARACTERISTIC to C3 full mode.
/// Returns 0 on success, 1 for invalid commands, other chips or disabled full mode.
/// # Safety
/// `e` must be live and exclusive; `ptr` must be readable for `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_ble_command(e: *mut Emu, ptr: *const u8, len: usize) -> u32 {
    let e = unsafe { &mut *e };
    let command = unsafe { super::text(ptr, len) };
    let MachineKind::C3(m) = &mut e.m else { return 1 };
    u32::from(m.bus.periph.ble_lc.command(command).is_err())
}
