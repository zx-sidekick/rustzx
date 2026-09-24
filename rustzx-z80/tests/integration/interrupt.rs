//! Taking interrupts, with `emulate` and with `step`.

use crate::TestingBus;
use rustzx_z80::{Step, Z80};

const EI: u8 = 0xFB;
const HALT: u8 = 0x76;
const INC_B: u8 = 0x04;
const INC_C: u8 = 0x0C;
const LD_A_N: u8 = 0x3E;
const NOP: u8 = 0x00;
const RET: u8 = 0xC9;
const JR_E: u8 = 0x18;

const HANDLER: u16 = 0x0038;
const PROGRAM: u16 = 0x8000;
const STACK: u16 = 0xC000;

/// A processor at `PROGRAM` with interrupts enabled in IM 1, over memory holding `program`
/// there and `handler` at 0x0038.
fn machine(program: &[u8], handler: &[u8]) -> (Z80, TestingBus) {
    let mut bus = TestingBus::new(0x10000);
    bus.load_to_memory(program, PROGRAM);
    bus.load_to_memory(handler, HANDLER);
    let mut cpu = Z80::default();
    cpu.set_im(1);
    cpu.regs.set_pc(PROGRAM);
    cpu.regs.set_sp(STACK);
    cpu.regs.set_iff1(true);
    cpu.regs.set_iff2(true);
    (cpu, bus)
}

fn return_address(cpu: &Z80, bus: &mut TestingBus) -> u16 {
    let sp = cpu.regs.get_sp();
    u16::from_le_bytes([bus.read_memory(sp), bus.read_memory(sp + 1)])
}

#[test]
fn emulate_runs_first_handler_instruction_with_interrupt() {
    let (mut cpu, mut bus) = machine(&[NOP], &[LD_A_N, 0x42]);
    bus.set_interrupt(true);

    cpu.emulate(&mut bus);

    assert_eq!(cpu.regs.get_pc(), HANDLER + 2);
    assert_eq!(cpu.regs.get_acc(), 0x42);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM);
}

#[test]
fn step_stops_at_handler_before_it_runs() {
    let (mut cpu, mut bus) = machine(&[NOP], &[LD_A_N, 0x42]);
    bus.add_breakpoint(HANDLER);
    bus.set_interrupt(true);

    assert_eq!(cpu.step(&mut bus), Step::Interrupt);
    assert_eq!(cpu.regs.get_pc(), HANDLER);
    assert_eq!(cpu.regs.get_acc(), 0);
    assert!(!cpu.regs.get_iff1());
    assert_eq!(cpu.regs.get_sp(), STACK - 2);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM);
    assert_eq!(bus.clocks(), 13);
    assert_eq!(bus.last_breakpoint(), Some(HANDLER));

    assert_eq!(cpu.step(&mut bus), Step::Instruction);
    assert_eq!(cpu.regs.get_pc(), HANDLER + 2);
    assert_eq!(cpu.regs.get_acc(), 0x42);
}

#[test]
fn step_runs_instruction_after_ei_before_interrupt() {
    let (mut cpu, mut bus) = machine(&[EI, NOP, NOP], &[NOP]);
    cpu.regs.set_iff1(false);
    cpu.regs.set_iff2(false);
    bus.set_interrupt(true);

    assert_eq!(cpu.step(&mut bus), Step::Instruction);
    assert_eq!(cpu.step(&mut bus), Step::Instruction);
    assert_eq!(cpu.regs.get_pc(), PROGRAM + 2);
    assert_eq!(cpu.step(&mut bus), Step::Interrupt);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM + 2);
}

#[test]
fn step_takes_interrupt_from_halt_returning_past_it() {
    let (mut cpu, mut bus) = machine(&[HALT], &[NOP]);

    assert_eq!(cpu.step(&mut bus), Step::Instruction);
    assert_eq!(cpu.step(&mut bus), Step::Instruction);
    assert!(cpu.is_halted());
    assert_eq!(cpu.regs.get_pc(), PROGRAM);

    bus.set_interrupt(true);
    assert_eq!(cpu.step(&mut bus), Step::Interrupt);
    assert!(!cpu.is_halted());
    assert_eq!(cpu.regs.get_pc(), HANDLER);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM + 1);
}

#[test]
fn restored_halt_is_left_by_interrupt() {
    let (mut cpu, mut bus) = machine(&[HALT], &[NOP]);
    cpu.halted = true;
    bus.set_interrupt(true);

    assert_eq!(cpu.step(&mut bus), Step::Interrupt);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM + 1);
}

/// Runs a program that counts in B, halts, and is interrupted every 1000 clocks by a handler
/// that counts in C, until `clocks` have passed. Returns the processor, the memory near the
/// stack, the clocks passed, and how many interrupts `step` reported.
fn run_counting(clocks: usize, split: bool) -> (Z80, Vec<u8>, usize, usize) {
    let (mut cpu, mut bus) = machine(
        &[EI, INC_B, HALT, JR_E, (-5i8).cast_unsigned()],
        &[INC_C, EI, RET],
    );
    let mut interrupts = 0;
    loop {
        bus.set_interrupt(bus.clocks() % 1000 < 32);
        if split {
            if cpu.step(&mut bus) == Step::Interrupt {
                interrupts += 1;
                continue;
            }
        } else {
            cpu.emulate(&mut bus);
        }
        if bus.clocks() >= clocks {
            break;
        }
    }
    let stack = (STACK - 4..STACK).map(|a| bus.read_memory(a)).collect();
    (cpu, stack, bus.clocks(), interrupts)
}

#[test]
fn step_runs_program_as_emulate_does() {
    let (by_emulate, emulate_stack, emulate_clocks, _) = run_counting(100_000, false);
    let (by_step, step_stack, step_clocks, interrupts) = run_counting(100_000, true);

    assert!(interrupts > 50);
    assert_eq!(by_step.regs.get_c() as usize, interrupts % 256);
    assert_eq!(step_clocks, emulate_clocks);
    assert_eq!(step_stack, emulate_stack);
    for (a, b) in [
        (by_step.regs.get_pc(), by_emulate.regs.get_pc()),
        (by_step.regs.get_sp(), by_emulate.regs.get_sp()),
        (by_step.regs.get_bc(), by_emulate.regs.get_bc()),
        (by_step.regs.get_af(), by_emulate.regs.get_af()),
        (
            u16::from(by_step.regs.get_r()),
            u16::from(by_emulate.regs.get_r()),
        ),
    ] {
        assert_eq!(a, b);
    }
    assert_eq!(by_step.regs.get_iff1(), by_emulate.regs.get_iff1());
    assert_eq!(by_step.is_halted(), by_emulate.is_halted());
}
