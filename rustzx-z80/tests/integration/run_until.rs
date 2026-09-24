//! `Z80::run_until`: the `step` loop, run inside the crate.

use crate::{snapshot, State, TestingBus};
use rustzx_z80::{Breakpoints, Step, Stop, Z80};

const NOP: u8 = 0x00;
const HALT: u8 = 0x76;
const EI: u8 = 0xFB;
const INC_B: u8 = 0x04;
const INC_C: u8 = 0x0C;
const JR_E: u8 = 0x18;
const RET: u8 = 0xC9;
const CALL_NN: u8 = 0xCD;
const PROGRAM: u16 = 0x8000;
const HANDLER: u16 = 0x0038;

// Runs that should stop at a breakpoint or an interrupt still get a limit, so a regression
// fails the test instead of hanging it.

fn machine(program: &[u8]) -> (Z80, TestingBus) {
    let mut bus = TestingBus::new(0x10000);
    bus.load_to_memory(program, PROGRAM);
    let mut cpu = Z80::default();
    cpu.set_im(1);
    cpu.regs.set_pc(PROGRAM);
    cpu.regs.set_sp(0xC000);
    (cpu, bus)
}

#[test]
fn breakpoints_is_a_set_of_addresses() {
    let mut set = Breakpoints::new();
    for addr in [0x0000, 0x003F, 0x0040, 0x8000, 0xFFFF] {
        assert!(!set.contains(addr));
        set.insert(addr);
        assert!(set.contains(addr));
    }
    assert!(!set.contains(0x0001));
    assert!(!set.contains(0xFFFE));
    set.remove(0x0040);
    assert!(!set.contains(0x0040));
    assert!(set.contains(0x003F));
    set.clear();
    assert!(!set.contains(0xFFFF));

    let from: Breakpoints = [0x1234, 0x5678].into_iter().collect();
    assert!(from.contains(0x1234) && from.contains(0x5678) && !from.contains(0x1235));
}

#[test]
fn stops_at_the_limit() {
    let (mut cpu, mut bus) = machine(&[NOP; 32]);

    let stop = cpu.run_until(&mut bus, &Breakpoints::new(), |bus| bus.clocks() >= 40);

    assert_eq!(stop, Stop::Limit);
    assert_eq!(bus.clocks(), 40);
    assert_eq!(cpu.regs.get_pc(), PROGRAM + 10);
}

#[test]
fn a_limit_already_reached_runs_nothing() {
    let (mut cpu, mut bus) = machine(&[NOP; 4]);

    assert_eq!(
        cpu.run_until(&mut bus, &Breakpoints::new(), |_| true),
        Stop::Limit
    );
    assert_eq!(bus.clocks(), 0);
    assert_eq!(cpu.regs.get_pc(), PROGRAM);
}

#[test]
fn stops_when_an_instruction_lands_on_a_breakpoint() {
    // CALL 0x9000 ; ... at 0x9000: NOP ; RET
    let (mut cpu, mut bus) = machine(&[CALL_NN, 0x00, 0x90, NOP, NOP]);
    bus.load_to_memory(&[NOP, RET], 0x9000);
    let breakpoints: Breakpoints = [0x9000, PROGRAM + 3].into_iter().collect();

    let stop = cpu.run_until(&mut bus, &breakpoints, |bus| bus.clocks() >= 10_000);
    assert_eq!(stop, Stop::Breakpoint(0x9000));
    assert_eq!(cpu.regs.get_pc(), 0x9000);

    // Called again at a breakpoint, it carries on rather than stopping at once
    let stop = cpu.run_until(&mut bus, &breakpoints, |bus| bus.clocks() >= 10_000);
    assert_eq!(stop, Stop::Breakpoint(PROGRAM + 3));
}

#[test]
fn stops_when_an_interrupt_is_taken() {
    let (mut cpu, mut bus) = machine(&[EI, NOP, NOP, NOP]);
    bus.load_to_memory(&[INC_C], HANDLER);
    bus.set_interrupt(true);

    let stop = cpu.run_until(&mut bus, &Breakpoints::new(), |bus| bus.clocks() >= 10_000);

    assert_eq!(stop, Stop::Interrupt);
    assert_eq!(cpu.regs.get_pc(), HANDLER);
    assert_eq!(cpu.regs.get_c(), 0, "none of the handler has run");
}

/// What a caller gets back from one stop: why, the processor, the clock and memory.
type Outcome = (Stop, State, usize, Vec<u8>);

/// The reference: the loop `run_until` stands for, written out with `step`.
fn by_step_loop(
    cpu: &mut Z80,
    bus: &mut TestingBus,
    breakpoints: &Breakpoints,
    end: usize,
) -> Stop {
    loop {
        if bus.clocks() >= end {
            return Stop::Limit;
        }
        if cpu.step(bus) == Step::Interrupt {
            return Stop::Interrupt;
        }
        let pc = cpu.regs.get_pc();
        if breakpoints.contains(pc) {
            return Stop::Breakpoint(pc);
        }
    }
}

/// Runs a program that counts in B and halts, interrupted every `period` T-states by a handler
/// that counts in C, until `total` T-states, stopping in frames of `frame` T-states and at
/// `breakpoints`. Returns every stop, made with `run_until` or with the reference loop.
fn stops(
    use_run_until: bool,
    breakpoints: &Breakpoints,
    period: usize,
    frame: usize,
    total: usize,
) -> Vec<Outcome> {
    let (mut cpu, mut bus) = machine(&[EI, INC_B, HALT, JR_E, (-5i8).cast_unsigned()]);
    bus.load_to_memory(&[INC_C, EI, RET], HANDLER);
    bus.set_interrupt_period(period, 32);
    let mut out = vec![];
    let mut end = frame;
    while bus.clocks() < total {
        let stop = if use_run_until {
            cpu.run_until(&mut bus, breakpoints, |bus| bus.clocks() >= end)
        } else {
            by_step_loop(&mut cpu, &mut bus, breakpoints, end)
        };
        if stop == Stop::Limit {
            end += frame;
        }
        out.push((stop, snapshot(&cpu), bus.clocks(), bus.memory().to_vec()));
    }
    out
}

#[test]
fn run_until_stops_where_the_step_loop_does() {
    let sets: Vec<Breakpoints> = vec![
        Breakpoints::new(),
        [PROGRAM + 2].into_iter().collect(),
        [HANDLER + 1, PROGRAM].into_iter().collect(),
        (0..=0xFFFF).collect(),
    ];
    for breakpoints in &sets {
        for (period, frame) in [(1000, 700), (997, 5000), (64, 50)] {
            let by_run_until = stops(true, breakpoints, period, frame, 30_000);
            let by_step = stops(false, breakpoints, period, frame, 30_000);
            assert_eq!(by_run_until.len(), by_step.len());
            for (n, (a, b)) in by_run_until.iter().zip(&by_step).enumerate() {
                assert!(a == b, "stop {n} differs: {:?} vs {:?}", a.0, b.0);
            }
            assert!(by_run_until.iter().any(|o| o.0 == Stop::Interrupt));
            assert!(by_run_until.iter().any(|o| o.0 == Stop::Limit));
        }
    }
}
