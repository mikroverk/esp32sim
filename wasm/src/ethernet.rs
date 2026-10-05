//! Host Ethernet transport, independent of the built-in virtual network.
use super::{bytes, Emu};

/// Enable (nonzero) or disable (zero) the host relay. Returns 0 on success, 1 on error.
/// Configure Wi-Fi with `esp32sim_wifi` before boot, then select relay mode.
/// # Safety
/// `e` must be a live emulator with exclusive access for this call.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_ethernet_relay(e: *mut Emu, enabled: u32) -> u32 {
    let e = unsafe { &mut *e };
    e.m.bus().set_ethernet_relay(enabled != 0).is_err() as u32
}

/// Inject one Ethernet frame without FCS. Returns 0 on success, 1 on rejection.
/// # Safety
/// `e` must be live and exclusively accessible; `ptr` must be readable for `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_ethernet_receive(e: *mut Emu, ptr: *const u8, len: usize) -> u32 {
    if !(14..=1518).contains(&len) || ptr.is_null() {
        return 1;
    }
    let e = unsafe { &mut *e };
    e.m.bus()
        .receive_ethernet_frame(unsafe { bytes(ptr, len) })
        .is_err() as u32
}

/// Drain sent frames and return their count. Replaces the previous drained batch.
/// # Safety
/// `e` must be a live emulator with exclusive access for this call.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_ethernet_take(e: *mut Emu) -> usize {
    let e = unsafe { &mut *e };
    e.ethernet_out = e.m.bus().take_ethernet_frames();
    e.ethernet_out.len()
}

/// Frame bytes, valid until the next `ethernet_take` or emulator deletion. Null for a bad index.
/// # Safety
/// `e` must be live and readable for this call. The returned bytes must not be modified.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_ethernet_ptr(e: *const Emu, index: usize) -> *const u8 {
    unsafe { &*e }
        .ethernet_out
        .get(index)
        .map_or(std::ptr::null(), |f| f.as_ptr())
}

/// Length of a drained frame, or zero for a bad index.
/// # Safety
/// `e` must be live and readable for this call.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_ethernet_len(e: *const Emu, index: usize) -> usize {
    unsafe { &*e }.ethernet_out.get(index).map_or(0, Vec::len)
}
