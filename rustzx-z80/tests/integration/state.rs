//! Keeping, copying and restoring processor state.

use crate::{snapshot, TestingBus};
use rustzx_z80::{Regs, Z80};

const LD_A_N: u8 = 0x3E;
const INC_A: u8 = 0x3C;

#[test]
fn clone_is_independent_of_original() {
    let mut bus = TestingBus::new(0x10000);
    bus.record_events();
    bus.load_to_memory(&[LD_A_N, 0x42, INC_A], 0x8000);
    let mut cpu = Z80::default();
    cpu.regs.set_pc(0x8000);
    cpu.emulate(&mut bus);

    let copy = cpu.clone();
    cpu.emulate(&mut bus);

    assert_eq!(cpu.regs.get_acc(), 0x43);
    assert_eq!(cpu.regs.get_pc(), 0x8003);
    assert_eq!(copy.regs.get_acc(), 0x42);
    assert_eq!(copy.regs.get_pc(), 0x8002);
}

#[test]
fn clone_keeps_halted_skip_interrupt_and_mode() {
    let mut cpu = Z80::default();
    cpu.set_im(2);
    cpu.halted = true;
    cpu.skip_interrupt = true;
    cpu.regs.set_ix(0x1234);
    cpu.regs.set_iff2(true);

    let copy = cpu.clone();

    assert_eq!(snapshot(&copy), snapshot(&cpu));
    assert_eq!(u8::from(copy.get_im()), 2);
}

/// A clone taken in the middle of a prefix chain carries the prefix still to be applied, which
/// the public interface doesn't show.
#[test]
fn clone_mid_prefix_chain_finishes_the_chain() {
    // DD FD 21 34 12: LD IY,0x1234, after a chain.
    let mut bus = TestingBus::new(0x10000);
    bus.record_events();
    bus.load_to_memory(&[0xDD, 0xFD, 0x21, 0x34, 0x12], 0x8000);
    let mut cpu = Z80::default();
    cpu.regs.set_pc(0x8000);
    cpu.emulate(&mut bus);
    assert!(cpu.skip_interrupt);

    let mut copy = cpu.clone();
    let mut copy_bus = bus.clone();
    cpu.emulate(&mut bus);
    copy.emulate(&mut copy_bus);

    assert_eq!(copy.regs.get_iy(), 0x1234);
    assert_eq!(copy.regs.get_ix(), 0);
    assert_eq!(copy.regs.get_pc(), 0x8005);
    assert_eq!(snapshot(&copy), snapshot(&cpu));
    assert_eq!(copy_bus.clocks(), bus.clocks());
    assert_eq!(copy_bus.take_waits(), bus.take_waits());
}

#[test]
fn regs_clone_is_independent_of_original() {
    let mut regs = Regs::default();
    regs.set_bc(0x1234);
    regs.set_iff1(true);

    let copy = regs.clone();
    regs.set_bc(0x5678);
    regs.set_iff1(false);
    regs.inc_r();

    assert_eq!(copy.get_bc(), 0x1234);
    assert!(copy.get_iff1());
    assert_eq!(copy.get_r(), 0);
}

#[test]
fn clone_takes_interrupt_as_original() {
    let mut bus = TestingBus::new(0x10000);
    bus.record_events();
    bus.load_to_memory(&[0x76], 0x8000); // HALT
    bus.load_to_memory(&[0x0C], 0x0038); // INC C
    let mut cpu = Z80::default();
    cpu.set_im(1);
    cpu.regs.set_pc(0x8000);
    cpu.regs.set_sp(0xC000);
    cpu.regs.set_iff1(true);
    cpu.regs.set_iff2(true);
    cpu.emulate(&mut bus);
    bus.set_interrupt(true);

    let mut copy = cpu.clone();
    let mut copy_bus = bus.clone();
    cpu.emulate(&mut bus);
    copy.emulate(&mut copy_bus);

    assert_eq!(copy.regs.get_c(), 1);
    assert_eq!(snapshot(&copy), snapshot(&cpu));
    assert_eq!(copy_bus.memory(), bus.memory());
    assert_eq!(copy_bus.clocks(), bus.clocks());
    assert_eq!(copy_bus.take_waits(), bus.take_waits());
}
