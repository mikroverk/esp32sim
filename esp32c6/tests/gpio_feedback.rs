const GPIO: u32 = 0x6009_1000;
const CPU_HZ: u64 = 160000000;
macro_rules! machine {
    () => {
        esp32c6::machine([0; 6], 4 << 20)
    };
}
include!("../../tests/gpio_feedback.rs");
