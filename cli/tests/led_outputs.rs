use emu_core::Bus;
use esp_soc::BoardModel;
use std::sync::{Arc, Mutex};

type Frames = Arc<Mutex<Vec<(u8, Vec<bool>)>>>;
struct Capture(Frames);
impl BoardModel for Capture {
    fn name(&self) -> &'static str { "led-output-test" }
    fn rmt_frame(&mut self, pin: u8, bits: &[bool]) { self.0.lock().unwrap().push((pin, bits.to_vec())); }
}

#[test]
fn rmt_fans_out_to_enabled_routes_on_each_chip() {
    macro_rules! check {
        ($bus:ident, $rmt:expr, $gpio:expr, $mux:expr, $sig:expr, $oen:expr) => {{
            let frames = Frames::default();
            $bus.board = Box::new(Capture(frames.clone()));
            $bus.attach_board_devices();
            for pin in [1, 2, 3] {
                $bus.write32($gpio + 0x554 + pin * 4, $sig | $oen).unwrap();
                $bus.write32($mux + 4 + pin * 4, if pin == 3 { 0 } else { 1 << 12 }).unwrap();
            }
            $bus.write32($gpio + 0x24, 14).unwrap();
            $rmt.done.push((0, vec![true, false]));
            $bus.tick(32768);
            assert_eq!(*frames.lock().unwrap(), [(1, vec![true, false]), (2, vec![true, false])]);
            frames.lock().unwrap().clear();
            $bus.write32($gpio + 0x28, 2).unwrap();
            $rmt.done.push((0, vec![false, true]));
            $bus.tick(32768);
            assert_eq!(*frames.lock().unwrap(), [(2, vec![false, true])]);
            frames.lock().unwrap().clear();
            $bus.write32($gpio + 0x554 + 8, $sig | $oen | ($oen >> 1)).unwrap();
            $rmt.done.push((0, vec![true]));
            $bus.tick(32768);
            assert!(frames.lock().unwrap().is_empty());
        }};
    }
    let mut bus = esp32s3::bus::SocBus::new(1024, 0, [0; 6]);
    check!(bus, bus.periph.rmt, 0x6000_4000, 0x6000_9000, 81, 1024);
    let mut bus = esp32c3::bus::SocBus::new(1024, [0; 6]);
    check!(bus, bus.periph.rmt.rmt, 0x6000_4000, 0x6000_9000, 51, 512);
    let mut bus = esp32c6::bus::SocBus::new(1024, [0; 6]);
    check!(bus, bus.periph.rmt.rmt, 0x6009_1000, 0x6009_0000, 71, 512);
}
