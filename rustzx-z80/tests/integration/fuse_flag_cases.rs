//! The six cases where ZX Sidekick's Fuse harness found bits 3 and 5 of F differing
//! (`37_1`, `3f`, `cb4e`, `cb5e`, `cb6e`, `cb76`).
//!
//! That harness runs an older copy of Fuse's Z80 tests (1,335 cases, no MEMPTR column), from
//! before Fuse modelled Q for `SCF`/`CCF` and MEMPTR for `BIT n,(HL)`. The current Fuse tests
//! (`z80/tests` in the Fuse repository, 1,356 cases with MEMPTR) expect what this crate does; the
//! states below are restated from those cases' initial and expected values, plus `cb46_2`,
//! whose non-zero MEMPTR shows that `BIT n,(HL)` reads it. Each case starts with Q = 0 (no
//! instruction before it changed the flags) and the MEMPTR it states.

use crate::TestingBus;
use rustzx_z80::Z80;

struct Case<'a> {
    code: &'a [u8],
    af: u16,
    bc: u16,
    de: u16,
    hl: u16,
    mem_ptr: u16,
    /// A byte the case puts in memory besides the code: (address, value)
    data: Option<(u16, u8)>,
    want_af: u16,
    t: usize,
}

fn run(case: &Case) {
    let mut bus = TestingBus::new(0x10000);
    bus.load_to_memory(case.code, 0x0000);
    if let Some((addr, value)) = case.data {
        bus.load_to_memory(&[value], addr);
    }
    let mut cpu = Z80::default();
    let r = &mut cpu.regs;
    r.set_af(case.af);
    r.set_bc(case.bc);
    r.set_de(case.de);
    r.set_hl(case.hl);
    r.set_mem_ptr(case.mem_ptr);
    cpu.emulate(&mut bus);

    let r = &cpu.regs;
    assert_eq!(r.get_af(), case.want_af, "{:02x?}", case.code);
    assert_eq!(
        (r.get_bc(), r.get_de(), r.get_hl()),
        (case.bc, case.de, case.hl)
    );
    assert_eq!(r.get_pc(), case.code.len() as u16);
    // one opcode fetch for each byte of the opcode
    assert_eq!(r.get_r(), case.code.len() as u8);
    assert_eq!(
        r.get_mem_ptr(),
        case.mem_ptr,
        "none of these changes MEMPTR"
    );
    assert_eq!(bus.clocks(), case.t);
}

/// `37_1`: SCF with A = 0 and F = 0xFF. Q = 0, so bits 3 and 5 come from (Q ^ F) | A = F.
#[test]
fn scf_37_1() {
    run(&Case {
        code: &[0x37],
        af: 0x00FF,
        bc: 0,
        de: 0,
        hl: 0,
        mem_ptr: 0,
        data: None,
        want_af: 0x00ED,
        t: 4,
    });
}

/// `3f`: CCF with A = 0 and F = 0x5B.
#[test]
fn ccf_3f() {
    run(&Case {
        code: &[0x3F],
        af: 0x005B,
        bc: 0,
        de: 0,
        hl: 0,
        mem_ptr: 0,
        data: None,
        want_af: 0x0058,
        t: 4,
    });
}

/// BIT n,(HL) cases as (code, AF, BC, DE, HL, MEMPTR, byte at HL, expected AF).
type BitCase = ([u8; 2], u16, u16, u16, u16, u16, u8, u16);

fn run_bit_n_hl(cases: &[BitCase]) {
    for (code, af, bc, de, hl, mem_ptr, at_hl, want_af) in cases {
        run(&Case {
            code,
            af: *af,
            bc: *bc,
            de: *de,
            hl: *hl,
            mem_ptr: *mem_ptr,
            data: Some((*hl, *at_hl)),
            want_af: *want_af,
            t: 12,
        });
    }
}

/// `cb4e`, `cb5e`, `cb6e`, `cb76`: BIT n,(HL), with MEMPTR 0. Bits 3 and 5 come from MEMPTR's
/// high byte, not from the byte read (the older model took them from the byte: 5b would give
/// 08, 3c 28, 31 20, 18 08).
#[test]
fn bit_n_hl_cases() {
    run_bit_n_hl(&[
        (
            [0xCB, 0x4E],
            0x2600,
            0x9207,
            0x459A,
            0xADA3,
            0x0000,
            0x5B,
            0x2610,
        ),
        (
            [0xCB, 0x5E],
            0x3000,
            0xAD43,
            0x16C1,
            0x349A,
            0x0000,
            0x3C,
            0x3010,
        ),
        (
            [0xCB, 0x6E],
            0x4A00,
            0x08C9,
            0x8177,
            0xD8BA,
            0x0000,
            0x31,
            0x4A10,
        ),
        (
            [0xCB, 0x76],
            0xF800,
            0x3057,
            0x3629,
            0xBC71,
            0x0000,
            0x18,
            0xF854,
        ),
    ]);
}

/// `cb46_2`: BIT 0,(HL) with MEMPTR 0xFF00, so bits 3 and 5 are set from MEMPTR's high byte
/// though the byte read (d5) has neither.
#[test]
fn bit_n_hl_reads_mem_ptr() {
    run_bit_n_hl(&[(
        [0xCB, 0x46],
        0x7200,
        0x7AE3,
        0xA11E,
        0x6131,
        0xFF00,
        0xD5,
        0x7238,
    )]);
}
