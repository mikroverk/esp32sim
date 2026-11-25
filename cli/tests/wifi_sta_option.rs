// `--wifi-sta` puts a virtual station on the air of every chip with WiFi (the guest as access
// point). It is refused where it does not work: on the P4, which has no WiFi of its own, beside
// `--wifi` (AP+STA), and with a spec it cannot use.
fn esp32sim(args: &[&str]) -> (Option<i32>, String) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_esp32sim")).args(args).output().unwrap();
    (output.status.code(), String::from_utf8(output.stderr).unwrap())
}

#[test]
fn wifi_sta_is_refused_where_it_does_not_work_yet() {
    for (args, why) in [
        (&["--chip", "p4", "--wifi-sta", "ssid=lab"][..], "the P4 has no WiFi of its own"),
        (&["--chip", "c6", "--wifi-sta", "ssid=lab", "--wifi", "ssid=demo"][..], "are not modelled yet"),
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
fn every_chip_with_wifi_accepts_the_wifi_sta_option() {
    for chip in ["esp32", "s3", "c3", "c6", "s31"] {
        let (_, stderr) = esp32sim(&["--chip", chip, "--wifi-sta", "ssid=lab,psk=lab-pass", "--pcap", "/dev/null", "--max-insns", "1"]);
        assert!(stderr.contains("virtual station 02:53:54:41:00:01 for 'lab' (WPA2-PSK)"), "{chip}: {stderr}");
        assert!(!stderr.contains("--pcap requires") && !stderr.contains("panicked"), "{chip}: {stderr}");
    }
}
