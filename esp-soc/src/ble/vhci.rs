//! Controller lifecycle and legacy VHCI packet delivery in a guest FreeRTOS task.
use emu_core::Core;
use std::collections::HashMap;

pub const INIT: u32 = 48;
pub const TASK: u32 = 112;
pub const POLL: u32 = 176;
pub const CREATED: u32 = 192;
const BUFFER_SIZE: u32 = 512;
const INVALID_STATE: u32 = 0x103;
const INVALID_ARG: u32 = 0x102;

pub struct Abi {
    pub code: &'static [u8],
    pub ram: std::ops::Range<u32>,
    pub hz: u64,
}

pub trait VhciBus: crate::SocBus {
    fn abi() -> Abi;
    fn ble(&mut self) -> &mut Ble;
    fn valid_callback(&mut self, pc: u32) -> bool;
    fn flash_offset(&mut self, pc: u32) -> Option<usize>;
    fn ble_debug(&self) -> bool;
}

#[derive(Clone, Copy)]
enum Hook {
    Init,
    Created,
    Poll,
    Enable,
    Disable,
    Deinit,
    Status,
    Release,
    Register,
    Available,
    Send,
    PowerGet,
    PowerSet,
}

#[derive(Default)]
pub struct Ble {
    pub controller: crate::ble::Controller,
    pub hooks: Vec<u32>,
    kinds: HashMap<u32, Hook>,
    entry: u32,
    pub init: u32,
    pub create: u32,
    pub delay: u32,
    pub buffer: u32,
    pub status: u32,
    pub callbacks: [u32; 2],
    pub send_ready: bool,
    yield_to_host: bool,
    pub task_created: bool,
    pub original_flash: Option<(usize, Vec<u8>)>,
}

impl Ble {
    pub fn enable(&mut self, symbols: &HashMap<String, u32>, abi: &Abi) -> Result<(), String> {
        let get = |name: &str| {
            symbols
                .get(name)
                .copied()
                .ok_or_else(|| format!("BLE requires ELF symbol {name}"))
        };
        let entry = get("esp_bt_controller_init")?;
        let init = entry.checked_add(3).ok_or("BLE invalid init address")? & !3;
        let next = symbols
            .values()
            .copied()
            .filter(|&pc| pc > entry)
            .min()
            .unwrap_or(init);
        if next.saturating_sub(init) < INIT + abi.code.len() as u32 {
            return Err("BLE controller init has no room for the guest task trampoline".into());
        }
        let buffer = get("_bt_controller_bss_start")?;
        let end = get("_bt_controller_bss_end")?;
        if !abi.ram.contains(&buffer)
            || end > abi.ram.end
            || buffer & 3 != 0
            || end.saturating_sub(buffer) < BUFFER_SIZE
        {
            return Err("BLE controller BSS is too small or outside DRAM".into());
        }
        let mut next = Self {
            entry,
            init,
            buffer,
            create: get("xTaskCreatePinnedToCore")?,
            delay: get("vTaskDelay")?,
            ..Self::default()
        };
        for (name, kind) in [
            ("esp_bt_controller_init", Hook::Init),
            ("esp_bt_controller_enable", Hook::Enable),
            ("esp_vhci_host_register_callback", Hook::Register),
            ("esp_vhci_host_check_send_available", Hook::Available),
            ("esp_vhci_host_send_packet", Hook::Send),
        ] {
            next.kinds.insert(get(name)?, kind);
        }
        for (name, kind) in [
            ("esp_bt_controller_get_status", Hook::Status),
            ("esp_bt_controller_rom_mem_release", Hook::Release),
            ("esp_bt_controller_disable", Hook::Disable),
            ("esp_bt_controller_deinit", Hook::Deinit),
            ("esp_bt_controller_mem_release", Hook::Release),
            ("esp_bt_mem_release", Hook::Release),
            ("esp_ble_tx_power_get", Hook::PowerGet),
            ("esp_ble_tx_power_set", Hook::PowerSet),
        ] {
            if let Some(&address) = symbols.get(name) {
                next.kinds.insert(address, kind);
            }
        }
        next.kinds.insert(init + POLL, Hook::Poll);
        next.kinds.insert(init + CREATED, Hook::Created);
        next.hooks = next.kinds.keys().copied().collect();
        *self = next;
        Ok(())
    }

    pub fn reset(&mut self) {
        self.controller = crate::ble::Controller::new();
        self.status = 0;
        self.callbacks = [0; 2];
        self.send_ready = false;
        self.yield_to_host = false;
        self.task_created = false;
    }

    pub fn command(&mut self, command: &str, cycles: u64, hz: u64) -> Result<(), String> {
        if self.hooks.is_empty() {
            return Err("BLE requires --ble and the application ELF".into());
        }
        let result = self.controller.command(command);
        self.log(cycles, hz);
        result
    }

    fn log(&mut self, cycles: u64, hz: u64) {
        for line in self.controller.drain_log() {
            eprintln!("[ble] t={:.6}s {line}", cycles as f64 / hz as f64);
        }
    }
}

pub fn install<B: VhciBus>(bus: &mut B) -> Result<(), String> {
    let abi = B::abi();
    let b = bus.ble();
    let (entry, init, buffer) = (b.entry, b.init, b.buffer);
    let literals = [
        b.create,
        init + TASK,
        buffer + 4,
        b.delay,
        init + POLL,
        init + CREATED,
        buffer,
        4096,
    ];
    if !bus.valid_callback(entry) {
        return Err("BLE controller init does not match the guest calling convention".into());
    }
    if let Some(offset) = bus.flash_offset(init + 4) {
        let len = INIT as usize + abi.code.len() - 4;
        if bus.flash_offset(init + 4 + len as u32 - 1) != Some(offset + len - 1) {
            return Err("BLE controller init crosses a noncontiguous flash mapping".into());
        }
        if bus.ble().original_flash.is_none() {
            let original = (0..len)
                .map(|i| bus.read8_unpriced(init + 4 + i as u32))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "BLE cannot read trampoline memory")?;
            bus.ble().original_flash = Some((offset, original));
        }
    }
    for (i, value) in literals.into_iter().enumerate() {
        bus.load_bytes(init + 4 + i as u32 * 4, &value.to_le_bytes())?;
    }
    bus.load_bytes(init + INIT, abi.code)?;
    bus.load_bytes(buffer + 4, b"vhci\0")?;
    Ok(())
}

pub fn intercept<C: Core, B: VhciBus>(
    cpu: &mut C,
    bus: &mut B,
    redirect: impl FnOnce(&mut C, u32),
) -> bool {
    let abi = B::abi();
    let Some(&hook) = bus.ble().kinds.get(&cpu.pc()) else {
        return false;
    };
    let arg = cpu.arg(bus, 0);
    let mut result = 0;
    match hook {
        Hook::Init => {
            if bus.ble().status != 0 {
                result = INVALID_STATE;
            } else if arg == 0 {
                result = INVALID_ARG;
            } else if bus.ble().task_created {
                bus.ble().status = 1;
            } else {
                match install(bus) {
                    Ok(()) => {
                        cpu.flush_caches();
                        redirect(cpu, bus.ble().init + INIT);
                        return true;
                    }
                    Err(error) => {
                        eprintln!("[ble] {error}");
                        result = u32::MAX;
                    }
                }
            }
        }
        Hook::Created => {
            if arg == 1 {
                bus.ble().task_created = true;
                bus.ble().status = 1;
            } else {
                eprintln!("[ble] controller task creation failed: {arg:#x}");
                result = u32::MAX;
            }
        }
        Hook::Enable => {
            if bus.ble().status != 1 {
                result = INVALID_STATE;
            } else if arg != 1 && arg != 3 {
                result = INVALID_ARG;
            } else {
                bus.ble().status = 2;
            }
        }
        Hook::Disable => {
            if bus.ble().status != 2 {
                result = INVALID_STATE;
            } else {
                bus.ble().status = 1;
            }
        }
        Hook::Deinit => {
            if bus.ble().status != 1 {
                result = INVALID_STATE;
            } else {
                bus.ble().status = 0;
                bus.ble().callbacks = [0; 2];
                bus.ble().send_ready = false;
                bus.ble().yield_to_host = false;
                // ponytail: keep one task per boot; delete it when memory release is modeled.
                bus.ble().controller = crate::ble::Controller::new();
            }
        }
        Hook::Status => result = bus.ble().status,
        // The injected task and packet buffer occupy controller memory for this boot.
        Hook::Release => {
            if bus.ble().status != 0 {
                result = INVALID_STATE;
            }
        }
        Hook::Register => {
            if arg == 0 || arg > u32::MAX - 7 {
                result = INVALID_ARG;
            } else if let (Ok(send), Ok(recv)) =
                (bus.read32_unpriced(arg), bus.read32_unpriced(arg + 4))
            {
                if !bus.valid_callback(send) || !bus.valid_callback(recv) {
                    result = INVALID_ARG;
                } else {
                    bus.ble().callbacks = [send, recv];
                    bus.ble().send_ready = true;
                }
            } else {
                result = INVALID_ARG;
            }
        }
        Hook::Available => result = u32::from(bus.ble().status == 2),
        Hook::Send => {
            let len = cpu.arg(bus, 1) as usize;
            if bus.ble().status != 2
                || len == 0
                || len > (BUFFER_SIZE - 16) as usize
                || arg < abi.ram.start
                || arg
                    .checked_add(len as u32)
                    .is_none_or(|end| end > abi.ram.end)
            {
                eprintln!("[ble] rejected invalid VHCI packet pointer, length or controller state");
            } else {
                let packet: Result<Vec<u8>, _> = (0..len)
                    .map(|i| bus.read8_unpriced(arg + i as u32))
                    .collect();
                if let Ok(packet) = packet {
                    if bus.ble_debug() {
                        eprintln!("[ble] tx {:02x?}", packet);
                    }
                    bus.ble().controller.send(&packet);
                    bus.ble().send_ready = true;
                    let cycles = bus.cycles();
                    bus.ble().log(cycles, abi.hz);
                }
            }
        }
        Hook::Poll => {
            // A command-complete callback wakes the host. Let it finish the operation
            // before delivering the next event, especially scan-enable followed by reports.
            if std::mem::take(&mut bus.ble().yield_to_host) {
                cpu.return_from_stub(bus, 0);
                return true;
            }
            if bus.ble().status == 2 && bus.ble().callbacks[1] != 0 {
                if let Some(packet) = bus.ble().controller.pop_packet() {
                    let buffer = bus.ble().buffer;
                    if packet.len() <= (BUFFER_SIZE - 16) as usize
                        && bus
                            .load_bytes(buffer, &(packet.len() as u32).to_le_bytes())
                            .is_ok()
                        && bus.load_bytes(buffer + 16, &packet).is_ok()
                    {
                        if bus.ble_debug() {
                            eprintln!("[ble] rx {:02x?}", packet);
                        }
                        bus.ble().yield_to_host = true;
                        result = bus.ble().callbacks[1];
                    } else {
                        eprintln!("[ble] rejected oversized or unmapped controller packet");
                    }
                } else if bus.ble().send_ready {
                    bus.ble().send_ready = false;
                    result = bus.ble().callbacks[0];
                }
            }
        }
        Hook::PowerGet => result = 5,
        Hook::PowerSet => {}
    }
    cpu.return_from_stub(bus, result);
    true
}
