//! What `emulate` does, pinned down exactly: clocks, registers, stack and bus events.
//!
//! Uses only what upstream `master` has (no `step`, no `Clone`), so this file can be run against
//! upstream to show that these are its values, and that `emulate` has not changed here.

use crate::{snapshot, Event, TestingBus, Wait};
use rustzx_z80::Z80;

const EI: u8 = 0xFB;
const HALT: u8 = 0x76;
const NOP: u8 = 0x00;
const INC_B: u8 = 0x04;
const INC_C: u8 = 0x0C;
const JR_E: u8 = 0x18;
const RET: u8 = 0xC9;
const PREFIX_DD: u8 = 0xDD;
const PREFIX_ED: u8 = 0xED;
const PREFIX_FD: u8 = 0xFD;
const LD_HL_NN: u8 = 0x21;
const RETI: u8 = 0x4D;
const RETN: u8 = 0x45;

const IM1_HANDLER: u16 = 0x0038;
const NMI_HANDLER: u16 = 0x0066;
const PROGRAM: u16 = 0x8000;
const STACK: u16 = 0xC000;

/// A processor at `PROGRAM` in interrupt mode `im`, interrupts enabled, over memory holding
/// `program` there, with event recording on.
fn machine(im: u8, program: &[u8]) -> (Z80, TestingBus) {
    let mut bus = TestingBus::new(0x10000);
    bus.load_to_memory(program, PROGRAM);
    bus.record_events();
    let mut cpu = Z80::default();
    cpu.set_im(im);
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
fn im1_interrupt_then_first_handler_instruction() {
    let (mut cpu, mut bus) = machine(1, &[NOP]);
    bus.set_interrupt(true);

    cpu.emulate(&mut bus);

    assert_eq!(bus.clocks(), 13 + 4);
    assert_eq!(cpu.regs.get_pc(), IM1_HANDLER + 1);
    assert_eq!(cpu.regs.get_sp(), STACK - 2);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM);
    assert_eq!(cpu.regs.get_r(), 2);
    assert_eq!(cpu.regs.get_mem_ptr(), IM1_HANDLER);
    assert!(!cpu.regs.get_iff1());
    assert!(!cpu.regs.get_iff2());
    assert_eq!(bus.take_events(), [Event::Pc(IM1_HANDLER + 1)]);
}

#[test]
fn im0_interrupt_is_taken_as_im1() {
    let (mut im0, mut bus0) = machine(0, &[NOP]);
    let (mut im1, mut bus1) = machine(1, &[NOP]);
    bus0.set_interrupt(true);
    bus1.set_interrupt(true);

    im0.emulate(&mut bus0);
    im1.emulate(&mut bus1);

    let mut expected = snapshot(&im1);
    expected.im = 0;
    assert_eq!(snapshot(&im0), expected);
    assert_eq!(bus0.clocks(), bus1.clocks());
    assert_eq!(bus0.memory(), bus1.memory());
}

#[test]
fn im2_interrupt_jumps_through_vector() {
    let (mut cpu, mut bus) = machine(2, &[NOP]);
    cpu.regs.set_i(0x90);
    bus.set_interrupt_data(0x20);
    bus.load_to_memory(&[0x00, 0xA0], 0x9020);
    bus.set_interrupt(true);

    cpu.emulate(&mut bus);

    assert_eq!(bus.clocks(), 19 + 4);
    assert_eq!(cpu.regs.get_pc(), 0xA001);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM);
    assert_eq!(cpu.regs.get_r(), 2);
    assert_eq!(cpu.regs.get_mem_ptr(), 0xA000);
    assert!(!cpu.regs.get_iff1());
    assert!(!cpu.regs.get_iff2());
    assert_eq!(bus.take_events(), [Event::Pc(0xA001)]);
}

#[test]
fn nmi_keeps_iff2() {
    let (mut cpu, mut bus) = machine(1, &[NOP]);
    bus.set_nmi(true);

    cpu.emulate(&mut bus);

    assert_eq!(bus.clocks(), 11 + 4);
    assert_eq!(cpu.regs.get_pc(), NMI_HANDLER + 1);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM);
    assert_eq!(cpu.regs.get_r(), 2);
    assert_eq!(cpu.regs.get_mem_ptr(), NMI_HANDLER);
    assert!(!cpu.regs.get_iff1());
    assert!(cpu.regs.get_iff2());
    assert_eq!(bus.take_events(), [Event::Pc(NMI_HANDLER + 1)]);
}

#[test]
fn nmi_is_taken_with_interrupts_disabled() {
    let (mut cpu, mut bus) = machine(1, &[NOP]);
    cpu.regs.set_iff1(false);
    cpu.regs.set_iff2(false);
    bus.set_nmi(true);

    cpu.emulate(&mut bus);

    assert_eq!(cpu.regs.get_pc(), NMI_HANDLER + 1);
    assert!(!cpu.regs.get_iff2());
}

#[test]
fn nmi_wins_over_int() {
    let (mut cpu, mut bus) = machine(1, &[NOP]);
    bus.set_nmi(true);
    bus.set_interrupt(true);

    cpu.emulate(&mut bus);

    assert_eq!(cpu.regs.get_pc(), NMI_HANDLER + 1);
    assert!(cpu.regs.get_iff2());
    assert_eq!(bus.clocks(), 11 + 4);
}

#[test]
fn retn_from_nmi_restores_iff1() {
    let (mut cpu, mut bus) = machine(1, &[NOP]);
    bus.load_to_memory(&[PREFIX_ED, RETN], NMI_HANDLER);
    bus.set_nmi(true);

    cpu.emulate(&mut bus);
    bus.set_nmi(false);

    assert_eq!(cpu.regs.get_pc(), PROGRAM);
    assert_eq!(cpu.regs.get_sp(), STACK);
    assert!(cpu.regs.get_iff1());
    assert_eq!(bus.clocks(), 11 + 14);
}

/// RETN copies IFF2 into IFF1 rather than setting it: with interrupts disabled before the NMI,
/// they stay disabled after the return.
#[test]
fn retn_from_nmi_copies_iff2() {
    let (mut cpu, mut bus) = machine(1, &[NOP]);
    cpu.regs.set_iff1(false);
    cpu.regs.set_iff2(false);
    bus.load_to_memory(&[PREFIX_ED, RETN], NMI_HANDLER);
    bus.set_nmi(true);

    cpu.emulate(&mut bus);
    bus.set_nmi(false);

    assert_eq!(cpu.regs.get_pc(), PROGRAM);
    assert!(!cpu.regs.get_iff1());
    assert!(!cpu.regs.get_iff2());
}

/// Taking an NMI clears Q, the record of whether the last instruction changed the flags: the
/// handler's first instruction sees Q = 0 though the instruction before the NMI changed F.
#[test]
fn nmi_clears_q() {
    // SCF changes the flags, so Q = F after it
    let (mut cpu, mut bus) = machine(1, &[0x37, NOP]);
    cpu.emulate(&mut bus);
    bus.set_nmi(true);

    cpu.emulate(&mut bus);

    assert_eq!(cpu.regs.get_pc(), NMI_HANDLER + 1);
    assert_eq!(snapshot(&cpu).last_q, 0);
}

#[test]
fn int_is_ignored_with_interrupts_disabled() {
    let (mut cpu, mut bus) = machine(1, &[NOP]);
    cpu.regs.set_iff1(false);
    bus.set_interrupt(true);

    cpu.emulate(&mut bus);

    assert_eq!(cpu.regs.get_pc(), PROGRAM + 1);
    assert_eq!(cpu.regs.get_sp(), STACK);
    assert_eq!(bus.clocks(), 4);
    assert_eq!(cpu.regs.get_r(), 1);
}

#[test]
fn ei_defers_interrupt_one_instruction() {
    let (mut cpu, mut bus) = machine(1, &[EI, NOP, NOP]);
    cpu.regs.set_iff1(false);
    cpu.regs.set_iff2(false);
    bus.set_interrupt(true);

    cpu.emulate(&mut bus);
    assert!(cpu.skip_interrupt);
    cpu.emulate(&mut bus);
    assert!(!cpu.skip_interrupt);
    assert_eq!(cpu.regs.get_pc(), PROGRAM + 2);
    cpu.emulate(&mut bus);
    assert_eq!(cpu.regs.get_pc(), IM1_HANDLER + 1);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM + 2);
}

#[test]
fn chained_ei_keeps_deferring() {
    let (mut cpu, mut bus) = machine(1, &[EI, EI, EI, NOP]);
    bus.set_interrupt(true);
    cpu.skip_interrupt = true;

    for _ in 0..3 {
        cpu.emulate(&mut bus);
    }
    assert_eq!(cpu.regs.get_pc(), PROGRAM + 3);
    cpu.emulate(&mut bus);
    assert_eq!(cpu.regs.get_pc(), PROGRAM + 4);
    cpu.emulate(&mut bus);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM + 4);
}

fn prefix_chain_defers_interrupt(first: u8, second: u8) -> Z80 {
    let (mut cpu, mut bus) = machine(1, &[first, second, LD_HL_NN, 0x34, 0x12, NOP]);

    cpu.emulate(&mut bus);
    assert_eq!(cpu.regs.get_pc(), PROGRAM + 2);
    assert!(cpu.skip_interrupt);
    assert_eq!(bus.clocks(), 8);
    assert_eq!(cpu.regs.get_r(), 2);
    assert_eq!(bus.take_events(), [Event::Pc(PROGRAM + 2)]);

    bus.set_interrupt(true);
    cpu.emulate(&mut bus);
    assert_eq!(cpu.regs.get_pc(), PROGRAM + 5);
    assert!(!cpu.skip_interrupt);
    assert_eq!(bus.clocks(), 18);
    assert_eq!(cpu.regs.get_r(), 3);

    cpu.emulate(&mut bus);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM + 5);
    cpu
}

#[test]
fn dd_dd_prefix_chain_defers_interrupt() {
    let cpu = prefix_chain_defers_interrupt(PREFIX_DD, PREFIX_DD);
    assert_eq!(cpu.regs.get_ix(), 0x1234);
    assert_eq!(cpu.regs.get_hl(), 0);
}

#[test]
fn fd_dd_prefix_chain_defers_interrupt() {
    let cpu = prefix_chain_defers_interrupt(PREFIX_FD, PREFIX_DD);
    assert_eq!(cpu.regs.get_ix(), 0x1234);
    assert_eq!(cpu.regs.get_iy(), 0);
}

#[test]
fn dd_fd_prefix_chain_defers_interrupt() {
    let cpu = prefix_chain_defers_interrupt(PREFIX_DD, PREFIX_FD);
    assert_eq!(cpu.regs.get_iy(), 0x1234);
    assert_eq!(cpu.regs.get_ix(), 0);
}

#[test]
fn skip_interrupt_set_by_caller_defers_one_call() {
    let (mut cpu, mut bus) = machine(1, &[NOP, NOP]);
    cpu.skip_interrupt = true;
    bus.set_interrupt(true);

    cpu.emulate(&mut bus);
    assert_eq!(cpu.regs.get_pc(), PROGRAM + 1);
    assert!(!cpu.skip_interrupt);
    cpu.emulate(&mut bus);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM + 1);
}

#[test]
fn halt_repeats_in_place() {
    let (mut cpu, mut bus) = machine(1, &[HALT]);

    for _ in 0..3 {
        cpu.emulate(&mut bus);
    }

    assert!(cpu.is_halted());
    assert_eq!(cpu.regs.get_pc(), PROGRAM);
    assert_eq!(bus.clocks(), 12);
    assert_eq!(cpu.regs.get_r(), 3);
    assert_eq!(
        bus.take_events(),
        [Event::Halt(true), Event::Pc(PROGRAM)].repeat(3)
    );
}

#[test]
fn int_leaves_halt() {
    let (mut cpu, mut bus) = machine(1, &[HALT]);
    for _ in 0..3 {
        cpu.emulate(&mut bus);
    }
    bus.take_events();
    bus.set_interrupt(true);

    cpu.emulate(&mut bus);

    assert!(!cpu.is_halted());
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM + 1);
    assert_eq!(cpu.regs.get_pc(), IM1_HANDLER + 1);
    assert_eq!(bus.clocks(), 12 + 13 + 4);
    assert_eq!(cpu.regs.get_r(), 5);
    assert_eq!(
        bus.take_events(),
        [Event::Halt(false), Event::Pc(IM1_HANDLER + 1)]
    );
}

#[test]
fn nmi_leaves_halt() {
    let (mut cpu, mut bus) = machine(1, &[HALT]);
    cpu.emulate(&mut bus);
    bus.take_events();
    bus.set_nmi(true);

    cpu.emulate(&mut bus);

    assert!(!cpu.is_halted());
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM + 1);
    assert_eq!(cpu.regs.get_pc(), NMI_HANDLER + 1);
    assert_eq!(bus.clocks(), 4 + 11 + 4);
    assert_eq!(
        bus.take_events(),
        [Event::Halt(false), Event::Pc(NMI_HANDLER + 1)]
    );
}

#[test]
fn halted_set_by_caller_is_left_by_interrupt() {
    let (mut cpu, mut bus) = machine(1, &[HALT]);
    cpu.halted = true;
    bus.set_interrupt(true);

    cpu.emulate(&mut bus);

    assert!(!cpu.is_halted());
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM + 1);
    assert_eq!(
        bus.take_events(),
        [Event::Halt(false), Event::Pc(IM1_HANDLER + 1)]
    );
}

#[test]
fn reti_tells_bus() {
    let (mut cpu, mut bus) = machine(1, &[PREFIX_ED, RETI]);
    cpu.regs.set_sp(STACK - 2);
    bus.load_to_memory(&[0x34, 0x12], STACK - 2);

    cpu.emulate(&mut bus);

    assert_eq!(cpu.regs.get_pc(), 0x1234);
    assert_eq!(bus.clocks(), 14);
    assert_eq!(bus.take_events(), [Event::Reti, Event::Pc(0x1234)]);
}

/// The five internal clocks of an NMI, each on the address bus as `addr`.
fn nmi_internal(addr: u16) -> [Wait; 5] {
    [Wait::NoMreq(addr, 1); 5]
}

#[test]
fn im1_interrupt_waits() {
    let (mut cpu, mut bus) = machine(1, &[NOP]);
    bus.set_interrupt(true);

    cpu.emulate(&mut bus);

    assert_eq!(
        bus.take_waits(),
        [
            Wait::Mreq(STACK - 1, 3),
            Wait::Mreq(STACK - 2, 3),
            Wait::Internal(7),
            Wait::Mreq(IM1_HANDLER, 4),
        ]
    );
}

#[test]
fn im2_interrupt_waits() {
    let (mut cpu, mut bus) = machine(2, &[NOP]);
    cpu.regs.set_i(0x90);
    bus.set_interrupt_data(0x20);
    bus.load_to_memory(&[0x00, 0xA0], 0x9020);
    bus.set_interrupt(true);

    cpu.emulate(&mut bus);

    assert_eq!(
        bus.take_waits(),
        [
            Wait::Mreq(STACK - 1, 3),
            Wait::Mreq(STACK - 2, 3),
            Wait::Mreq(0x9020, 3),
            Wait::Mreq(0x9021, 3),
            Wait::Internal(7),
            Wait::Mreq(0xA000, 4),
        ]
    );
}

#[test]
fn nmi_waits() {
    let (mut cpu, mut bus) = machine(1, &[NOP]);
    bus.set_nmi(true);

    cpu.emulate(&mut bus);

    let mut expected = nmi_internal(PROGRAM).to_vec();
    expected.extend([
        Wait::Mreq(STACK - 1, 3),
        Wait::Mreq(STACK - 2, 3),
        Wait::Mreq(NMI_HANDLER, 4),
    ]);
    assert_eq!(bus.take_waits(), expected);
}

/// Leaving a HALT moves the program counter past it before the NMI's internal clocks, so they
/// are on the address after the HALT.
#[test]
fn nmi_from_halt_waits() {
    let (mut cpu, mut bus) = machine(1, &[HALT]);
    cpu.emulate(&mut bus);
    assert_eq!(bus.take_waits(), [Wait::Mreq(PROGRAM, 4)]);
    bus.set_nmi(true);

    cpu.emulate(&mut bus);

    let mut expected = nmi_internal(PROGRAM + 1).to_vec();
    expected.extend([
        Wait::Mreq(STACK - 1, 3),
        Wait::Mreq(STACK - 2, 3),
        Wait::Mreq(NMI_HANDLER, 4),
    ]);
    assert_eq!(bus.take_waits(), expected);
}

#[test]
fn int_from_halt_waits() {
    let (mut cpu, mut bus) = machine(1, &[HALT]);
    cpu.emulate(&mut bus);
    cpu.emulate(&mut bus);
    assert_eq!(bus.take_waits(), [Wait::Mreq(PROGRAM, 4); 2]);
    bus.set_interrupt(true);

    cpu.emulate(&mut bus);

    assert_eq!(
        bus.take_waits(),
        [
            Wait::Mreq(STACK - 1, 3),
            Wait::Mreq(STACK - 2, 3),
            Wait::Internal(7),
            Wait::Mreq(IM1_HANDLER, 4),
        ]
    );
}

#[test]
fn prefix_chain_waits() {
    let (mut cpu, mut bus) = machine(1, &[PREFIX_DD, PREFIX_FD, LD_HL_NN, 0x34, 0x12]);

    cpu.emulate(&mut bus);
    assert_eq!(
        bus.take_waits(),
        [Wait::Mreq(PROGRAM, 4), Wait::Mreq(PROGRAM + 1, 4)]
    );
    cpu.emulate(&mut bus);
    assert_eq!(
        bus.take_waits(),
        [
            Wait::Mreq(PROGRAM + 2, 4),
            Wait::Mreq(PROGRAM + 3, 3),
            Wait::Mreq(PROGRAM + 4, 3),
        ]
    );
}

/// A program that counts in B and halts, interrupted in IM 2 every 1000 clocks by a handler that
/// counts in C, and once by an NMI whose handler counts in E. Pins the end state after `100_000`
/// clocks, so any change to how `emulate` runs, times or interrupts a program shows here.
#[test]
fn long_run_end_state() {
    let (mut cpu, mut bus) = machine(2, &[EI, INC_B, HALT, JR_E, (-5i8).cast_unsigned()]);
    cpu.regs.set_iff1(false);
    cpu.regs.set_iff2(false);
    cpu.regs.set_i(0x90);
    bus.set_interrupt_data(0xFF);
    bus.load_to_memory(&[0x00, 0xA0], 0x90FF);
    bus.load_to_memory(&[INC_C, EI, RET], 0xA000);
    // INC E; EI; RETN. The EI means RETN leaves IFF1 as it is, which upstream's rustzx-z80 and
    // ZX Sidekick's treat alike (see corrections.rs for RETN changing IFF1).
    bus.load_to_memory(&[0x1C, EI, PREFIX_ED, RETN], NMI_HANDLER);
    let mut nmi_taken = false;
    while bus.clocks() < 100_000 {
        bus.set_interrupt(bus.clocks() % 1000 < 32);
        let nmi_due = !nmi_taken && bus.clocks() >= 50_000;
        bus.set_nmi(nmi_due);
        cpu.emulate(&mut bus);
        if nmi_due && cpu.regs.get_pc() == NMI_HANDLER + 1 {
            nmi_taken = true;
        }
    }
    bus.set_nmi(false);

    let events = bus.take_events();
    let waits = bus.take_waits();
    let s = snapshot(&cpu);
    let stack: Vec<u8> = (STACK - 4..STACK).map(|a| bus.read_memory(a)).collect();

    assert!(nmi_taken);
    assert_eq!(cpu.regs.get_e(), 1);
    assert_eq!(bus.clocks(), LONG_RUN_CLOCKS);
    assert_eq!(s.bc, LONG_RUN_BC);
    assert_eq!(s.pc, LONG_RUN_PC);
    assert_eq!(s.sp, STACK);
    assert_eq!(s.r, LONG_RUN_R);
    assert_eq!(s.af, LONG_RUN_AF);
    assert_eq!(s.mem_ptr, LONG_RUN_MEM_PTR);
    assert_eq!((s.iff1, s.iff2, s.halted), LONG_RUN_IFF1_IFF2_HALTED);
    assert_eq!(stack, LONG_RUN_STACK);
    assert_eq!(events.len(), LONG_RUN_EVENTS);
    assert_eq!(waits.len(), LONG_RUN_WAITS);
    assert_eq!(wait_digest(&waits), LONG_RUN_WAIT_DIGEST);
    assert_eq!(
        events.iter().filter(|e| **e == Event::Halt(true)).count(),
        LONG_RUN_HALTS
    );
}

const LONG_RUN_CLOCKS: usize = 100_000;
const LONG_RUN_BC: u16 = 0x6463; // 100 loops, 99 interrupts
const LONG_RUN_PC: u16 = PROGRAM + 2;
const LONG_RUN_R: u8 = 0x57;
const LONG_RUN_AF: u16 = 0x0020;
const LONG_RUN_MEM_PTR: u16 = PROGRAM;
const LONG_RUN_IFF1_IFF2_HALTED: (bool, bool, bool) = (true, true, true);
const LONG_RUN_STACK: [u8; 4] = [0x00, 0x00, 0x03, 0x80];
const LONG_RUN_EVENTS: usize = 47_856;
const LONG_RUN_WAITS: usize = 25_475;
const LONG_RUN_WAIT_DIGEST: u64 = 0x1000_9fc3_c1e2_9cea;

/// FNV-1a over every wait's kind, address and clocks, so one number stands for the whole
/// sequence.
fn wait_digest(waits: &[Wait]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for wait in waits {
        let (kind, addr, clk) = match *wait {
            Wait::Mreq(addr, clk) => (0u8, addr, clk),
            Wait::NoMreq(addr, clk) => (1, addr, clk),
            Wait::Internal(clk) => (2, 0, clk),
        };
        for byte in [kind, (addr >> 8) as u8, addr as u8, clk as u8] {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}
const LONG_RUN_HALTS: usize = 23_579;
