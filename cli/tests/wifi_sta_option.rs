// `--wifi-sta` puts a virtual station on the classic ESP32's air (the guest as access point). It
// is refused where it does not work yet: on the other chips, beside `--wifi` (AP+STA), and with a
// spec it cannot use.
fn esp32sim(args: &[&str]) -> (Option<i32>, String) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_esp32sim")).args(args).output().unwrap();
    (output.status.code(), String::from_utf8(output.stderr).unwrap())
}

#[test]
fn wifi_sta_is_refused_where_it_does_not_work_yet() {
    for (args, why) in [
        (&["--chip", "s3", "--wifi-sta", "ssid=lab"][..], "only the classic ESP32 so far"),
        (&["--chip", "esp32", "--wifi-sta", "ssid=lab", "--wifi", "ssid=demo"][..], "are not modelled yet"),
        (&["--chip", "esp32", "--wifi-sta", "ssid=lab,psk=short"][..], "8 to 63 ASCII characters"),
        (&["--chip", "esp32", "--wifi-sta", "psk=lab-pass"][..], "needs ssid=NAME"),
        (&["--chip", "esp32", "--wifi-sta", "ssid=lab", "--wifi-sta", "ssid=lab2"][..], "one for now"),
        (&["--chip", "esp32", "--wifi-sta", "ssid=lab", "--net", "none,epoch=1767225600"][..], "--net configures the network behind --wifi's access point"),
    ] {
        let (code, stderr) = esp32sim(args);
        assert_eq!(code, Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains(why), "{args:?}: {stderr}");
    }
}

#[test]
fn classic_esp32_accepts_the_wifi_sta_option() {
    let (_, stderr) = esp32sim(&["--chip", "esp32", "--wifi-sta", "ssid=lab,psk=lab-pass", "--pcap", "/dev/null", "--max-insns", "1"]);
    assert!(stderr.contains("virtual station 02:53:54:41:00:01 for 'lab' (WPA2-PSK)"), "{stderr}");
    assert!(!stderr.contains("--pcap requires") && !stderr.contains("panicked"), "{stderr}");
}
