use super::*;

/// Retained owner indices must not survive the permutation made by arena turnover.
/// Two live regions may contain the same successor; coverage chooses the first owner.
#[no_mangle]
pub(super) extern "C" fn esp32sim_test_region_ownership() -> u32 {
    let mut ram = Ram::new(true, false);
    let mut program = vec![0; 80];
    for off in [0, 8, 16, 64] {
        let mut chunk = asm::addi_n(2, 2, 1);
        chunk.extend(asm::j(BASE + off + 2, BASE + 16));
        program[off as usize..off as usize + chunk.len()].copy_from_slice(&chunk);
    }
    ram.ram.mem[..program.len()].copy_from_slice(&program);
    let mut cc = CodeCache::new(0).unwrap();
    let mut c = cpu(0);
    c.ps = 0;
    // Old order: unrelated, first owner, overlapping owner, shared dispatch.
    for off in [64, 0, 8, 16] {
        let mut pc = BASE + off;
        let mut instructions: Vec<BlockInsn> = (0..2).map(|_| {
            let insn = crate::decode::decode(pc, ram.fetch(pc).unwrap());
            pc += insn.len as u32;
            BlockInsn { insn, max_ar: crate::exec::max_ar(&insn), straddle: false, off: 0 }
        }).collect();
        queue(&mut cc, &mut instructions, BASE + off, true);
    }
    for id in [1usize, 2] {
        let b = &cc.blocks[id];
        let f = emitter::region::form(&c, &mut ram, b.pc, &b.instructions, true).unwrap();
        assert!(f.chunks.iter().any(|chunk| chunk.pc == BASE + 16));
        let (bytes, sites) = emitter::region::generate(&f.chunks, &f.pages, &f.loops, &f.leaves, true, None);
        let slot = unsafe { host_jit_compile(bytes.as_ptr(), bytes.len()) };
        assert_ne!(slot, 0);
        let r = Region {
            lens: f.chunks.iter().map(|chunk| chunk.instructions.len() as u32).collect(),
            copies: vec![0; f.chunks.len()], short: Vec::new(), tunes: 0,
            chunks: f.chunks, leaves: f.leaves, slot, bytes: bytes.len(),
            bloom: f.bloom, lo: f.lo, hi: f.hi, loops: f.loops, pages: f.pages, sites,
        };
        for (k, chunk) in r.chunks.iter().enumerate() {
            cc.covered.borrow_mut().entry(chunk.pc).or_insert((id as u32, k as u32));
        }
        *b.region.borrow_mut() = Some(r);
    }
    let owner = cc.covered.borrow()[&(BASE + 16)];
    assert_eq!(owner.0, 1);
    // A coverage map is a preference for new lookups, not exclusive ownership.
    // Model a live hint retained after overlapping coverage acquired a new owner.
    let overlap_k = cc.blocks[2].region.borrow().as_ref().unwrap().chunks.iter()
        .position(|chunk| chunk.pc == BASE + 16).unwrap() as u32;
    cc.blocks[3].covered_by.set((2, overlap_k));
    for _ in 0..2 {
        c.pc = BASE + 16;
        let before = c.get_ar(2);
        let fm = ram.fast_mem();
        let result = unsafe { run(&cc, 3, &mut c, &mut ram, &Helpers::new::<Ram>(), 2, 0, fm) };
        assert_eq!(result & 0xffff, 2);
        assert_eq!(c.get_ar(2), before.wrapping_add(1));
        assert_eq!(c.pc, BASE + 16);
    }
    cc.blocks[3].covered_by.set(owner);
    // Recent owners sort ahead of the unrelated block, changing both indices.
    cc.generation = 1;
    cc.blocks[1].generation = 1;
    cc.blocks[2].generation = 1;
    let epoch = cc.region_epoch.get();
    cc.reset();
    assert!(cc.region_epoch.get() > epoch);
    let code = cc.by_pc[&(BASE + 16, 2, true)];
    assert_eq!(cc.covered.borrow()[&(BASE + 16)].0, 0);
    assert_eq!(cc.blocks[code as usize].covered_by.get(), (NONE, 0),
        "reset must discard owner indices before rebuilding coverage");
    // A valid-looking stale hint would name owner 1, which also contains the shared PC.
    // The first dispatch fills Hot; the second checks it against canonical coverage.
    for _ in 0..2 {
        c.pc = BASE + 16;
        let fm = ram.fast_mem();
        let result = unsafe { run(&cc, code, &mut c, &mut ram, &Helpers::new::<Ram>(), 2, 0, fm) };
        assert_eq!(result & 0xffff, 2);
        let canonical = cc.blocks[0].region.borrow();
        assert_eq!(cc.recs[code as usize].hot.get().slot, canonical.as_ref().unwrap().slot,
            "reset must discard owner indices before rebuilding coverage");
    }
    1
}
