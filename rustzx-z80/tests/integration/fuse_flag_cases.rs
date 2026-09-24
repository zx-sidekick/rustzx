//! The six cases where ZX Sidekick's Fuse harness found bits 3 and 5 of F differing
//! (`37_1`, `3f`, `cb4e`, `cb5e`, `cb6e`, `cb76`).
//!
//! That harness runs an older copy of Fuse's Z80 tests (1,335 cases, no MEMPTR column), from
//! before Fuse modelled Q for `SCF`/`CCF` and MEMPTR for `BIT n,(HL)`. The current Fuse tests
//! (`z80/tests` in the Fuse repository, 1,356 cases with MEMPTR) expect what this crate does; the
//! states below are those cases' initial and expected values. Each case starts with Q = 0 (no
//! instruction before it changed the flags) and the MEMPTR it states.

use crate::TestingBus;
use rustzx_z80::Z80;

struct Case {
    code: &'static [u8],
    af: u16,
    bc: u16,
    de: u16,
    hl: u16,
    /// The byte at HL
    at_hl: u8,
    want_af: u16,
    t: usize,
}

fn run(case: &Case) {
    let mut bus = TestingBus::new(0x10000);
    bus.load_to_memory(case.code, 0x0000);
    if case.hl != 0 {
        bus.load_to_memory(&[case.at_hl], case.hl);
    }
    let mut cpu = Z80::default();
    let r = &mut cpu.regs;
    r.set_af(case.af);
    r.set_bc(case.bc);
    r.set_de(case.de);
    r.set_hl(case.hl);
    r.set_mem_ptr(0x0000);
    cpu.emulate(&mut bus);

    assert_eq!(cpu.regs.get_af(), case.want_af, "{:02x?}", case.code);
    assert_eq!(cpu.regs.get_pc(), case.code.len() as u16);
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
        at_hl: 0,
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
        at_hl: 0,
        want_af: 0x0058,
        t: 4,
    });
}

/// `cb4e`, `cb5e`, `cb6e`, `cb76`: BIT n,(HL). Bits 3 and 5 come from MEMPTR's high byte, which
/// the cases state as 0.
#[test]
fn bit_n_hl_cases() {
    for case in [
        Case {
            code: &[0xCB, 0x4E],
            af: 0x2600,
            bc: 0x9207,
            de: 0x459A,
            hl: 0xADA3,
            at_hl: 0x5B,
            want_af: 0x2610,
            t: 12,
        },
        Case {
            code: &[0xCB, 0x5E],
            af: 0x3000,
            bc: 0xAD43,
            de: 0x16C1,
            hl: 0x349A,
            at_hl: 0x3C,
            want_af: 0x3010,
            t: 12,
        },
        Case {
            code: &[0xCB, 0x6E],
            af: 0x4A00,
            bc: 0x08C9,
            de: 0x8177,
            hl: 0xD8BA,
            at_hl: 0x31,
            want_af: 0x4A10,
            t: 12,
        },
        Case {
            code: &[0xCB, 0x76],
            af: 0xF800,
            bc: 0x3057,
            de: 0x3629,
            hl: 0xBC71,
            at_hl: 0x18,
            want_af: 0xF854,
            t: 12,
        },
    ] {
        run(&case);
    }
}
