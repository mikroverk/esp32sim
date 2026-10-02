//! S3 windowed-Xtensa calling glue for the virtual VHCI controller.
use crate::bus::{SocBus, DRAM_HIGH, DRAM_LOW};
use emu_core::Bus;
use esp_soc::ble::vhci::{Abi, Ble, VhciBus};
use xtensa_lx7::decode::{decode, Op};

pub fn intercept(cpu: &mut xtensa_lx7::Cpu, bus: &mut SocBus) -> bool {
    esp_soc::ble::vhci::intercept(cpu, bus, |cpu, pc| {
        cpu.pc = pc;
        cpu.insn_count += 1;
        emu_core::Core::advance_cycles(cpu, 1);
    })
}

impl VhciBus for SocBus {
    fn abi() -> Abi {
        Abi {
            code: CODE,
            ram: DRAM_LOW..DRAM_HIGH,
            hz: crate::periph::CPU_HZ,
        }
    }
    fn ble(&mut self) -> &mut Ble {
        &mut self.ble
    }
    fn valid_callback(&mut self, pc: u32) -> bool {
        self.fetch(pc)
            .is_ok_and(|bytes| decode(pc, bytes).op == Op::Entry)
    }
    fn flash_offset(&mut self, pc: u32) -> Option<usize> {
        self.flash_off(pc)
    }
    fn ble_debug(&self) -> bool {
        self.debug.has("ble")
    }
}

// Xtensa little-endian, no-transform assembly, relative to controller_init.
// Literals at +4: create, task, name, delay, poll, created, buffer, stack size.
// +48: entry a1,64; l32r a8,create; l32r a10,task; l32r a11,name;
// l32r a12,stack; movi a13,0; movi a14,23; movi a15,0; movi a9,0;
// s32i a9,a1,0; callx8 a8; l32r a8,created; callx8 a8; mov a2,a10; retw.
// +112: entry a1,48; again: l32r a8,poll; callx8 a8; beqz a10,sleep;
// mov a8,a10; l32r a10,buffer; l32i a11,a10,0; addi a10,a10,16;
// callx8 a8; j again; sleep: movi a10,1; l32r a8,delay; callx8 a8; j again.
// +176/+192: entry a1,32; retw. These two entries are host-intercepted.
const CODE: &[u8] = &[
    0x36, 0x81, 0x00, 0x81, 0xf4, 0xff, 0xa1, 0xf4, 0xff, 0xb1, 0xf4, 0xff, 0xc1, 0xf9, 0xff, 0xd2,
    0xa0, 0x00, 0xe2, 0xa0, 0x17, 0xf2, 0xa0, 0x00, 0x92, 0xa0, 0x00, 0x92, 0x61, 0x00, 0xe0, 0x08,
    0x00, 0x81, 0xf1, 0xff, 0xe0, 0x08, 0x00, 0xa0, 0x2a, 0x20, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x36, 0x61, 0x00, 0x81, 0xe8, 0xff, 0xe0, 0x08, 0x00, 0x16, 0x1a, 0x01, 0xa0, 0x8a, 0x20, 0xa1,
    0xe7, 0xff, 0xb2, 0x2a, 0x00, 0xa2, 0xca, 0x10, 0xe0, 0x08, 0x00, 0x06, 0xf9, 0xff, 0xa2, 0xa0,
    0x01, 0x81, 0xdf, 0xff, 0xe0, 0x08, 0x00, 0x06, 0xf6, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x36, 0x41, 0x00, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x36, 0x41, 0x00, 0x90, 0x00, 0x00,
];

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Core;
    use esp_soc::ble::vhci::{install, INIT, TASK};
    use std::collections::HashMap;
    use xtensa_lx7::Cpu;
    const INVALID_ARG: u32 = 0x102;
    use xtensa_lx7::state::ps;

    const BASE: u32 = 0x4038_8000;
    const RETURN: u32 = 0x4038_9000;
    const SEND_CB: u32 = BASE + 0x700;
    const RECV_CB: u32 = BASE + 0x710;

    fn fixture() -> (SocBus, HashMap<String, u32>) {
        let mut bus = SocBus::new(0x400000, 0, [0; 6]);
        let symbols: HashMap<String, u32> = [
            ("esp_bt_controller_init", BASE),
            ("xTaskCreatePinnedToCore", BASE + 0x400),
            ("vTaskDelay", BASE + 0x500),
            ("esp_bt_controller_get_status", BASE + 0x600),
            ("esp_bt_controller_enable", BASE + 0x610),
            ("esp_vhci_host_check_send_available", BASE + 0x620),
            ("esp_vhci_host_send_packet", BASE + 0x630),
            ("esp_vhci_host_register_callback", BASE + 0x640),
            ("_bt_controller_bss_start", 0x3fc9_0000),
            ("_bt_controller_bss_end", 0x3fc9_1000),
        ]
        .into_iter()
        .map(|(n, a)| (n.into(), a))
        .collect();
        for &pc in symbols.values().filter(|&&pc| pc >= BASE) {
            bus.load_bytes(pc, &[0x36, 0x41, 0, 0x90, 0, 0]).unwrap();
        }
        for pc in [SEND_CB, RECV_CB] {
            bus.load_bytes(pc, &[0x36, 0x41, 0, 0x90, 0, 0]).unwrap();
        }
        bus.ble.enable(&symbols, &SocBus::abi()).unwrap();
        (bus, symbols)
    }

    fn caller(pc: u32, arg: u32) -> Cpu {
        let mut cpu = Cpu::new(0);
        cpu.ps = ps::WOE | ps::UM | (2 << ps::CALLINC_SHIFT);
        cpu.windowbase = 0;
        cpu.windowstart = 1;
        cpu.pc = pc;
        cpu.set_ar(1, 0x3fcf_0000);
        cpu.set_ar(8, (2 << 30) | (RETURN & 0x3fff_ffff));
        cpu.set_ar(10, arg);
        cpu
    }

    #[test]
    fn trampoline_creates_guest_task_with_windowed_abi_and_returns_esp_ok() {
        let (mut bus, _) = fixture();
        let mut cpu = caller(BASE, DRAM_LOW);
        assert!(intercept(&mut cpu, &mut bus));
        let mut created = false;
        for _ in 0..100 {
            if cpu.pc == RETURN {
                break;
            }
            if cpu.pc == bus.ble.create {
                let args: Vec<u32> = (0..6).map(|n| cpu.arg(&mut bus, n)).collect();
                assert_eq!(args, [BASE + TASK, bus.ble.buffer + 4, 4096, 0, 23, 0]);
                assert_eq!(
                    bus.read32_unpriced(cpu.get_ar(1)).unwrap(),
                    0,
                    "core ID is seventh stack argument"
                );
                cpu.return_from_stub(&mut bus, 1);
                created = true;
            } else if !intercept(&mut cpu, &mut bus) {
                assert!(cpu.step(&mut bus).trap().is_none());
            }
        }
        assert!(created);
        assert_eq!(cpu.pc, RETURN);
        assert_eq!(cpu.get_ar(10), 0);
        assert_eq!(cpu.windowbase, 0);
        assert_eq!(bus.ble.status, 1);
        assert!(bus.ble.task_created);
        let hooks = bus.ble.hooks.clone();
        bus.ble.reset();
        assert_eq!(bus.ble.status, 0);
        assert!(!bus.ble.task_created);
        assert_eq!(bus.ble.hooks, hooks);
    }

    #[test]
    fn guest_task_calls_receive_and_send_ready_then_delays() {
        let (mut bus, _) = fixture();
        install(&mut bus).unwrap();
        bus.ble.status = 2;
        bus.ble.callbacks = [SEND_CB, RECV_CB];
        bus.ble.send_ready = true;
        bus.ble.controller.send(&[1, 3, 12, 0]);
        let mut cpu = caller(BASE + TASK, 0);
        let (mut received, mut ready, mut delayed) = (false, false, false);
        for _ in 0..100 {
            if cpu.pc == RECV_CB {
                let (ptr, len) = (cpu.arg(&mut bus, 0), cpu.arg(&mut bus, 1));
                let bytes: Vec<u8> = (0..len)
                    .map(|i| bus.read8_unpriced(ptr + i).unwrap())
                    .collect();
                assert_eq!(bytes, [4, 14, 4, 1, 3, 12, 0]);
                cpu.return_from_stub(&mut bus, 0);
                received = true;
            } else if cpu.pc == SEND_CB {
                assert!(delayed, "host must run after receiving a packet");
                cpu.return_from_stub(&mut bus, 0);
                ready = true;
            } else if cpu.pc == bus.ble.delay {
                assert_eq!(cpu.arg(&mut bus, 0), 1);
                delayed = true;
                if ready {
                    break;
                }
                cpu.return_from_stub(&mut bus, 0);
            } else if !intercept(&mut cpu, &mut bus) {
                assert!(cpu.step(&mut bus).trap().is_none());
            }
        }
        assert!(received && ready && delayed);
    }

    #[test]
    fn invalid_guest_buffers_and_missing_symbols_are_rejected() {
        let (mut bus, mut symbols) = fixture();
        let mut cpu = caller(symbols["esp_vhci_host_register_callback"], 0xffff_fffc);
        assert!(intercept(&mut cpu, &mut bus));
        assert_eq!(cpu.get_ar(10), INVALID_ARG);
        let table = 0x3fc9_2000;
        bus.load_bytes(table, &SEND_CB.to_le_bytes()).unwrap();
        bus.load_bytes(table + 4, &RECV_CB.to_le_bytes()).unwrap();
        let mut cpu = caller(symbols["esp_vhci_host_register_callback"], table);
        assert!(intercept(&mut cpu, &mut bus));
        assert_eq!(
            cpu.get_ar(10),
            0,
            "a static callback table can live in DROM"
        );
        assert_eq!(bus.ble.callbacks, [SEND_CB, RECV_CB]);
        symbols.remove("vTaskDelay");
        assert!(bus
            .ble
            .enable(&symbols, &SocBus::abi())
            .unwrap_err()
            .contains("vTaskDelay"));
    }

    #[test]
    fn machine_dispatches_chip_hooks_as_function_boundaries() {
        let (bus, symbols) = fixture();
        let mut m = esp_soc::Machine::<crate::S3>::new([0; 6], bus);
        m.quantum = 1;
        m.bus.ble.status = 2;
        m.cores[0] = caller(symbols["esp_bt_controller_get_status"], 0);
        assert!(matches!(m.run(1), esp_soc::Stop::MaxInsns));
        assert_eq!(m.cores[0].pc, RETURN);
        assert_eq!(m.cores[0].get_ar(10), 2);
    }
    #[test]
    fn reboot_restores_controller_flash_even_after_mmu_mapping_changes() {
        let (mut bus, symbols) = fixture();
        let init = crate::bus::IBUS_LOW;
        let symbols = symbols
            .into_iter()
            .map(|(name, address)| {
                (
                    name,
                    if address >= BASE {
                        address - BASE + init
                    } else {
                        address
                    },
                )
            })
            .collect();
        bus.ble.enable(&symbols, &SocBus::abi()).unwrap();
        let slot = ((init & 0x1ff_ffff) >> 16) as usize;
        bus.mmu[slot] = 0;
        let mut original: Vec<u8> = (0..INIT as usize + CODE.len()).map(|i| i as u8).collect();
        original[..3].copy_from_slice(&[0x36, 0x41, 0]);
        bus.load_bytes(init, &original).unwrap();
        install(&mut bus).unwrap();
        assert_ne!(&bus.flash[..original.len()], &original);
        bus.mmu[slot] = 1;
        esp_soc::SocBus::reboot(&mut bus, [0; 6]);
        assert_eq!(&bus.flash[..original.len()], &original);
        assert!(bus.ble.original_flash.is_none());
        assert!(!bus.ble.hooks.is_empty());
    }
}
