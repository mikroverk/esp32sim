# UART devices on a board

Opt in with `BoardModel::uses_uart_pins()`. After installing or replacing a board,
call `bus.attach_board_devices()` to refresh the cached capability. The bare-module
path performs no UART board calls or route decoding.
`BoardModel::uart_tx(cycle, route, byte)` observes a firmware FIFO write. The route
contains the port, every non-inverted TX GPIO, the selected RX GPIO, and the
configured baud. Check `route.transmits_on(device_rx_pin)` and
`route.matches_baud(device_baud)` before passing the byte to a device model.
The baud check allows three percent error. An absent baud means the UART has no
configured source or divider.

Return `UartInput::new(device_tx_pin, baud, data)` from `BoardModel::uart_rx(cycle)`
to inject completed characters at the next device tick. Every UART receiving on
that GPIO can see them. Unrouted input is discarded. A baud mismatch discards the
input and raises the UART's framing-error interrupt. Matching input uses the
existing 128-byte FIFO, overflow flag and receive interrupts.

Routes follow the GPIO matrix and native IO_MUX UART functions on S3, C3 and C6.
TX is observed when the byte is written, so a later pin or clock change cannot
relabel it. GPIO matrix input selection is independent of the IO_MUX output
function. The input buffer must be enabled. Board-owned device state survives
chip resets; firmware must reconfigure the UART and pins after reset.

This is a completed-byte interface for ordinary, non-inverted 8N1 serial. It does
not simulate wire timing, parity, flow control, inverted signals or clock gating.
Both callbacks receive emulated CPU bus cycles. A model can queue replies and
return them from `uart_rx` when their deadline has passed. Delivery occurs at
the next device tick, not at individual serial bit boundaries. The existing
host console queues and `SocBus::uart_input` retain their behavior.

[The echo example](../cli/examples/uart_echo.rs) connects a board endpoint to
GPIO5/4 at 9600 baud. Its executable checks the supplied Arduino sketch on any
of the three chips. See the [validation receipt](evidence/uart-endpoint-2026-10-02/README.md)
for commands and the passing three-chip Arduino checks.
