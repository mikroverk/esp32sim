#!/usr/bin/env python3
"""Run from repository root; restore each source after a deliberate mutation."""
import json
import os
from pathlib import Path
import subprocess

lc = 'esp32c3/src/ble_lc.rs'
peer = 'esp-soc/src/ble/peer.rs'
cases = [
    (lc, 'if link & 0x8000 != 0 {', 'if false {', 'active_scan_uses_guest_rx_ring_and_ifs_before_response'),
    (lc, 'if c.anchor < event.due || c.anchor > event.due + width * HALF_US_CYCLES {', 'if false {', 'connection_anchor_must_fit_the_programmed_receive_window'),
    (lc, 'if c.pending.is_none() {', 'if true {', 'unacknowledged_connection_tx_reuses_the_cached_packet'),
    (lc, '0x0c => c.version(),', '0x0c => {},', 'version_reply_is_dispatched_from_guest_tx_and_duplicates_are_suppressed'),
    (lc, 'if !self.version_sent {', 'if true {', 'peripheral_version_request_gets_one_response_without_att'),
    (lc, 'let unmapped = (previous + hop) % 37;', 'let unmapped = previous;', 'csa1_hops_and_remaps_sparse_channels'),
    (lc, 'let index = unmapped as u32 % map.count_ones();', 'let index = 0;', 'csa1_hops_and_remaps_sparse_channels'),
    (lc, 'self.sn ^= 1;', 'self.sn ^= 0;', 'sequence_retransmits_until_ack_and_rejects_duplicate_payload'),
    (lc, 'self.nesn ^= 1;', 'self.nesn ^= 0;', 'sequence_retransmits_until_ack_and_rejects_duplicate_payload'),
    (lc, 'if c.peripheral.acknowledge(pdu[0]) {', 'if true {', 'tx_descriptor_is_released_only_after_peer_ack'),
    (lc, 'self.state.as_mut().unwrap().raise(1 << 1);', 'self.state.as_mut().unwrap().raise(1 << 6);', 'tx_descriptor_is_released_only_after_peer_ack'),
    (lc, '2 * (40 + ((self.ram.read(0x90) >> 8) & 127) as u64)', '0', 'sync_timestamp_recovers_packet_start_across_half_slots_and_wrap'),
    (lc, 'if s.connection.is_some() { return Err("central is already connected".into()) }', '', 'commands_reject_disabled_busy_and_unsupported_states'),
    (lc, 'if c.terminating || c.stopped { return Err("central is already stopping".into()) }', '', 'commands_reject_disabled_busy_and_unsupported_states'),
    (peer, 'if start == 0 || end < start {', 'if false {', 'uuid_read_validates_pages_permissions_and_errors'),
    (peer, 'if declaration <= last || declaration > end || handle <= declaration || handle > end {', 'if false {', 'uuid_read_validates_pages_permissions_and_errors'),
    (peer, 'if item[2] & 2 == 0 {', 'if false {', 'uuid_read_validates_pages_permissions_and_errors'),
    (peer, 'let next = last + 1;', 'let next = last;', 'uuid_read_validates_pages_permissions_and_errors'),
    (peer, 'if item[5..] == self.characteristic {', 'if true {', 'uuid_read_validates_pages_permissions_and_errors'),
    (peer, 'bytes.reverse();', '', 'uuid_read_validates_pages_permissions_and_errors'),
    (lc, 'event.phase = ScanPhase::Receive(channel, pdu);', 'event.phase = ScanPhase::Receive(channel, connect_request(&advertising.pdu));', 'connect_command_does_not_change_an_inflight_scan_request'),
    (lc, 'event.due += 300 * HALF_US_CYCLES;\n                event.phase = ScanPhase::Response(channel);', 'event.due += 0;\n                event.phase = ScanPhase::Response(channel);', 'active_scan_uses_guest_rx_ring_and_ifs_before_response'),
    (lc, 'self.ram.write(0x24, (link & 0x7fff) as u32);', 'self.ram.write(0x24, logical);', 'active_scan_uses_guest_rx_ring_and_ifs_before_response'),
    (lc, 'put_half(sram, cs + 28, next);', 'put_half(sram, cs + 28, 0);', 'tx_descriptor_is_released_only_after_peer_ack'),

    ('esp-soc/src/machine.rs', 'self.bus.load_bytes(source, &section.data)?;', '', 'bluetooth_rom_initializer_has_its_rom_source_copy'),

    (peer, 'if matches!(command, Command::CentralStop | Command::Disconnect | Command::ReadUuid(..)) {', 'if false {', 'full_controller_commands_do_not_queue_in_hci_mode'),
    (lc, 's.host = old.host;', 's.host = Host::default();', 'host_configuration_survives_controller_reset'),
    (lc, 's.host = std::mem::take(&mut old.host);', 's.host = Host::default();', 'host_configuration_survives_machine_reboot'),
    (lc, 'let pdu = std::mem::take(&mut c.incoming);', 'let pdu = c.outgoing.front().cloned().unwrap_or_else(|| vec![1,0]);', 'central_receive_uses_the_transmitted_packet_and_header'),
    (lc, '?.silent = true,', '?.silent = false,', 'full_ble_script_connect_read_and_stop'),
    (peer, 'b.len() <= 20', 'true', 'hex_bytes_and_read_by_type_preserve_wire_values'),
    (peer, 'end as u8, (end >> 8) as u8, 3, 0x28', '0xff, 0xff, 3, 0x28', 'hex_bytes_and_read_by_type_preserve_wire_values'),

]
results = []
for file, before, after, test in cases:
    path = Path(file)
    original = path.read_text()
    assert original.count(before) == 1, (file, before)
    package = 'esp32c3' if file == lc else 'esp-soc'
    command = ['cargo', '+1.99.0', 'test', '-p', package, '--lib', test]
    if test == 'bluetooth_rom_initializer_has_its_rom_source_copy':
        command = ['cargo', '+1.99.0', 'test', '-p', 'esp32c3', '--test', 'ble_full', test, '--', '--ignored']
    if test == 'full_ble_script_connect_read_and_stop':
        command = ['cargo', '+1.99.0', 'test', '--release', '-p', 'esp32sim', '--test', 'ble', test, '--', '--ignored']
    env = os.environ | {'ESP32SIM_ROM_DIR': str(Path('web/wasm/fw').resolve())}
    try:
        path.write_text(original.replace(before, after))
        run = subprocess.run(command, capture_output=True, text=True, env=env)
        output = run.stdout + run.stderr
        killed = run.returncode != 0 and 'test result: FAILED' in output and test in output
        results.append(dict(file=file, mutation=before + ' -> ' + after, test=test, killed=killed))
        print(json.dumps(results[-1]), flush=True)
        if not killed:
            raise AssertionError(output)
    finally:
        path.write_text(original)
Path('docs/evidence/ble-c3-connection/mutations.json').write_text(json.dumps(results, indent=2) + '\n')
