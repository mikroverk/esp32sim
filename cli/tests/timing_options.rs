// Invalid timing options must fail before firmware setup, including Cooja's input loop.
#[test]
fn invalid_timing_options_exit_as_usage_errors() {
    for (args, env, message) in [
        (vec!["--chip", "c3", "--approximate-timing"], None, "require --chip s3"),
        (vec!["--chip", "c6", "--cooja", "--approximate-cache"], None, "require --chip s3"),
        (vec!["--memory-contention"], None, "requires --approximate-memory"),
        (vec!["--approximate-memory", "3"], None, "requires --boot rom"),
        (vec!["--approximate-memory", "wrong"], None, "--approximate-memory: expected"),
        (vec!["--approximate-cache"], Some(("ESP32SIM_CACHE_FILL", "wrong")), "ESP32SIM_CACHE_FILL: expected"),
        (vec!["--approximate-cache"], Some(("ESP32SIM_CACHE_WRITEBACK", "-1")), "ESP32SIM_CACHE_WRITEBACK: expected"),
        (vec!["--approximate-memory", "3", "--boot", "rom", "--approximate-cache"], Some(("ESP32SIM_CACHE_FILL", "wrong")), "ESP32SIM_CACHE_FILL: expected"),
    ] {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_esp32sim"));
        command.args(&args).env_remove("ESP32SIM_CACHE_FILL").env_remove("ESP32SIM_CACHE_WRITEBACK");
        if let Some((name, value)) = env { command.env(name, value); }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(message), "{args:?}: {stderr}");
        assert!(!stderr.contains("panicked"), "{stderr}");
    }
}

#[test]
fn invalid_pwm_pins_exit_as_usage_errors() {
    for args in [vec!["--pwm"], vec!["--pwm", "wrong"], vec!["--pwm", "-1"], vec!["--pwm", "256"]] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_esp32sim")).args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(!stderr.contains("panicked"));
        assert!(stderr.contains(if args.len() == 1 { "usage:" } else { "--pwm:" }));
    }
}
