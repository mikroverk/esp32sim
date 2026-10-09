const GPIO: u32 = 0x6000_4000;
const CPU_HZ: u64 = 240000000;
macro_rules! machine {
    () => {
        esp32s3::machine([0; 6])
    };
}
include!("../../tests/gpio_feedback.rs");
