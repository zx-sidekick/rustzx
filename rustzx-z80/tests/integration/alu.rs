//! `alu`: the instructions' arithmetic, for callers acting from outside the processor.

use crate::TestingBus;
use rustzx_z80::{alu::add16_flags, Z80};

const FLAG_C: u8 = 0x01;
const FLAG_H: u8 = 0x10;

/// Values worked out from the rules for ADD HL,ss: S, Z and P/V kept, H from bit 11's carry,
/// C from bit 15's, N reset, bits 3 and 5 from the sum's high byte.
#[test]
fn add16_flags_by_hand() {
    // carry out of bit 11 only
    assert_eq!(add16_flags(0x00, 0x0FFF, 0x0001), (0x1000, FLAG_H));
    // carry out of bits 11 and 15; Z is kept (clear), not set from the zero sum
    assert_eq!(add16_flags(0x00, 0xFFFF, 0x0001), (0x0000, FLAG_H | FLAG_C));
    // carry out of bit 15 only
    assert_eq!(add16_flags(0x00, 0x8000, 0x8000), (0x0000, FLAG_C));
    // S, Z and P/V kept; H, N, C and bits 3 and 5 all replaced
    assert_eq!(add16_flags(0xFF, 0x0000, 0x0000), (0x0000, 0xC4));
    // H is bit 11's carry, not bit 3's or bit 10's
    assert_eq!(add16_flags(0x00, 0x0800, 0x0800), (0x1000, FLAG_H));
    assert_eq!(add16_flags(0x00, 0x0400, 0x0400), (0x0800, 0x08)); // F3 from the high byte 0x08
    assert_eq!(add16_flags(0x00, 0x0080, 0x0080), (0x0100, 0x00));
    assert_eq!(add16_flags(0x00, 0x0008, 0x0008), (0x0010, 0x00));
    // C only past 0xFFFF, not at it
    assert_eq!(add16_flags(0x00, 0xFFFF, 0x0000), (0xFFFF, 0x28));
    // bits 3 and 5 from the sum's high byte, not the operands'
    assert_eq!(add16_flags(0x00, 0x2000, 0x0800), (0x2800, 0x28));
    assert_eq!(add16_flags(0x00, 0x0028, 0x0000), (0x0028, 0x00));
}

/// Runs `ADD rr,ss` (opcode `op`, after `prefix` if any) with `acc` in the destination, `operand`
/// in the source (unless they are the same register) and F = `flags`. Returns the sum and F.
fn instruction(prefix: Option<u8>, op: u8, flags: u8, acc: u16, operand: u16) -> (u16, u8) {
    let mut bus = TestingBus::new(0x10000);
    let program: Vec<u8> = prefix.into_iter().chain([op]).collect();
    bus.load_to_memory(&program, 0x8000);
    let mut cpu = Z80::default();
    let r = &mut cpu.regs;
    r.set_pc(0x8000);
    r.set_af(u16::from(flags));
    // source first, so a destination that is also the source ends up holding `acc`
    match op {
        0x09 => r.set_bc(operand),
        0x19 => r.set_de(operand),
        0x39 => r.set_sp(operand),
        _ => 0,
    };
    match prefix {
        None => r.set_hl(acc),
        Some(0xDD) => r.set_ix(acc),
        _ => r.set_iy(acc),
    };
    cpu.emulate(&mut bus);
    let sum = match prefix {
        None => cpu.regs.get_hl(),
        Some(0xDD) => cpu.regs.get_ix(),
        _ => cpu.regs.get_iy(),
    };
    (sum, cpu.regs.get_flags())
}

/// The function and the instruction agree for every source register, every prefix, every
/// starting F, and a spread of operands including the carry boundaries.
#[test]
fn add16_flags_matches_the_instruction() {
    let mut seed = 0x1234_5678_u32;
    let mut next = || {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        (seed >> 8) as u16
    };
    let mut operands = vec![
        (0x0000, 0x0000),
        (0x0FFF, 0x0001),
        (0xFFFF, 0x0001),
        (0x8000, 0x8000),
        (0x7FFF, 0x7FFF),
        (0xF800, 0x0800),
    ];
    operands.extend((0..40).map(|_| (next(), next())));

    for prefix in [None, Some(0xDD), Some(0xFD)] {
        for op in [0x09, 0x19, 0x29, 0x39] {
            for flags in 0..=255u8 {
                for &(acc, operand) in &operands {
                    // ADD rr,rr adds the register to itself
                    let operand = if op == 0x29 { acc } else { operand };
                    assert_eq!(
                        instruction(prefix, op, flags, acc, operand),
                        add16_flags(flags, acc, operand),
                        "prefix {prefix:02x?} op {op:02x} F {flags:02x} {acc:04x} + {operand:04x}"
                    );
                }
            }
        }
    }
}
