//! `--uart-tcp` / `--uart1-tcp`: a UART's bytes both ways over TCP, raw, to one client at a time — what esptool and
//! espefuse open as `--port socket://HOST:PORT` (QEMU's `-serial tcp::PORT,server,nowait`).
//! Single-threaded and non-blocking: the machine polls it from its real-time pacing step, so nothing runs per
//! instruction and an idle socket costs a `read` that returns `WouldBlock`.
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};

/// Most bytes taken from the client per poll; the UART's host queue releases them at the line rate.
const READ_MAX: usize = 64 << 10;
/// Most guest output held for a client that does not read; older bytes beyond it are dropped.
const PENDING_MAX: usize = 1 << 20;

pub struct UartTcp { pub uart: usize, listener: TcpListener, client: Option<TcpStream>, pending: Vec<u8> }

impl UartTcp {
    /// Listen on `addr` (`HOST:PORT`; port 0 picks a free one, see `local_addr`) for UART `uart`.
    pub fn bind(addr: &str, uart: usize) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        Ok(UartTcp { uart, listener, client: None, pending: Vec::new() })
    }
    pub fn local_addr(&self) -> Option<SocketAddr> { self.listener.local_addr().ok() }
    pub fn connected(&self) -> bool { self.client.is_some() }

    /// Take a waiting client when none is connected, append what it sent to `input`, and write held output.
    /// A second client waits in the listen backlog until the first one closes.
    pub fn poll(&mut self, input: &mut Vec<u8>) {
        if self.client.is_none() {
            if let Ok((s, peer)) = self.listener.accept() {
                if s.set_nonblocking(true).is_ok() {
                    let _ = s.set_nodelay(true);
                    eprintln!("[uart{}-tcp] client {peer} connected", self.uart);
                    self.client = Some(s);
                }
            }
        }
        let Some(s) = &mut self.client else { return };
        let mut buf = [0u8; 4096];
        let start = input.len();
        while input.len() - start < READ_MAX {
            match s.read(&mut buf) {
                Ok(0) => { self.drop_client("closed"); return; }
                Ok(n) => input.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => { self.drop_client("lost"); return; }
            }
        }
        self.flush();
    }

    /// Guest output for the client; without one it is discarded, as on an unconnected QEMU chardev.
    pub fn send(&mut self, data: &[u8]) {
        if self.client.is_none() || data.is_empty() { return; }
        self.pending.extend_from_slice(data);
        if self.pending.len() > PENDING_MAX { let cut = self.pending.len() - PENDING_MAX; self.pending.drain(..cut); }
        self.flush();
    }

    fn flush(&mut self) {
        let Some(s) = &mut self.client else { return };
        let mut sent = 0;
        while sent < self.pending.len() {
            match s.write(&self.pending[sent..]) {
                Ok(0) => break,
                Ok(n) => sent += n,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => { self.drop_client("lost"); return; }
            }
        }
        self.pending.drain(..sent);
    }

    fn drop_client(&mut self, how: &str) {
        eprintln!("[uart{}-tcp] client {how}", self.uart);
        self.client = None; self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_cross_both_ways_and_a_closed_client_frees_the_bridge_for_the_next() {
        let mut b = UartTcp::bind("127.0.0.1:0", 0).unwrap();
        let addr = b.local_addr().unwrap();
        b.send(b"before");   // no client yet: discarded
        let mut c = TcpStream::connect(addr).unwrap();
        c.write_all(&[0xc0, 0x00, 0x08, 0xff]).unwrap();
        let mut got = Vec::new();
        for _ in 0..200 { b.poll(&mut got); if got.len() >= 4 { break; } std::thread::sleep(std::time::Duration::from_millis(5)); }
        assert_eq!(got, [0xc0, 0x00, 0x08, 0xff]);
        b.send(&[0x01, 0x80, 0xc0]);
        let mut back = [0u8; 3];
        c.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        c.read_exact(&mut back).unwrap();
        assert_eq!(back, [0x01, 0x80, 0xc0]);
        drop(c);
        for _ in 0..200 { b.poll(&mut got); if !b.connected() { break; } std::thread::sleep(std::time::Duration::from_millis(5)); }
        assert!(!b.connected());
        let mut c2 = TcpStream::connect(addr).unwrap();
        c2.write_all(b"x").unwrap();
        got.clear();
        for _ in 0..200 { b.poll(&mut got); if !got.is_empty() { break; } std::thread::sleep(std::time::Duration::from_millis(5)); }
        assert_eq!(got, b"x");
    }
}
