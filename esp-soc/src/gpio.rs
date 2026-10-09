//! Board GPIO input delivery shared by the chip buses.
use crate::board::BoardModel;
use esp_periph::Gpio;

#[inline(always)]
pub fn release_and_report(gpio: &mut Gpio, events: &mut Option<Vec<(u64, u8, bool)>>, cycle: u64, pin: u8) -> bool {
    let changed = gpio.release_input(pin);
    if changed {
        if let Some(events) = events { events.push((cycle, pin, gpio.level(pin))); }
    }
    changed
}

#[inline(always)]
pub fn deliver_board_inputs(board: &mut dyn BoardModel, gpio: &mut Gpio, events: &mut Option<Vec<(u64, u8, bool)>>, cycle: u64) -> bool {
    board.advance_to(cycle);
    let mut changed = false;
    for edge in board.take_edges() {
        changed |= gpio.set_input(edge.pin, edge.level);
        if let Some(events) = events { events.push((edge.cycle, edge.pin, edge.level)); }
    }
    for pin in board.released_inputs() { changed |= release_and_report(gpio, events, cycle, pin); }
    changed
}
