# UART devices on a board

`BoardModel::uart_tx(route, byte)` observes a firmware FIFO write. The route
contains the port, every non-inverted TX GPIO, the selected RX GPIO, and the
configured baud. Check `route.transmits_on(device_rx_pin)` and
`route.matches_baud(device_baud)` before passing the byte to a device model.
The baud check allows three percent error. An absent baud means the UART has no
configured source or divider.

Return `UartInput { pin: device_tx_pin, baud, data }` from `BoardModel::uart_rx()`
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
A model that needs timed delivery must pace the bytes it returns. The existing
host console queues and `SocBus::uart_input` retain their behavior.

[The echo example](../cli/examples/uart_echo.rs) connects a board endpoint to
GPIO5/4 at 9600 baud. Its executable checks the supplied Arduino sketch on any
of the three chips. See the [validation receipt](evidence/uart-endpoint-2026-10-02/README.md)
for commands and the passing three-chip Arduino checks.
