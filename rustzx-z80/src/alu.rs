//! The arithmetic of some Z80 instructions, for a caller that acts on the processor from outside
//! and has to leave the flags exactly as those instructions would.
//!
//! The instructions themselves call these functions, so the two cannot drift apart.

use crate::{
    tables::{lookup16_r12, F3F5_TABLE, HALF_CARRY_ADD_TABLE},
    FLAG_CARRY, FLAG_PV, FLAG_SIGN, FLAG_ZERO,
};

/// The 16-bit add of `ADD HL,ss`, `ADD IX,ss` and `ADD IY,ss`: returns `a + b` (wrapping) and the
/// flags it leaves, given the flags before it.
///
/// S, Z and P/V are kept from `flags`; H is the carry out of bit 11 and C the carry out of bit
/// 15; N is reset; bits 3 and 5 are those of the sum's high byte.
///
/// How to store the flags depends on what the caller stands in for, because a following `SCF`
/// or `CCF` depends on whether the last instruction changed F (the Z80's Q):
/// - for the add itself, as the last instruction that ran, use
///   [`Regs::set_flags`](crate::Regs::set_flags), which records the change as the instruction
///   does;
/// - for a routine whose last instruction leaves F alone (one ending in `POP BC; RET`, say), use
///   [`Regs::set_reg_8`](crate::Regs::set_reg_8) with [`RegName8::F`](crate::RegName8::F), which
///   doesn't.
///
/// The instruction also sets MEMPTR to the first operand + 1, which this leaves to the caller.
#[must_use]
pub fn add16_flags(flags: u8, a: u16, b: u16) -> (u16, u8) {
    let sum = u32::from(a) + u32::from(b);
    let lookup = lookup16_r12(a, b, sum as u16);
    let mut f = flags & (FLAG_ZERO | FLAG_PV | FLAG_SIGN);
    f |= HALF_CARRY_ADD_TABLE[(lookup & 0x07) as usize];
    f |= u8::from(sum > 0xFFFF) * FLAG_CARRY;
    f |= F3F5_TABLE[((sum >> 8) as u8) as usize];
    (sum as u16, f)
}
