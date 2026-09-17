//! Native raw-TCP transport for an emulated UART.
//!
//! This module deliberately knows nothing about ESP ROMs, SLIP or esptool. It preserves byte
//! streams between one TCP client and bounded host-side queues; the machine decides when bytes
//! can enter or leave an emulated UART FIFO.
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

const QUEUE_CAPACITY: usize = 1 << 20;
const IO_CHUNK: usize = 4096;
const POLL_DELAY: Duration = Duration::from_millis(1);

struct State {
    input: VecDeque<u8>,
    output: VecDeque<u8>,
    client: Option<u64>,
    next_client: u64,
}

struct Shared {
    state: Mutex<State>,
}

/// One raw UART listener. The most recently accepted client owns the byte stream; reconnecting
/// clears bytes left by the previous connection but does not affect emulator state.
#[derive(Clone)]
pub struct UartTcp {
    shared: Arc<Shared>,
    local_addr: SocketAddr,
}

impl UartTcp {
    pub fn start(addr: impl ToSocketAddrs) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        let local_addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                input: VecDeque::new(),
                output: VecDeque::new(),
                client: None,
                next_client: 1,
            }),
        });
        let weak = Arc::downgrade(&shared);
        std::thread::spawn(move || accept_loop(listener, weak));
        Ok(Self { shared, local_addr })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn connected(&self) -> bool {
        self.shared.state.lock().unwrap().client.is_some()
    }

    /// Remove at most `max` bytes received from the client, preserving their order.
    pub fn take_input(&self, max: usize) -> Vec<u8> {
        let mut state = self.shared.state.lock().unwrap();
        let n = max.min(state.input.len());
        state.input.drain(..n).collect()
    }

    /// Queue as much UART output as fits. The caller retains and retries an unaccepted suffix.
    pub fn queue_output(&self, data: &[u8]) -> usize {
        let mut state = self.shared.state.lock().unwrap();
        if state.client.is_none() {
            return 0;
        }
        let n = data.len().min(QUEUE_CAPACITY - state.output.len());
        state.output.extend(&data[..n]);
        n
    }

    pub fn pending_input(&self) -> usize {
        self.shared.state.lock().unwrap().input.len()
    }
    pub fn pending_output(&self) -> usize {
        self.shared.state.lock().unwrap().output.len()
    }

    /// Drop the active client and all bytes belonging to that connection.
    pub fn disconnect(&self) {
        let mut state = self.shared.state.lock().unwrap();
        state.client = None;
        state.input.clear();
        state.output.clear();
    }
}

fn accept_loop(listener: TcpListener, shared: Weak<Shared>) {
    while let Some(shared) = shared.upgrade() {
        match listener.accept() {
            Ok((stream, _)) => attach(stream, &shared),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(POLL_DELAY)
            }
            Err(_) => break,
        }
    }
}

fn attach(stream: TcpStream, shared: &Arc<Shared>) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_nonblocking(true);
    let client = {
        let mut state = shared.state.lock().unwrap();
        let client = state.next_client;
        state.next_client = state.next_client.wrapping_add(1).max(1);
        state.client = Some(client);
        state.input.clear();
        state.output.clear();
        client
    };
    let weak = Arc::downgrade(shared);
    std::thread::spawn(move || client_loop(stream, weak, client));
}

fn client_loop(mut stream: TcpStream, shared: Weak<Shared>, client: u64) {
    let mut input = [0u8; IO_CHUNK];
    loop {
        let Some(shared) = shared.upgrade() else {
            return;
        };
        let input_room = {
            let state = shared.state.lock().unwrap();
            if state.client != Some(client) {
                return;
            }
            QUEUE_CAPACITY - state.input.len()
        };
        if input_room > 0 {
            match stream.read(&mut input[..input_room.min(IO_CHUNK)]) {
                Ok(0) => break,
                Ok(n) => {
                    let mut state = shared.state.lock().unwrap();
                    if state.client != Some(client) {
                        return;
                    }
                    state.input.extend(&input[..n]);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
        }

        let output: Vec<u8> = {
            let state = shared.state.lock().unwrap();
            if state.client != Some(client) {
                return;
            }
            state.output.iter().take(IO_CHUNK).copied().collect()
        };
        if !output.is_empty() {
            match stream.write(&output) {
                Ok(0) => break,
                Ok(n) => {
                    let mut state = shared.state.lock().unwrap();
                    if state.client != Some(client) {
                        return;
                    }
                    state.output.drain(..n);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
        }
        std::thread::sleep(POLL_DELAY);
    }
    if let Some(shared) = shared.upgrade() {
        let mut state = shared.state.lock().unwrap();
        if state.client == Some(client) {
            state.client = None;
            state.input.clear();
            state.output.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn wait_for(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !ready() {
            assert!(Instant::now() < deadline, "UART TCP operation timed out");
            std::thread::sleep(POLL_DELAY);
        }
    }

    #[test]
    fn preserves_binary_bytes_in_both_directions() {
        let uart = UartTcp::start("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(uart.local_addr()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        wait_for(|| uart.connected());

        let from_client = [0x00, 0xc0, 0xdb, 0xff, 0x80, 0x7f];
        client.write_all(&from_client).unwrap();
        wait_for(|| uart.pending_input() == from_client.len());
        assert_eq!(uart.take_input(2), from_client[..2]);
        assert_eq!(uart.take_input(99), from_client[2..]);

        let to_client = [0xff, 0xdb, 0xc0, 0x00, 0x81];
        assert_eq!(uart.queue_output(&to_client), to_client.len());
        let mut received = [0u8; 5];
        client.read_exact(&mut received).unwrap();
        assert_eq!(received, to_client);
    }

    #[test]
    fn reconnect_replaces_the_previous_stream_and_clears_its_queues() {
        let uart = UartTcp::start("127.0.0.1:0").unwrap();
        let mut first = TcpStream::connect(uart.local_addr()).unwrap();
        wait_for(|| uart.connected());
        first.write_all(b"old").unwrap();
        wait_for(|| uart.pending_input() == 3);

        let mut second = TcpStream::connect(uart.local_addr()).unwrap();
        second
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        wait_for(|| uart.connected() && uart.pending_input() == 0);
        second.write_all(b"new").unwrap();
        wait_for(|| uart.pending_input() == 3);
        assert_eq!(uart.take_input(3), b"new");
        assert_eq!(uart.queue_output(b"ok"), 2);
        let mut reply = [0u8; 2];
        second.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"ok");
    }

    #[test]
    fn output_is_not_buffered_without_a_client() {
        let uart = UartTcp::start("127.0.0.1:0").unwrap();
        assert_eq!(uart.queue_output(b"unclaimed"), 0);
        assert_eq!(uart.pending_output(), 0);
    }

    #[test]
    fn output_queue_never_exceeds_its_bound() {
        let uart = UartTcp::start("127.0.0.1:0").unwrap();
        {
            let mut state = uart.shared.state.lock().unwrap();
            state.client = Some(1);
            state.output.resize(QUEUE_CAPACITY - 2, 0xaa);
        }
        assert_eq!(uart.queue_output(&[1, 2, 3, 4]), 2);
        assert_eq!(uart.pending_output(), QUEUE_CAPACITY);
    }
}
