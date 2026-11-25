# A board in another process: `--board socket:`

`--board socket:PATH` (a Unix socket) or `--board tcp:HOST:PORT` replaces the board with a
`BoardModel` that forwards its calls to another process: a modem model, a GNSS source, a
recording, a test harness in any language. The emulator connects to the socket, so the peer
listens first. Classic ESP32 only for now (`--chip esp32`); UART only for now, GPIO is next. Design:
[issue #325](https://github.com/mikroverk/esp32sim/issues/325).

```sh
cargo run --release -p esp32sim --example socket_at_peer -- /tmp/board.sock &   # an AT modem on UART1, answers on GPIO26
target/release/esp32sim --chip esp32 --board socket:/tmp/board.sock --boot rom ... --max-seconds 10
```

The peer in [`cli/tests/support/at_peer.rs`](../cli/tests/support/at_peer.rs) is the reference:
CI runs it against a small guest in [`cli/tests/socket_board.rs`](../cli/tests/socket_board.rs),
and the example above serves it on a socket.

## Time

The emulator keeps the clock and runs its normal loop. Every time is an integer number of
nanoseconds of emulated time since power-on, derived from the bus's cycle counter, which never
restarts, not even across a chip reset. The peer never needs to know a chip's clock.

The peer makes two promises, in `hello` and again in each `done`:

- **`quiet_until`**: it will produce nothing of its own before this time. `null`: never.
- **`latency`**: it answers something it is sent no sooner than this long after it. `null`: it
  never answers (a recorder).

The emulator runs freely until `min(quiet_until, first event since the last sync + latency)`,
then sends `sync` and waits for the peer's `done`. A GNSS source that speaks once a second costs
one exchange per emulated second; a modem with a 1 ms latency costs at most one per millisecond of
traffic, not one per byte. The board sees time at device ticks, so a sync, and a received
byte, can come up to one scheduling quantum (64 instructions) after the time it was due. The
peer answers only after a `sync` and stops at `done`; with a deterministic peer the whole run is
reproducible.

If the peer closes the connection, sends something malformed, or does not answer a message
within 30 s of wall time, the run stops with exit status 1 and an error naming the peer. It never
waits silently or carries on without the board.

## Messages

One JSON object per line, in both directions. Byte payloads are lowercase or uppercase hex
without separators. A message type the peer does not know can be ignored: later versions add
message types only on request in `hello`.

### Emulator to peer

| Message | When |
| --- | --- |
| `{"type":"hello","version":1,"chip":"esp32"}` | first, once |
| `{"type":"uart_tx","t":T,"t_end":T2,"port":1,"tx":[27],"rx":26,"baud":115273,"data":"41540d"}` | the guest wrote these bytes to a UART's TX FIFO, the first at `t`, the last at `t_end`; consecutive bytes on one route share a message (at most 256) |
| `{"type":"reset","t":T,"cause":12}` | the guest came out of a chip reset with this RESET_REASON code (a wake from deep sleep is one too); the board, and the peer, are not reset |
| `{"type":"sync","t":T}` | emulated time is at `t`; answer with your output, then `done` |
| `{"type":"stop","t":T}` | the run is over; then the connection closes |

`uart_tx` carries the route at the FIFO write: `port`, every GPIO the TX signal is routed to
(`tx`), the GPIO the port receives from (`rx`, `null` if none) and the configured baud (`null`
if the UART has no clock source or divider). Times are FIFO-write times, not when the byte would
leave the wire, so a recording made against the emulator is not comparable byte for byte with
wire times captured from a device.

### Peer to emulator

| Message | When |
| --- | --- |
| `{"type":"hello","version":1,"name":"at-modem","quiet_until":0,"latency":1000000,"uarts":[1]}` | the answer to `hello`. `uarts` (optional) lists the ports whose transmissions it wants; the default is all of them, the console included |
| `{"type":"uart_rx","t":T,"pin":26,"baud":115200,"data":"0d0a4f4b0d0a","bits":10}` | bytes on the line into chip GPIO `pin`, the first character starting at `t`; only after a `sync` |
| `{"type":"log","line":"..."}` | a line for the emulator's stderr; only after a `sync` |
| `{"type":"done","quiet_until":T,"latency":L}` | the end of the answer to a `sync`. `latency` is optional (unchanged if absent) |

The emulator releases `uart_rx` bytes one character time apart at `baud`: byte i is complete
`(i + 1) × bits` bit times after the line is free, which is `t` or the end of what that pin
already carries, and never before the sync that delivered it. `bits` is the whole character
(start, data, parity, stop bits; 10 for 8N1, the default). Without the pacing, a 1500-byte PPP
frame would overflow the 128-byte RX FIFO at once. The bytes then go through the chip's routing
like any other board's: a UART receives them if its RX signal is routed from `pin` and its baud
is within 3 % of `baud`, a baud mismatch raises its framing-error interrupt ([uart.md](uart.md)).

## Not yet

- GPIO (`gpio_out`, `gpio_in`, `release`): the next pull request.
- The ESP32-S3 and ESP32-C6. Their buses already call the same UART hooks and reset hook.
- SPI and I2C. An I2C read needs its answer at once, so forwarding it costs a round trip per byte.
- Real devices on the far end with `--realtime`: stamping wall time as emulated time needs its own
  design.
