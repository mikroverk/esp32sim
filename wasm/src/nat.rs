//! The station's NAT carried by the page: the guest's sockets past the gateway become records the
//! page takes after every run (`esp32sim_nat_take`), and the page answers with events
//! (`esp32sim_nat_event`) from whatever network it has (a WebSocket relay, say). The NAT and its
//! TCP stay here; the page sees connects, data, shutdowns and closes per flow
//! (`esp_soc::nat::queue`).
use super::{bytes, log, Emu};
use esp_soc::nat::queue::{op, OUT_MAX, RECORD_HEADER};
use esp_soc::nat::{HostEvent, Nat, QueueTransport};

/// Carry the network behind the access point (`esp32sim_wifi`, call it first and again after
/// each new one) through the page. `flags` bit 0: on (TCP and UDP past the gateway become records);
/// without it, back to no NAT. Bit 1: the page resolves the guest's A / AAAA queries (`RESOLVE`
/// records); without it DNS datagrams go to the page as UDP to 10.0.2.3:53. Bit 2: SNTP is
/// answered here, from the network clock, instead of going to the page. A chip reset aborts the
/// open flows (`TCP_CLOSE` records with abort), and so does a second call, which starts a new
/// queue: the next take hands over the old queue's records, those closes last, before the new
/// one's. Take between calls: past 256 KiB of such records waiting, only the closes are kept.
/// Returns 0, or 1 without a network (the reason goes to `host_log`).
/// # Safety
/// `e` must be a live emulator with exclusive access for this call.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_nat_relay(e: *mut Emu, flags: u32) -> u32 {
    let e = unsafe { &mut *e };
    let (nat, handle) = if flags & 1 == 0 { (None, None) } else {
        let (transport, handle) = QueueTransport::with_dns(flags & 2 != 0);
        (Some(Nat::with_transport(false, Box::new(transport))), Some(handle))
    };
    if let Err(reason) = e.m.attach_nat(nat, flags & 1 != 0 && flags & 4 != 0) { log(&format!("[emu] nat relay: {reason}")); return 1 }
    keep_left(e);
    e.nat = handle;
    0
}

/// Keep the replaced relay's untaken records, the closes of its flows among them, for the next take.
/// A page that switches again and again without taking does not grow them without bound: from the
/// first record that would take the waiting ones past `OUT_MAX` bytes, only that queue's closes
/// are kept, since the page may hold those flows open.
fn keep_left(e: &mut Emu) {
    let Some(old) = e.nat.take() else { return };
    let mut left = Vec::new();
    old.take_ops(&mut left);
    let (mut at, mut fits) = (0, true);
    while at + RECORD_HEADER <= left.len() {
        let len = u32::from_le_bytes([left[at + 8], left[at + 9], left[at + 10], left[at + 11]]) as usize;
        let end = at + (RECORD_HEADER + len).next_multiple_of(4);
        fits = fits && e.nat_left.len() + (end - at) <= OUT_MAX;
        if fits || matches!(left[at], op::TCP_CLOSE | op::UDP_CLOSE) { e.nat_left.extend_from_slice(&left[at..end]); }
        at = end;
    }
}

/// End the relay before its network goes (`esp32sim_wifi` puts up a new one): its flows are
/// aborted and the next take hands over their closes.
pub(crate) fn end_relay(e: &mut Emu) {
    if e.nat.is_none() { return; }
    let _ = e.m.attach_nat(None, false);
    keep_left(e);
}

/// Take the records the guest's sockets queued since the last take, returning their byte length
/// (zero when none, or without `esp32sim_nat_relay`). The bytes are at `esp32sim_nat_ptr`, valid
/// until the next take or emulator deletion: records of `u8 op, u8 flags, u16 port, u32 id, u32 len`
/// and `len` bytes, little-endian, each padded to a multiple of 4 bytes. op 1 TCP_CONNECT (port,
/// data = IPv4[4]), 2 TCP_DATA, 3 TCP_SHUTDOWN, 4 TCP_CLOSE (flags bit 0: abort), 5 UDP_OPEN (port,
/// IPv4[4]), 6 UDP_DATA, 7 UDP_CLOSE, 8 RESOLVE (id = query, port = qtype 1 A / 28 AAAA, data = the
/// name in UTF-8).
/// # Safety
/// `e` must be a live emulator with exclusive access for this call.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_nat_take(e: *mut Emu) -> usize {
    let e = unsafe { &mut *e };
    match &e.nat {
        Some(h) => { h.take_ops(&mut e.nat_out); }
        None => e.nat_out.clear(),
    }
    if !e.nat_left.is_empty() {
        let mut out = std::mem::take(&mut e.nat_left);
        out.extend_from_slice(&e.nat_out);
        e.nat_out = out;
    }
    e.nat_out.len()
}

/// Bytes from `esp32sim_nat_take`, valid until the next take or emulator deletion.
/// # Safety
/// `e` must be live and readable. The returned bytes must not be modified.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_nat_ptr(e: *const Emu) -> *const u8 {
    unsafe { &*e }.nat_out.as_ptr()
}

/// Deliver one event from the page. `kind` 1 CONNECTED (`arg` = bytes the page takes before its
/// first TX_CREDIT), 2 FAILED (`arg` 1 refused, 2 unreachable or timed out, 3 policy, 4 offline:
/// the guest gets RST; an open flow is reset), 3 TCP_DATA (returns the bytes taken: the rest is
/// offered again later; all of them for a flow the guest has forgotten), 4 TCP_EOF, 5 TCP_RESET
/// (after the bytes already taken), 6 TX_CREDIT (`arg` bytes), 7 UDP_DATA (1 taken, 0 dropped:
/// 16 wait per flow), 8 RESOLVED (`id` = query, `arg` = TTL, data = n × IPv4[4], none for an empty
/// answer), 9 UNRESOLVED (`arg` = rcode 2 SERVFAIL or 3 NXDOMAIN). The others return 1 when
/// applied, 0 for a flow or query not waiting for it, an unknown kind or no relay. No TCP_CLOSE
/// record follows FAILED or TCP_RESET.
/// # Safety
/// `e` must be live and exclusive; `ptr` must be readable for `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn esp32sim_nat_event(e: *mut Emu, kind: u32, id: u32, arg: u32, ptr: *const u8, len: usize) -> u32 {
    let e = unsafe { &mut *e };
    let data = unsafe { bytes(ptr, len) };
    let (Some(h), Some(ev)) = (&e.nat, HostEvent::from_abi(kind, id, arg, data)) else { return 0 };
    h.event(ev)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{esp32sim_delete, esp32sim_new, esp32sim_wifi};
    use esp_soc::nat::NatTransport;

    /// (op, flags) of each record the next take hands over.
    fn take(e: *mut Emu) -> Vec<(u8, u8)> {
        // SAFETY: the test owns the live emulator; the bytes are read before the next call.
        let bytes = unsafe { let n = esp32sim_nat_take(e); std::slice::from_raw_parts(esp32sim_nat_ptr(e), n).to_vec() };
        let mut out = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let len = u32::from_le_bytes(bytes[at + 8..at + 12].try_into().unwrap()) as usize;
            out.push((bytes[at], bytes[at + 1]));
            at += (12 + len).next_multiple_of(4);
        }
        out
    }

    /// A relay with one flow the guest opened: its TCP_CONNECT waits, untaken.
    fn relay_with_a_flow(e: *mut Emu) {
        // SAFETY: the test owns the live emulator.
        let emu = unsafe { &mut *e };
        let (transport, handle) = QueueTransport::new();
        let mut nat = Nat::with_transport(false, Box::new(transport));
        let mut syn = vec![0u8; 20];
        syn[..2].copy_from_slice(&1234u16.to_be_bytes());
        syn[2..4].copy_from_slice(&80u16.to_be_bytes());
        syn[12] = 0x50; syn[13] = 0x02;
        assert!(nat.tcp_in(&[2; 6], &[10, 0, 2, 15], &[93, 184, 215, 14], &syn, 0).is_empty());
        emu.m.attach_nat(Some(nat), false).unwrap();
        emu.nat = Some(handle);
    }

    /// Switching the relay, or a new access point, closes the old flows: the next take hands over
    /// the old queue's records, the aborts last, then the new queue's.
    #[test]
    fn a_replaced_relay_hands_over_its_records_and_the_closes() {
        let board = "none";
        // SAFETY: the board name is readable; the emulator is deleted below.
        let e = unsafe { esp32sim_new(board.as_ptr(), board.len(), 4, 0) };
        let spec = "ssid=esp32sim";
        // SAFETY: the test owns the live emulator; `spec` is readable for the call.
        assert_eq!(unsafe { esp32sim_wifi(e, spec.as_ptr(), spec.len()) }, 0);
        relay_with_a_flow(e);
        // SAFETY: as above.
        assert_eq!(unsafe { esp32sim_nat_relay(e, 1) }, 0);
        assert_eq!(take(e), [(op::TCP_CONNECT, 0), (op::TCP_CLOSE, 1)]);
        assert!(take(e).is_empty(), "handed over once");
        relay_with_a_flow(e);
        // SAFETY: as above.
        assert_eq!(unsafe { esp32sim_wifi(e, spec.as_ptr(), spec.len()) }, 0);
        assert_eq!(take(e), [(op::TCP_CONNECT, 0), (op::TCP_CLOSE, 1)]);
        // SAFETY: the emulator is live and not used again.
        unsafe { esp32sim_delete(e) };
    }

    /// A relay whose queue holds a UDP flow's open, an empty datagram and its close, untaken.
    fn relay_with_a_closed_udp_flow(e: *mut Emu) {
        let (mut transport, handle) = QueueTransport::new();
        let id = transport.udp_open(std::net::SocketAddrV4::new([93, 184, 215, 14].into(), 123)).unwrap();
        assert_eq!(transport.udp_send(id, &[]).unwrap(), 0);
        transport.udp_close(id);
        // SAFETY: the test owns the live emulator.
        unsafe { &mut *e }.nat = Some(handle);
    }

    /// Left-over records of `room` bytes less than the bound: one record that fills them.
    fn leave_room(e: *mut Emu, room: usize) {
        let mut waiting = vec![0u8; OUT_MAX - room];
        waiting[0] = op::UDP_DATA;
        waiting[8..12].copy_from_slice(&((OUT_MAX - room - RECORD_HEADER) as u32).to_le_bytes());
        // SAFETY: the test owns the live emulator.
        unsafe { &mut *e }.nat_left = waiting;
    }

    /// A page that keeps switching without taking: the left-over records stop at `OUT_MAX` bytes,
    /// and from the first record of a queue that does not fit only its closes are kept.
    #[test]
    fn left_over_records_are_bounded_and_the_closes_are_kept() {
        let board = "none";
        // SAFETY: the board name is readable; the emulator is deleted below.
        let e = unsafe { esp32sim_new(board.as_ptr(), board.len(), 4, 0) };
        let spec = "ssid=esp32sim";
        // SAFETY: the test owns the live emulator; `spec` is readable for the call.
        assert_eq!(unsafe { esp32sim_wifi(e, spec.as_ptr(), spec.len()) }, 0);
        // the open (16 bytes) and the empty datagram (12) fill the room exactly; the close is past it
        leave_room(e, 28);
        relay_with_a_closed_udp_flow(e);
        // SAFETY: as above.
        assert_eq!(unsafe { esp32sim_nat_relay(e, 0) }, 0);
        assert_eq!(take(e), [(op::UDP_DATA, 0), (op::UDP_OPEN, 0), (op::UDP_DATA, 0), (op::UDP_CLOSE, 0)]);
        // the open does not fit; the datagram after it would, and is dropped with it
        leave_room(e, 12);
        relay_with_a_closed_udp_flow(e);
        // SAFETY: as above.
        assert_eq!(unsafe { esp32sim_nat_relay(e, 1) }, 0);
        relay_with_a_flow(e);
        // SAFETY: as above.
        assert_eq!(unsafe { esp32sim_nat_relay(e, 0) }, 0);
        assert_eq!(take(e), [(op::UDP_DATA, 0), (op::UDP_CLOSE, 0), (op::TCP_CLOSE, 1)]);
        assert!(take(e).is_empty());
        // SAFETY: the emulator is live and not used again.
        unsafe { esp32sim_delete(e) };
    }
}
