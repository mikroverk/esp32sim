// `--hostfwd` binds a host TCP or UDP port only when a NAT is attached, and refuses a run that
// could not deliver the SYN or the datagram to a guest.
#[test]
fn hostfwd_requires_wifi_and_nat() {
    let bin = env!("CARGO_BIN_EXE_esp32sim");
    let output = std::process::Command::new(bin)
        .args(["--hostfwd", "tcp:127.0.0.1:0-:80", "--max-insns", "1"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr).unwrap().contains("--hostfwd requires --wifi"));

    let output = std::process::Command::new(bin)
        .args(["--wifi", "ssid=demo", "--net", "none", "--hostfwd", "tcp:127.0.0.1:0-:80", "--max-insns", "1"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr).unwrap().contains("--hostfwd requires --net nat"));

    let output = std::process::Command::new(bin)
        .args(["--wifi", "ssid=demo", "--hostfwd", "sctp:127.0.0.1:0-:80", "--max-insns", "1"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr).unwrap().contains("expected tcp:HOST:HOSTPORT-GUEST:GUESTPORT or udp:"));

    let output = std::process::Command::new(bin)
        .args(["--board", "none", "--wifi", "ssid=demo", "--net", "nat,epoch=1767225600", "--hostfwd", "tcp:127.0.0.1:0-:80", "--max-insns", "1"]).output().unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("--hostfwd requires"), "{stderr}");
    assert!(stderr.contains("hostfwd tcp 127.0.0.1:"), "{stderr}");
}

#[test]
fn hostfwd_binds_before_the_rom_is_required() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_esp32sim"))
        .args(["--board", "none", "--wifi", "ssid=demo", "--hostfwd", "tcp:127.0.0.1:0-:80", "--max-insns", "1"]).output().unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("hostfwd tcp 127.0.0.1:"), "{stderr}");
    assert!(stderr.contains("-> 10.0.2.15:80"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}

#[test]
fn udp_hostfwd_binds_a_host_udp_port() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_esp32sim"))
        .args(["--board", "none", "--wifi", "ssid=demo", "--hostfwd", "udp:127.0.0.1:0-:7", "--hostfwd", "tcp:127.0.0.1:0-:80", "--max-insns", "1"]).output().unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("hostfwd udp 127.0.0.1:") && stderr.contains("-> 10.0.2.15:7"), "{stderr}");
    assert!(stderr.contains("hostfwd tcp 127.0.0.1:"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}
