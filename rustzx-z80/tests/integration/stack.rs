//! `Z80::push`, `pop` and `ret`: the stack operations, for callers acting between instructions.

use crate::{snapshot, Event, TestingBus};
use rustzx_z80::Z80;

const PUSH_BC: u8 = 0xC5;
const RET: u8 = 0xC9;
const SCF: u8 = 0x37;

fn machine(sp: u16) -> (Z80, TestingBus) {
    let mut bus = TestingBus::new(0x10000);
    bus.record_events();
    let mut cpu = Z80::default();
    cpu.regs.set_sp(sp);
    cpu.regs.set_pc(0x8000);
    (cpu, bus)
}

#[test]
fn push_stores_high_byte_above_low_byte() {
    let (mut cpu, mut bus) = machine(0xC000);

    cpu.push(&mut bus, 0x1234);

    assert_eq!(cpu.regs.get_sp(), 0xBFFE);
    assert_eq!(bus.memory()[0xBFFE..0xC000], [0x34, 0x12]);
}

#[test]
fn pop_reverses_push() {
    let (mut cpu, mut bus) = machine(0xC000);

    cpu.push(&mut bus, 0x1234);
    cpu.push(&mut bus, 0xABCD);

    assert_eq!(cpu.pop(&mut bus), 0xABCD);
    assert_eq!(cpu.pop(&mut bus), 0x1234);
    assert_eq!(cpu.regs.get_sp(), 0xC000);
}

#[test]
fn stack_wraps_around_memory() {
    let (mut cpu, mut bus) = machine(0x0000);

    cpu.push(&mut bus, 0x1234);
    assert_eq!(cpu.regs.get_sp(), 0xFFFE);
    assert_eq!((bus.memory()[0xFFFE], bus.memory()[0xFFFF]), (0x34, 0x12));

    cpu.regs.set_sp(0xFFFF);
    bus.load_to_memory(&[0x78], 0xFFFF);
    bus.load_to_memory(&[0x56], 0x0000);
    assert_eq!(cpu.pop(&mut bus), 0x5678);
    assert_eq!(cpu.regs.get_sp(), 0x0001);
}

/// The helpers are not instructions: no time passes, nothing is contended, and the bus hears of
/// no wait at all. Only `ret` tells the bus where the program counter went, as `RET` does.
#[test]
fn helpers_take_no_time() {
    let (mut cpu, mut bus) = machine(0xC000);
    bus.load_to_memory(&[0x00, 0x90], 0xBFFC);

    cpu.push(&mut bus, 0x1234);
    cpu.pop(&mut bus);
    assert_eq!(bus.take_events(), []);
    cpu.regs.set_sp(0xBFFC);
    cpu.ret(&mut bus);

    assert_eq!(bus.clocks(), 0);
    assert_eq!(bus.take_waits(), []);
    assert_eq!(bus.take_events(), [Event::Pc(0x9000)]);
}

/// A breakpoint on the address `ret` returns to is hit, as it is after `RET`.
#[test]
fn ret_reaches_a_breakpoint() {
    let (mut cpu, mut bus) = machine(0xBFFE);
    bus.load_to_memory(&[0x00, 0x90], 0xBFFE);
    bus.add_breakpoint(0x9000);

    cpu.ret(&mut bus);

    assert_eq!(bus.last_breakpoint(), Some(0x9000));
}

#[test]
fn ret_wraps_around_memory() {
    let (mut cpu, mut bus) = machine(0xFFFF);
    bus.load_to_memory(&[0x34], 0xFFFF);
    bus.load_to_memory(&[0x12], 0x0000);

    cpu.ret(&mut bus);

    assert_eq!((cpu.regs.get_pc(), cpu.regs.get_sp()), (0x1234, 0x0001));
}

/// `ret` is an instruction's worth of time after `LD A,I`, as `RET` is: a maskable interrupt
/// taken after it keeps the P/V that `LD A,I` set, where one taken straight after `LD A,I`
/// would clear it.
#[test]
fn ret_after_ld_a_i_is_like_ret() {
    let run = |use_helper: bool| {
        // LD A,I with interrupts enabled (P/V = IFF2 = 1), then return, then INT
        let (mut cpu, mut bus) = machine(0xBFFE);
        bus.load_to_memory(&[0xED, 0x57, RET], 0x8000);
        bus.load_to_memory(&[0x00, 0x90], 0xBFFE);
        cpu.set_im(1);
        cpu.regs.set_iff1(true);
        cpu.regs.set_iff2(true);
        cpu.emulate(&mut bus);
        if use_helper {
            cpu.ret(&mut bus);
        } else {
            cpu.emulate(&mut bus);
        }
        bus.set_interrupt(true);
        cpu.step(&mut bus);
        cpu.regs.get_flags() & 0x04
    };
    assert_eq!(run(false), 0x04);
    assert_eq!(run(true), 0x04);
}

#[test]
fn push_leaves_memory_and_sp_as_push_bc_does() {
    let (mut by_instruction, mut instruction_bus) = machine(0xC000);
    instruction_bus.load_to_memory(&[PUSH_BC], 0x8000);
    by_instruction.regs.set_bc(0xBEEF);
    let (mut by_helper, mut helper_bus) = machine(0xC000);
    helper_bus.load_to_memory(&[PUSH_BC], 0x8000);

    by_instruction.emulate(&mut instruction_bus);
    by_helper.push(&mut helper_bus, 0xBEEF);

    assert_eq!(by_helper.regs.get_sp(), by_instruction.regs.get_sp());
    assert_eq!(helper_bus.memory(), instruction_bus.memory());
}

/// `ret` leaves the processor as `RET` does, down to MEMPTR and what a following `SCF` sees
/// (Q = 0), apart from what fetching an opcode does: R and time.
#[test]
fn ret_leaves_the_processor_as_ret_does() {
    let setup = || {
        let (mut cpu, mut bus) = machine(0xBFFE);
        bus.load_to_memory(&[0x00, 0x90], 0xBFFE);
        bus.load_to_memory(&[RET], 0x8000);
        bus.load_to_memory(&[SCF], 0x9000);
        // flags just changed, as by the answered routine's last instruction: Q = F
        cpu.regs.set_acc(0x00);
        cpu.regs.set_flags(0x28);
        (cpu, bus)
    };
    let (mut by_instruction, mut instruction_bus) = setup();
    let (mut by_helper, mut helper_bus) = setup();

    by_instruction.emulate(&mut instruction_bus);
    by_helper.ret(&mut helper_bus);

    let mut want = snapshot(&by_instruction);
    // Only running an instruction fetches an opcode (R) and moves Q along to `last_q`; what the
    // next instruction reads is Q itself, which the SCF below checks.
    want.r = by_helper.regs.get_r();
    want.last_q = by_helper.regs.get_last_q();
    assert_eq!(snapshot(&by_helper), want);
    assert_eq!(by_helper.regs.get_pc(), 0x9000);
    assert_eq!(by_helper.regs.get_mem_ptr(), 0x9000);

    by_instruction.emulate(&mut instruction_bus);
    by_helper.emulate(&mut helper_bus);
    // SCF after RET: Q = 0, so bits 3 and 5 come from F | A = 0x28
    assert_eq!(by_helper.regs.get_flags() & 0x28, 0x28);
    assert_eq!(by_helper.regs.get_flags(), by_instruction.regs.get_flags());
    assert_eq!(helper_bus.memory(), instruction_bus.memory());
}
