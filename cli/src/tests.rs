use super::*;

#[test]
fn ble_is_opt_in_and_requires_a_supported_adapter_and_symbols() {
    assert!(!parse(&["esp32sim".into()], "esp32s3").ble);
    assert!(parse(&["esp32sim".into(), "--ble".into()], "esp32s3").ble);
    let symbols = esp_soc::elf::Elf::default();
    assert!(esp32s3::machine([0; 6]).bus.enable_ble(&symbols).is_err());
    assert!(esp32c3::machine([0; 6], 4 << 20).bus.enable_ble(&symbols).is_err());
    assert!(esp32c6::machine([0; 6], 4 << 20).bus.enable_ble(&symbols).is_err());
}


#[test]
fn function_trace_patterns_share_prefix_and_exact_matching() {
    let mut m = esp32c6::machine([0; 6], 4 << 20);
    m.symbols.extend([(1, "foo".into()), (2, "foobar".into()), (3, "other".into())]);
    assert_eq!(m.trace_fns("foo"), 2);
    assert_eq!(m.fn_probes.len(), 2);
    assert!(m.fn_probes.contains_key(&1));
    assert!(m.fn_probes.contains_key(&2));
    m.fn_probes.clear();
    assert_eq!(m.trace_fns("foo$"), 1);
    assert_eq!(m.fn_probes.keys().copied().collect::<Vec<_>>(), [1]);
    assert_eq!(m.trace_fns("missing$"), 0);
}

#[test]
fn stub_values_are_explicit() {
    for (spec, expected) in [("func", 0), ("func=0", 0), ("func=true", 1), ("func=false", 0), ("func=42", 42), ("func=0x2a", 42)] {
        assert_eq!(stub_spec(spec), Ok(("func", expected)));
    }
    for spec in ["func=typo", "func=", "func=-1", "func=4294967296"] {
        assert!(stub_spec(spec).is_err(), "{spec}");
    }
}

#[test]
fn reset_loop_spends_one_run_budget() {
    let mut m = esp32c6::machine([0; 6], 4 << 20);
    // lui t0,0x600b1; lui t1,0x10000; sw t1,0x38(t0): request CPU reset.
    let program: Vec<u8> = [0x600b12b7u32, 0x10000337, 0x0262ac23].into_iter().flat_map(u32::to_le_bytes).collect();
    m.bus.load_bytes(0x4000_0000, &program).unwrap();
    // A watchdog for the regression itself: the old per-boot budget reaches this cap.
    m.max_cycles = 10_000;
    assert!(matches!(run_with_reboots(&mut m, 10, true, false), Stop::MaxInsns));
    assert!(m.reboots > 0 && m.reboots < 10, "reboots: {}", m.reboots);
    assert!(m.insns() < 100, "instructions: {}", m.insns());
}

/// A C6 with a one-segment app image at the usual offset: an instruction that jumps to itself, in
/// HP SRAM, entered by `boot_app` the way `--boot app` does it.
fn app_mode_machine() -> esp32c6::Machine {
    let mut m = esp32c6::machine([0; 6], 4 << 20);
    let mut image = vec![0xe9, 1, 0, 0];
    image.extend_from_slice(&0x4080_0000u32.to_le_bytes());              // entry
    image.resize(24, 0);
    image.extend_from_slice(&0x4080_0000u32.to_le_bytes());              // the segment: load address, length, data
    image.extend_from_slice(&4u32.to_le_bytes());
    image.extend_from_slice(&0x0000_006fu32.to_le_bytes());              // j .
    m.bus.write_flash(0x10000, &image).unwrap();
    m.boot_app(0x10000).unwrap();
    m.web = Some(esp_soc::web::WebServer::queued());
    m.max_cycles = 100_000;                                              // a test that never ends is not a result
    m
}

/// The page's Restart on an app-mode run, which is the S3's default boot: the chip resets and the
/// app is entered again. It used to end the run, because only a ROM-booted run was brought back up.
#[test]
fn the_pages_restart_brings_an_app_mode_run_back_up() {
    let mut m = app_mode_machine();
    m.web_restart = true;
    m.web.as_ref().unwrap().push_incoming(r#"{"t":"reset"}"#.into());
    let stop = run_with_reboots(&mut m, 2000, false, true);
    assert!(!matches!(stop, Stop::SwReset), "the run ended at the reset: {stop:?}");
    assert_eq!(m.reboots, 1, "one chip reset, then the app again");
    assert_eq!(m.cores[0].pc(), 0x4080_0000, "back in the app");
}

/// With `--no-reboot` the front-end does not offer a restart, and the message is ignored: the page
/// cannot end a run that is meant to stop at a chip reset. A reset the firmware asks for still ends
/// an app-mode run, as before.
#[test]
fn a_run_that_cannot_restart_ignores_the_pages_restart() {
    let mut m = app_mode_machine();
    m.web.as_ref().unwrap().push_incoming(r#"{"t":"reset"}"#.into());
    assert!(matches!(run_with_reboots(&mut m, 2000, false, true), Stop::MaxInsns));
    assert_eq!(m.reboots, 0);

    let mut m = app_mode_machine();
    m.web_restart = true;
    esp_soc::SocBus::request_reset(&mut m.bus, esp_periph::RST_SW_CPU);   // not the button: the firmware
    assert!(matches!(run_with_reboots(&mut m, 2000, false, true), Stop::SwReset));
}

#[test]
fn approximate_options_reject_unsupported_combinations() {
    for chip in ["c3", "c6", "esp32c3", "esp32c6"] {
        for flag in ["--approximate-timing", "--approximate-cache", "--approximate-memory"] {
            let mut args = vec!["esp32sim".into(), flag.into()];
            if flag == "--approximate-memory" { args.push("3".into()); }
            assert!(validate_timing(&parse(&args, chip)).unwrap_err().contains("require --chip s3"));
        }
    }
    let mut o = Opts { chip: "s3".into(), memory_contention: true, ..Default::default() };
    assert!(validate_timing(&o).unwrap_err().contains("requires --approximate-memory"));
    o.approximate_timing = true;
    o.approximate_memory = Some(3);
    assert!(validate_timing(&o).unwrap_err().contains("requires --boot rom"));
    o.boot = Some("rom".into());
    assert!(validate_timing(&o).is_ok());
}

#[test]
fn timing_cycle_values_report_usage_errors() {
    for name in ["--approximate-memory", "ESP32SIM_CACHE_FILL", "ESP32SIM_CACHE_WRITEBACK"] {
        for value in ["", "wrong", "-1", "4294967296"] {
            assert!(timing_cycles(value, name).unwrap_err().starts_with(name));
        }
        assert_eq!(timing_cycles("0", name), Ok(0));
        assert_eq!(timing_cycles("4294967295", name), Ok(u32::MAX));
    }
}

#[test]
fn ble_scripts_validate_commands_and_require_enable_at_parse_time() {
    let mut m = esp32c3::machine([0; 6], 4 << 20);
    assert!(m.load_script("0 ble connect").unwrap_err().contains("--ble"));
    for command in ["conect", "read", "read 0", "read 65536", "write 3 zz", "write 3 f", "discover extra"] {
        let error = m.load_script(&format!("0 ble {command}")).unwrap_err();
        assert!(error.contains("line 1"));
        assert!(!error.contains("--ble"), "syntax must be checked independently of enable state");
    }
}

#[test]
fn pwm_pins_are_observation_options() {
    let o = parse(&["esp32sim".into(), "--pwm".into(), "4".into(), "--pwm".into(), "21".into()], "s3");
    assert_eq!(o.pwm_pins, [4, 21]);
}

#[test]
fn classic_defaults_to_its_own_board_and_keeps_explicit_board_names() {
    assert_eq!(parse(&["emu".into(), "--chip".into(), "esp32".into()], "s3").board, "esp32dev");
    assert_eq!(parse(&["emu".into(), "--chip".into(), "classic".into(), "--board".into(), "none".into()], "s3").board, "none");
}
