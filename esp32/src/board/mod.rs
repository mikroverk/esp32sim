//! Boards wired for the classic ESP32, independent of S3 pin assignments.
//!
//! - `TtgoTDisplay` — the LilyGO TTGO T-Display: an ST7789 135×240 on VSPI, two buttons
//! - `UlanziTc001` — the Ulanzi TC001 pixel clock: a 32×8 WS2812 matrix on RMT, three buttons
//! - `GeneralDriver` — Waveshare's General Driver for Robots (UGV Rover): motors, encoders, OLED, INA219
//! - `NoBoard` — a bare module (`none`, `bare`, `esp32dev`): nothing on the pins, console only
//! - `esp32dev-loopback` — the SPI loopback fixture in `crate::spi`
pub mod tdisplay;
pub mod aithinker;
pub mod ulanzi;
pub mod cyd;
pub mod general_driver;
#[cfg(not(target_arch = "wasm32"))]
pub mod rover_sim;

pub use tdisplay::*;

pub fn make_board(name: &str) -> Option<esp_soc::Board> {
    // `waveshare-general-driver,sim=HOST:PORT`: the rover board linked to a physics simulation
    if let Some((base, opts)) = name.split_once(',') {
        if !matches!(base, "waveshare-general-driver" | "general-driver" | "ugv-rover") { return None; }
        let addr = opts.strip_prefix("sim=").filter(|a| a.contains(':'))?;
        #[cfg(not(target_arch = "wasm32"))]
        return Some(Box::new(general_driver::GeneralDriver::with_sim(addr)));
        #[cfg(target_arch = "wasm32")]
        { let _ = addr; return None; }
    }
    match name {
        "none" | "bare" | "esp32dev" => Some(Box::new(esp_soc::NoBoard)),
        "ttgo-tdisplay" | "tdisplay" => Some(Box::new(TtgoTDisplay::new())),
        "ai-thinker-cam" | "esp32cam" => Some(Box::new(aithinker::AiThinkerCam::default())),
        "cyd" | "esp32-2432s028" => Some(Box::new(cyd::Cyd::new())),
        "ulanzi-tc001" | "tc001" => Some(Box::new(ulanzi::UlanziTc001::new())),
        "waveshare-general-driver" | "general-driver" | "ugv-rover" => Some(Box::new(general_driver::GeneralDriver::new())),
        _ => crate::spi::make_board(name),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn classic_boards_reject_s3_names() {
        for name in ["none", "bare", "esp32dev", "ttgo-tdisplay", "tdisplay", "ulanzi-tc001", "tc001", "waveshare-general-driver"] { assert!(super::make_board(name).is_some()); }
        for name in ["waveshare-amoled18-v2", "waveshare-lcd4b", "atech14", "waveshare-c6-lcd147"] { assert!(super::make_board(name).is_none()); }
    }
}
