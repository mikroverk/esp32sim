//! Boards wired for the classic ESP32, independent of S3 pin assignments.
pub fn make_board(name: &str) -> Option<esp_soc::Board> {
    match name { "none" | "bare" | "esp32dev" => Some(Box::new(esp_soc::NoBoard)), _ => crate::spi::make_board(name) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn classic_boards_reject_s3_names() {
        for name in ["none", "bare", "esp32dev"] { assert!(super::make_board(name).is_some()); }
        for name in ["waveshare-amoled18-v2", "waveshare-lcd4b", "atech14"] { assert!(super::make_board(name).is_none()); }
    }
}
