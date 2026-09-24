//! What `step` does, and that it runs a program exactly as `emulate` does.

use crate::{snapshot, Event, State, TestingBus};
use rustzx_z80::{Step, Z80};

const DI: u8 = 0xF3;
const EI: u8 = 0xFB;
const HALT: u8 = 0x76;
const NOP: u8 = 0x00;
const INC_B: u8 = 0x04;
const INC_C: u8 = 0x0C;
const INC_E: u8 = 0x1C;
const JR_E: u8 = 0x18;
const RET: u8 = 0xC9;
const PREFIX_CB: u8 = 0xCB;
const PREFIX_DD: u8 = 0xDD;
const PREFIX_ED: u8 = 0xED;
const PREFIX_FD: u8 = 0xFD;
const LD_HL_NN: u8 = 0x21;
const RETN: u8 = 0x45;

const IM1_HANDLER: u16 = 0x0038;
const IM2_HANDLER: u16 = 0xA000;
const NMI_HANDLER: u16 = 0x0066;
const PROGRAM: u16 = 0x8000;
const STACK: u16 = 0xC000;

/// A processor at `PROGRAM` in interrupt mode `im`, interrupts enabled, over memory holding
/// `program` there, a handler at `handler` for each kind of interrupt, and an IM 2 vector
/// pointing at `IM2_HANDLER`. Event recording is on.
fn machine(im: u8, program: &[u8], handler: &[u8]) -> (Z80, TestingBus) {
    let mut bus = TestingBus::new(0x10000);
    bus.load_to_memory(program, PROGRAM);
    bus.load_to_memory(handler, IM1_HANDLER);
    bus.load_to_memory(handler, NMI_HANDLER);
    bus.load_to_memory(handler, IM2_HANDLER);
    bus.load_to_memory(&IM2_HANDLER.to_le_bytes(), 0x90FF);
    bus.set_interrupt_data(0xFF);
    bus.record_events();
    let mut cpu = Z80::default();
    cpu.set_im(im);
    cpu.regs.set_i(0x90);
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

#[derive(Clone, Copy, Debug)]
enum Line {
    Int,
    Nmi,
}

impl Line {
    fn set(self, bus: &mut TestingBus, active: bool) {
        match self {
            Line::Int => bus.set_interrupt(active),
            Line::Nmi => bus.set_nmi(active),
        }
    }
}

/// Every kind of interrupt: (interrupt mode, line, handler address, clocks the interrupt takes).
const KINDS: [(u8, Line, u16, usize); 4] = [
    (0, Line::Int, IM1_HANDLER, 13),
    (1, Line::Int, IM1_HANDLER, 13),
    (2, Line::Int, IM2_HANDLER, 19),
    (1, Line::Nmi, NMI_HANDLER, 11),
];

/// Takes the interrupt with `step` on one copy of the machine and with `emulate` on another,
/// and checks that `step` stopped at the handler with nothing of it run, and that the
/// instruction `step` runs next brings both copies to the same place.
fn check_interrupt_step(
    cpu: Z80,
    mut bus: TestingBus,
    line: Line,
    handler: u16,
    int_clocks: usize,
) {
    bus.take_waits();
    let (mut by_step, mut step_bus) = (cpu.clone(), bus.clone());
    let (mut by_emulate, mut emulate_bus) = (cpu, bus);
    let before = snapshot(&by_step);
    let clocks_before = step_bus.clocks();
    line.set(&mut step_bus, true);
    line.set(&mut emulate_bus, true);

    assert_eq!(by_step.step(&mut step_bus), Step::Interrupt);
    line.set(&mut step_bus, false);

    let after = snapshot(&by_step);
    assert_eq!(after.pc, handler);
    assert_eq!(after.sp, before.sp.wrapping_sub(2));
    assert_eq!(after.mem_ptr, handler);
    // R counts in its low 7 bits and keeps bit 7.
    assert_eq!(
        after.r,
        (before.r & 0x80) | (before.r.wrapping_add(1) & 0x7F)
    );
    assert!(!after.iff1);
    // an NMI keeps IFF2 (for RETN to restore); a maskable interrupt clears it
    assert_eq!(after.iff2, before.iff2 && matches!(line, Line::Nmi));
    assert!(!after.halted);
    assert!(!after.skip_interrupt);
    assert_eq!(step_bus.clocks() - clocks_before, int_clocks);
    let expected_return = if before.halted {
        before.pc + 1
    } else {
        before.pc
    };
    assert_eq!(return_address(&by_step, &mut step_bus), expected_return);
    // Only the registers an interrupt touches have changed.
    assert_eq!(
        State {
            pc: before.pc,
            sp: before.sp,
            mem_ptr: before.mem_ptr,
            r: before.r,
            iff1: before.iff1,
            iff2: before.iff2,
            halted: before.halted,
            ..after.clone()
        },
        before
    );
    let mut step_events = step_bus.take_events();
    // exactly one callback for the interrupt, with the handler's address
    assert_eq!(step_events.pop(), Some(Event::Pc(handler)));
    assert!(!step_events.contains(&Event::Pc(handler)));
    let mut step_waits = step_bus.take_waits();

    assert_eq!(by_step.step(&mut step_bus), Step::Instruction);
    by_emulate.emulate(&mut emulate_bus);
    line.set(&mut emulate_bus, false);

    assert_eq!(snapshot(&by_step), snapshot(&by_emulate));
    assert_eq!(step_bus.clocks(), emulate_bus.clocks());
    assert_eq!(step_bus.memory(), emulate_bus.memory());
    step_waits.extend(step_bus.take_waits());
    assert_eq!(step_waits, emulate_bus.take_waits());
    step_events.extend(step_bus.take_events());
    assert_eq!(step_events, emulate_bus.take_events());
}

#[test]
fn interrupt_step_for_each_kind() {
    for (im, line, handler, clocks) in KINDS {
        let (cpu, bus) = machine(im, &[NOP], &[INC_C, INC_C]);
        check_interrupt_step(cpu, bus, line, handler, clocks);
    }
}

#[test]
fn interrupt_step_for_each_kind_from_halt() {
    for (im, line, handler, clocks) in KINDS {
        let (mut cpu, mut bus) = machine(im, &[HALT], &[INC_C, INC_C]);
        assert_eq!(cpu.step(&mut bus), Step::Instruction);
        assert_eq!(cpu.step(&mut bus), Step::Instruction);
        assert!(cpu.is_halted());
        bus.take_events();
        check_interrupt_step(cpu, bus, line, handler, clocks);
    }
}

#[test]
fn interrupt_step_for_each_kind_from_halted_set_by_caller() {
    for (im, line, handler, clocks) in KINDS {
        let (mut cpu, bus) = machine(im, &[HALT], &[INC_C, INC_C]);
        cpu.halted = true;
        check_interrupt_step(cpu, bus, line, handler, clocks);
    }
}

#[test]
fn interrupt_step_with_nonzero_registers() {
    for (im, line, handler, clocks) in KINDS {
        let (mut cpu, bus) = machine(im, &[NOP], &[INC_C, INC_C]);
        cpu.regs.set_bc(0x1234);
        cpu.regs.set_de(0x5678);
        cpu.regs.set_hl(0x9ABC);
        cpu.regs.set_ix(0xDEF0);
        cpu.regs.set_iy(0x0FED);
        cpu.regs.set_acc(0x55);
        cpu.regs.set_flags(0xAA);
        cpu.regs.set_r(0xFF);
        cpu.regs.set_iff2(false);
        check_interrupt_step(cpu, bus, line, handler, clocks);
    }
}

/// Runs `program` up to its last byte with nothing pending, and then, with the interrupt raised
/// on the next check, checks that `step` defers it exactly as `emulate` does.
fn check_deferred(program: &[u8], deferred_steps: usize, line: Line) {
    let (mut cpu, mut bus) = machine(1, program, &[NOP]);
    let (mut other, mut other_bus) = (cpu.clone(), bus.clone());

    // The first instruction sets up the deferral, with no interrupt pending yet.
    assert_eq!(cpu.step(&mut bus), Step::Instruction);
    other.emulate(&mut other_bus);
    line.set(&mut bus, true);
    line.set(&mut other_bus, true);

    for _ in 0..deferred_steps {
        assert_eq!(cpu.step(&mut bus), Step::Instruction);
        other.emulate(&mut other_bus);
        assert_eq!(snapshot(&cpu), snapshot(&other));
        assert_eq!(bus.take_waits(), other_bus.take_waits());
    }
    let pc = cpu.regs.get_pc();
    assert_eq!(cpu.step(&mut bus), Step::Interrupt);
    assert_eq!(return_address(&cpu, &mut bus), pc);
    other.emulate(&mut other_bus);
    assert_eq!(return_address(&other, &mut other_bus), pc);
}

#[test]
fn ei_defers_interrupt_step() {
    let (mut cpu, mut bus) = machine(1, &[EI, NOP, NOP], &[NOP]);
    cpu.regs.set_iff1(false);
    cpu.regs.set_iff2(false);
    assert_eq!(cpu.step(&mut bus), Step::Instruction);
    bus.set_interrupt(true);
    assert_eq!(cpu.step(&mut bus), Step::Instruction);
    assert_eq!(cpu.step(&mut bus), Step::Interrupt);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM + 2);
}

#[test]
fn chained_ei_defers_interrupt_step() {
    check_deferred(&[EI, EI, EI, NOP], 3, Line::Int);
}

/// `EI` and `DI` hold off only the maskable interrupt; an NMI is taken straight after them.
#[test]
fn ei_and_di_do_not_defer_nmi_step() {
    check_deferred(&[DI, NOP, NOP], 0, Line::Nmi);
    check_deferred(&[EI, NOP, NOP], 0, Line::Nmi);
}

#[test]
fn prefix_chains_defer_interrupt_step() {
    for (first, second) in [
        (PREFIX_DD, PREFIX_DD),
        (PREFIX_DD, PREFIX_FD),
        (PREFIX_FD, PREFIX_DD),
        (PREFIX_FD, PREFIX_FD),
    ] {
        for line in [Line::Int, Line::Nmi] {
            check_deferred(&[first, second, LD_HL_NN, 0x34, 0x12, NOP], 1, line);
        }
    }
}

#[test]
fn skip_interrupt_set_by_caller_defers_one_step() {
    let (mut cpu, mut bus) = machine(1, &[NOP, NOP], &[NOP]);
    cpu.skip_interrupt = true;
    bus.set_interrupt(true);

    assert_eq!(cpu.step(&mut bus), Step::Instruction);
    assert!(!cpu.skip_interrupt);
    assert_eq!(cpu.step(&mut bus), Step::Interrupt);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM + 1);
}

#[test]
fn int_is_ignored_by_step_with_interrupts_disabled() {
    let (mut cpu, mut bus) = machine(1, &[NOP], &[NOP]);
    cpu.regs.set_iff1(false);
    bus.set_interrupt(true);

    assert_eq!(cpu.step(&mut bus), Step::Instruction);
    assert_eq!(cpu.regs.get_pc(), PROGRAM + 1);
}

#[test]
fn nmi_wins_over_int_in_step() {
    let (mut cpu, mut bus) = machine(1, &[NOP], &[NOP]);
    bus.set_nmi(true);
    bus.set_interrupt(true);

    assert_eq!(cpu.step(&mut bus), Step::Interrupt);
    assert_eq!(cpu.regs.get_pc(), NMI_HANDLER);
    assert!(cpu.regs.get_iff2());
}

/// With nothing pending, `step` and `emulate` are the same call.
#[test]
fn step_without_interrupt_is_emulate() {
    let program = [
        NOP, INC_B, PREFIX_CB, 0x00, // RLC B
        PREFIX_ED, 0x44, // NEG
        PREFIX_DD, LD_HL_NN, 0x34, 0x12, // LD IX,0x1234
        PREFIX_DD, PREFIX_CB, 0x01, 0x06, // RLC (IX+1)
        PREFIX_FD, PREFIX_DD, PREFIX_ED, 0x44, // FD DD ED NEG, a prefix chain
        PREFIX_DD, PREFIX_DD, PREFIX_FD, LD_HL_NN, 0x78,
        0x56, // LD IY,0x5678 after a chain of three
        0x3E, 0x0F, // LD A,0x0F
        0x37, // SCF, which reads Q
        0x3F, // CCF
        HALT,
    ];
    let (mut by_step, mut step_bus) = machine(1, &program, &[NOP]);
    let (mut by_emulate, mut emulate_bus) = machine(1, &program, &[NOP]);

    for _ in 0..30 {
        assert_eq!(by_step.step(&mut step_bus), Step::Instruction);
        by_emulate.emulate(&mut emulate_bus);
        assert_eq!(snapshot(&by_step), snapshot(&by_emulate));
        assert_eq!(step_bus.clocks(), emulate_bus.clocks());
        assert_eq!(step_bus.take_events(), emulate_bus.take_events());
        assert_eq!(step_bus.take_waits(), emulate_bus.take_waits());
    }
    assert_eq!(step_bus.memory(), emulate_bus.memory());
    assert!(by_step.is_halted());
    assert_eq!(by_step.regs.get_ix(), 0x1234);
    assert_eq!(by_step.regs.get_iy(), 0x5678);
}

#[test]
fn step_is_plain_data() {
    fn assert_traits<T: Copy + Clone + Eq + std::fmt::Debug + Send + Sync + 'static>() {}
    assert_traits::<Step>();
    let a = Step::Interrupt;
    let b = a;
    assert_eq!(a, b);
    assert_ne!(Step::Interrupt, Step::Instruction);
    assert_eq!(format!("{:?}", Step::Instruction), "Instruction");
}

/// Small deterministic pseudo-random numbers, so runs are repeatable without a dependency.
struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12345);
        self.0 >> 16
    }
}

/// When the interrupt lines are raised, as a function of the clock.
#[derive(Clone, Copy)]
enum Schedule {
    /// INT held for 32 clocks every `period`.
    Periodic { period: usize },
    /// INT at random, and NMI in one-call pulses at random, which the processor latches.
    Random { seed: u32 },
}

/// A machine that is run both ways, and what its bus said.
struct Run {
    cpu: Z80,
    bus: TestingBus,
    rng: Lcg,
}

impl Run {
    /// Raises lines for the coming step. The same clock and rng state give the same lines, so both
    /// runs see the same interrupts as long as they stay in step.
    fn drive(&mut self, schedule: Schedule) {
        match schedule {
            Schedule::Periodic { period } => {
                self.bus.set_interrupt(self.bus.clocks() % period < 32);
            }
            Schedule::Random { .. } => {
                let n = self.rng.next();
                self.bus.set_interrupt(n.is_multiple_of(7));
                self.bus.set_nmi(n.is_multiple_of(97));
            }
        }
    }
}

/// Runs `program` with `handler` for each kind of interrupt, driven by `schedule`, with
/// `emulate` and with `step`, and compares both machines after every `emulate` call.
fn check_same_run(im: u8, program: &[u8], handler: &[u8], schedule: Schedule, clocks: usize) {
    let seed = match schedule {
        Schedule::Random { seed } => seed,
        Schedule::Periodic { .. } => 0,
    };
    let (cpu, bus) = machine(im, program, handler);
    let mut by_emulate = Run {
        cpu: cpu.clone(),
        bus: bus.clone(),
        rng: Lcg(seed),
    };
    let mut by_step = Run {
        cpu,
        bus,
        rng: Lcg(seed),
    };
    let handlers = [IM1_HANDLER, IM2_HANDLER, NMI_HANDLER];
    let mut interrupts = 0;
    let mut nmis = 0;
    let mut calls = 0;

    while by_emulate.bus.clocks() < clocks {
        calls += 1;

        by_emulate.drive(schedule);
        by_emulate.cpu.emulate(&mut by_emulate.bus);

        by_step.drive(schedule);
        let mut step_events = Vec::new();
        if by_step.cpu.step(&mut by_step.bus) == Step::Interrupt {
            interrupts += 1;
            let at = by_step.cpu.regs.get_pc();
            assert!(
                handlers.contains(&at),
                "step {calls}: interrupt to {at:#06x}"
            );
            if at == NMI_HANDLER {
                nmis += 1;
            }
            step_events.extend(by_step.bus.take_events());
            assert_eq!(step_events.pop(), Some(Event::Pc(at)));
            assert_eq!(by_step.cpu.step(&mut by_step.bus), Step::Instruction);
        }
        step_events.extend(by_step.bus.take_events());

        assert_eq!(
            snapshot(&by_step.cpu),
            snapshot(&by_emulate.cpu),
            "call {calls}"
        );
        assert_eq!(
            by_step.bus.clocks(),
            by_emulate.bus.clocks(),
            "call {calls}"
        );
        assert_eq!(step_events, by_emulate.bus.take_events(), "call {calls}");
        assert_eq!(
            by_step.bus.take_waits(),
            by_emulate.bus.take_waits(),
            "call {calls}"
        );
    }
    assert!(
        by_step.bus.memory() == by_emulate.bus.memory(),
        "memory differs"
    );
    assert!(interrupts > 10, "only {interrupts} interrupts");
    if let Schedule::Random { .. } = schedule {
        assert!(nmis > 5, "only {nmis} NMIs");
    }
}

/// Counts in B and halts; interrupts count in C.
const COUNTING: [u8; 5] = [EI, INC_B, HALT, JR_E, (-5i8).cast_unsigned()];
const COUNTING_HANDLER: [u8; 3] = [INC_C, EI, RET];

/// Toggles interrupts, runs prefix chains, and never halts; interrupts count in E and return
/// with RETN, which also serves the NMI.
const BUSY: [u8; 22] = [
    EI,
    INC_B,
    PREFIX_DD,
    PREFIX_DD,
    PREFIX_FD,
    LD_HL_NN,
    0x34,
    0x12,
    DI,
    PREFIX_FD,
    PREFIX_CB,
    0x00,
    0x06, // RLC (IY+0)
    EI,
    EI,
    PREFIX_ED,
    0x44, // NEG
    PREFIX_DD,
    0x23, // INC IX
    0x37, // SCF
    JR_E,
    (-22i8).cast_unsigned(),
];
const BUSY_HANDLER: [u8; 4] = [INC_E, 0xFB, PREFIX_ED, RETN]; // INC E; EI; RETN

#[test]
fn same_run_counting_each_mode() {
    for im in 0..3 {
        for period in [1000, 777, 64] {
            check_same_run(
                im,
                &COUNTING,
                &COUNTING_HANDLER,
                Schedule::Periodic { period },
                50_000,
            );
        }
    }
}

#[test]
fn same_run_busy_each_mode_random_lines() {
    for im in 0..3 {
        for seed in [1, 2, 3, 0xDEAD] {
            check_same_run(im, &BUSY, &BUSY_HANDLER, Schedule::Random { seed }, 30_000);
        }
    }
}

#[test]
fn same_run_counting_random_lines() {
    for seed in [7, 8, 9] {
        check_same_run(
            1,
            &COUNTING,
            &COUNTING_HANDLER,
            Schedule::Random { seed },
            30_000,
        );
    }
}
