//! Keeping, copying and restoring processor state.

use crate::TestingBus;
use rustzx_z80::Z80;

const LD_A_N: u8 = 0x3E;
const INC_A: u8 = 0x3C;

#[test]
fn clone_is_independent_of_original() {
    let mut bus = TestingBus::new(0x10000);
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
