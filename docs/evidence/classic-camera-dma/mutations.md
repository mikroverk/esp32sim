# C7 mutation-to-test table

All 25 semantic mutants are killed by named assertion failures; baseline passes; zero compile-error kills.

| Mutation | Assertion-failing tests |
|---|---|
| peripheral reset preserves stale frame | c7::c7_dport_reset_drops_capture_and_requires_fresh_inlink |
| camera-free board polling | c7::c7_camera_free_active_pcm_does_not_poll_board_camera |
| camera dispatch cache missing | c7::c7_dport_reset_drops_capture_and_requires_fresh_inlink, c7::c7_bad_owner_forbidden_descriptor_and_buffer_never_eof, c7::c7_qqvga_pattern_twenty_descriptors_ten_serviced_eof_chunks, c7::c7_ring_starvation_and_finite_guest_service_reuse, c7::c7_tick_split_invariance_and_late_arm_waits_vsync, c7::c7_reacquire_next_image_on_reused_ring, c7::c7_mid_frame_route_or_power_loss_is_terminal_until_rearm, c7::c7_stop_restart_and_every_rx_reset_discard_incomplete_frame, c7::c7_exact_native_descriptors_two_eofs_and_classic_irq |
| transport disabled | c7::c7_qqvga_pattern_twenty_descriptors_ten_serviced_eof_chunks, c7::c7_ring_starvation_and_finite_guest_service_reuse, c7::c7_exact_native_descriptors_two_eofs_and_classic_irq, c7::c7_tick_split_invariance_and_late_arm_waits_vsync |
| ignore VSYNC arm | c7::c7_tick_split_invariance_and_late_arm_waits_vsync |
| charge pulse as pixels | c7::c7_qqvga_pattern_twenty_descriptors_ten_serviced_eof_chunks |
| lose fractional word position | c7::c7_ring_starvation_and_finite_guest_service_reuse, c7::c7_qqvga_pattern_twenty_descriptors_ten_serviced_eof_chunks |
| wrong frame rate | c7::c7_exact_native_descriptors_two_eofs_and_classic_irq, c7::c7_ring_starvation_and_finite_guest_service_reuse, c7::c7_qqvga_pattern_twenty_descriptors_ten_serviced_eof_chunks |
| do not consume byte position | c7::c7_qqvga_pattern_twenty_descriptors_ten_serviced_eof_chunks, c7::c7_ring_starvation_and_finite_guest_service_reuse, c7::c7_exact_native_descriptors_two_eofs_and_classic_irq |
| drop final boundary word | c7::c7_bad_owner_forbidden_descriptor_and_buffer_never_eof, c7::c7_exact_native_descriptors_two_eofs_and_classic_irq, c7::c7_reacquire_next_image_on_reused_ring, c7::c7_qqvga_pattern_twenty_descriptors_ten_serviced_eof_chunks, c7::c7_stop_restart_and_every_rx_reset_discard_incomplete_frame, c7::c7_tick_split_invariance_and_late_arm_waits_vsync, c7::c7_dport_reset_drops_capture_and_requires_fresh_inlink, c7::c7_ring_starvation_and_finite_guest_service_reuse |
| wrong FIFO mode | c7::c7_exact_native_descriptors_two_eofs_and_classic_irq, c7::c7_ring_starvation_and_finite_guest_service_reuse, c7::c7_tick_split_invariance_and_late_arm_waits_vsync, c7::c7_stop_restart_and_every_rx_reset_discard_incomplete_frame, c7::c7_reacquire_next_image_on_reused_ring |
| route loss not terminal | c7::c7_mid_frame_route_or_power_loss_is_terminal_until_rearm |
| RX resets keep DMA running | c7::c7_stop_restart_and_every_rx_reset_discard_incomplete_frame |
| inlink stop ignored | c7::c7_stop_restart_and_every_rx_reset_discard_incomplete_frame |
| wrong EOF count unit | c7::c7_exact_native_descriptors_two_eofs_and_classic_irq, c7::c7_ring_starvation_and_finite_guest_service_reuse, c7::c7_tick_split_invariance_and_late_arm_waits_vsync, c7::c7_stop_restart_and_every_rx_reset_discard_incomplete_frame, c7::c7_mid_frame_route_or_power_loss_is_terminal_until_rearm, c7::c7_reacquire_next_image_on_reused_ring, c7::c7_qqvga_pattern_twenty_descriptors_ten_serviced_eof_chunks, c7::c7_bad_owner_forbidden_descriptor_and_buffer_never_eof |
| wrong EOF descriptor register | c7::c7_exact_native_descriptors_two_eofs_and_classic_irq, c7::c7_ring_starvation_and_finite_guest_service_reuse, c7::c7_qqvga_pattern_twenty_descriptors_ten_serviced_eof_chunks |
| wrong classic IRQ mapping | c7::c7_qqvga_pattern_twenty_descriptors_ten_serviced_eof_chunks, c7::c7_stop_restart_and_every_rx_reset_discard_incomplete_frame, c7::c7_ring_starvation_and_finite_guest_service_reuse, c7::c7_exact_native_descriptors_two_eofs_and_classic_irq, c7::c7_reacquire_next_image_on_reused_ring, c7::c7_dport_reset_drops_capture_and_requires_fresh_inlink |
| owner check bypassed | c7::c7_bad_owner_forbidden_descriptor_and_buffer_never_eof, c7::c7_ring_starvation_and_finite_guest_service_reuse |
| owner not returned | c7::c7_qqvga_pattern_twenty_descriptors_ten_serviced_eof_chunks, c7::c7_exact_native_descriptors_two_eofs_and_classic_irq, c7::c7_tick_split_invariance_and_late_arm_waits_vsync, c7::c7_stop_restart_and_every_rx_reset_discard_incomplete_frame, c7::c7_ring_starvation_and_finite_guest_service_reuse |
| length not written | c7::c7_exact_native_descriptors_two_eofs_and_classic_irq, c7::c7_stop_restart_and_every_rx_reset_discard_incomplete_frame, c7::c7_tick_split_invariance_and_late_arm_waits_vsync, c7::c7_qqvga_pattern_twenty_descriptors_ten_serviced_eof_chunks |
| EOF flag not written | c7::c7_exact_native_descriptors_two_eofs_and_classic_irq, c7::c7_qqvga_pattern_twenty_descriptors_ten_serviced_eof_chunks, c7::c7_stop_restart_and_every_rx_reset_discard_incomplete_frame, c7::c7_tick_split_invariance_and_late_arm_waits_vsync |
| wrong descriptor advance | c7::c7_reacquire_next_image_on_reused_ring, c7::c7_ring_starvation_and_finite_guest_service_reuse, c7::c7_qqvga_pattern_twenty_descriptors_ten_serviced_eof_chunks |
| DRAM rule admits CPU-only prefix | c7::c7_bad_owner_forbidden_descriptor_and_buffer_never_eof |
| mode1 sensor sample ordering | c7::c7_exact_native_descriptors_two_eofs_and_classic_irq, c7::c7_reacquire_next_image_on_reused_ring, c7::c7_tick_split_invariance_and_late_arm_waits_vsync, c7::c7_stop_restart_and_every_rx_reset_discard_incomplete_frame, c7::c7_ring_starvation_and_finite_guest_service_reuse, c7::c7_qqvga_pattern_twenty_descriptors_ten_serviced_eof_chunks |
| mode3 sensor sample lane | c7::c7_exact_native_descriptors_two_eofs_and_classic_irq |
