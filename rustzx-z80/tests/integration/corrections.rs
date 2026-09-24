//! Behaviour this fork corrects from upstream, each pinned to what the hardware does.
//!
//! Unlike `emulate.rs`, these do not pass on upstream `master`.

use crate::{snapshot, Event, TestingBus};
use rustzx_z80::{CodeGenerator, CodegenMemorySpace, Regs, Step, Z80};

const EXX: u8 = 0xD9;
const HALT: u8 = 0x76;
const NOP: u8 = 0x00;
const LD_NN_A: u8 = 0x32;
const OUT_N_A: u8 = 0xD3;
const PREFIX_ED: u8 = 0xED;
const LD_A_I: u8 = 0x57;
const LD_A_R: u8 = 0x5F;
const DI: u8 = 0xF3;

const FLAG_PV: u8 = 0x04;
const IM1_HANDLER: u16 = 0x0038;
const NMI_HANDLER: u16 = 0x0066;
const PROGRAM: u16 = 0x8000;
const STACK: u16 = 0xC000;

fn machine(program: &[u8]) -> (Z80, TestingBus) {
    let mut bus = TestingBus::new(0x10000);
    bus.load_to_memory(program, PROGRAM);
    bus.record_events();
    let mut cpu = Z80::default();
    cpu.set_im(1);
    cpu.regs.set_pc(PROGRAM);
    cpu.regs.set_sp(STACK);
    cpu.regs.set_iff1(true);
    cpu.regs.set_iff2(true);
    (cpu, bus)
}

// --- HL' ---

#[test]
fn alternate_hl_getters_read_hl_alt() {
    let mut regs = Regs::default();
    regs.set_hl(0x1234);
    regs.exx();

    assert_eq!(regs.get_h_alt(), 0x12);
    assert_eq!(regs.get_l_alt(), 0x34);
    assert_eq!(regs.get_hl(), 0);
}

/// The test snapshot reads HL' through those getters, so it has to see EXX.
#[test]
fn snapshot_sees_exx() {
    let (mut cpu, mut bus) = machine(&[EXX]);
    cpu.regs.set_bc(0x1111);
    cpu.regs.set_de(0x2222);
    cpu.regs.set_hl(0x3333);

    cpu.emulate(&mut bus);

    let s = snapshot(&cpu);
    assert_eq!((s.bc, s.de, s.hl), (0, 0, 0));
    assert_eq!((s.bc_alt, s.de_alt, s.hl_alt), (0x1111, 0x2222, 0x3333));
}

// --- MEMPTR ---
//
// From "MEMPTR, esoteric register of the Zilog Z80 CPU" (boo_boo, Vladimir Kladov):
// LD (addr),A: MEMPTR_low = (addr + 1) & 0xFF, MEMPTR_hi = A
// OUT (n),A:   MEMPTR_low = (n + 1) & 0xFF,    MEMPTR_hi = A

fn mem_ptr_after(program: &[u8], acc: u8) -> u16 {
    let (mut cpu, mut bus) = machine(program);
    cpu.regs.set_acc(acc);
    cpu.emulate(&mut bus);
    cpu.regs.get_mem_ptr()
}

#[test]
fn ld_nn_a_mem_ptr() {
    assert_eq!(mem_ptr_after(&[LD_NN_A, 0x34, 0x12], 0x56), 0x5635);
    assert_eq!(mem_ptr_after(&[LD_NN_A, 0xFF, 0x12], 0x40), 0x4000);
    assert_eq!(mem_ptr_after(&[LD_NN_A, 0x00, 0xFF], 0x00), 0x0001);
}

#[test]
fn out_n_a_mem_ptr() {
    assert_eq!(mem_ptr_after(&[OUT_N_A, 0xFE], 0x12), 0x12FF);
    assert_eq!(mem_ptr_after(&[OUT_N_A, 0xFF], 0x40), 0x4000);
}

// --- Code generation ---

struct Ram(Vec<u8>);

impl CodegenMemorySpace for Ram {
    fn write_byte(&mut self, addr: u16, byte: u8) {
        self.0[addr as usize] = byte;
    }
}

#[test]
fn codegen_write_word_is_little_endian_over_two_addresses() {
    let mut ram = Ram(vec![0; 0x10000]);

    ram.write_word(0x8000, 0x1234);

    assert_eq!(ram.0[0x8000..0x8002], [0x34, 0x12]);
}

#[test]
fn codegen_write_word_wraps_at_top_of_memory() {
    let mut ram = Ram(vec![0; 0x10000]);

    ram.write_word(0xFFFF, 0x1234);

    assert_eq!((ram.0[0xFFFF], ram.0[0x0000]), (0x34, 0x12));
}

/// Code put into memory by the emulator is not a memory access by the processor, so it must not
/// wait (and on a contended machine, be delayed).
#[test]
fn codegen_into_bus_does_not_wait() {
    let mut bus = TestingBus::new(0x10000);
    bus.record_events();

    CodeGenerator::new(&mut bus)
        .codegen_set_addr(0x4000)
        .jump(0x1234);
    CodegenMemorySpace::write_word(&mut bus, 0x5000, 0xBEEF);

    assert_eq!(bus.memory()[0x4000..0x4003], [0xC3, 0x34, 0x12]);
    assert_eq!(bus.memory()[0x5000..0x5002], [0xEF, 0xBE]);
    assert_eq!(bus.take_waits(), []);
    assert_eq!(bus.clocks(), 0);
}

/// Debug builds panic on overflowing `+=`; the address space wraps instead.
#[test]
fn codegen_wraps_at_top_of_memory() {
    let mut ram = Ram(vec![0; 0x10000]);

    CodeGenerator::new(&mut ram)
        .codegen_set_addr(0xFFFE)
        .jump(0x1234);

    assert_eq!(
        (ram.0[0xFFFE], ram.0[0xFFFF], ram.0[0x0000]),
        (0xC3, 0x34, 0x12)
    );
}

// --- NMI is edge-triggered ---
//
// The Z80 latches the falling edge of /NMI and takes one interrupt for it, however long the
// line is then held.

#[test]
fn held_nmi_is_taken_once_by_emulate() {
    let (mut cpu, mut bus) = machine(&[NOP; 8]);
    bus.load_to_memory(&[NOP; 8], NMI_HANDLER);
    bus.set_nmi(true);

    for _ in 0..5 {
        cpu.emulate(&mut bus);
    }

    assert_eq!(cpu.regs.get_sp(), STACK - 2);
    assert_eq!(cpu.regs.get_pc(), NMI_HANDLER + 5);
}

#[test]
fn held_nmi_is_taken_once_by_step() {
    let (mut cpu, mut bus) = machine(&[NOP; 8]);
    bus.load_to_memory(&[NOP; 8], NMI_HANDLER);
    bus.set_nmi(true);

    assert_eq!(cpu.step(&mut bus), Step::Interrupt);
    for _ in 0..4 {
        assert_eq!(cpu.step(&mut bus), Step::Instruction);
    }
    assert_eq!(cpu.regs.get_sp(), STACK - 2);
}

#[test]
fn nmi_raised_again_is_taken_again() {
    let (mut cpu, mut bus) = machine(&[NOP; 8]);
    bus.load_to_memory(&[NOP; 8], NMI_HANDLER);

    bus.set_nmi(true);
    cpu.emulate(&mut bus);
    cpu.emulate(&mut bus);
    bus.set_nmi(false);
    cpu.emulate(&mut bus);
    bus.set_nmi(true);
    cpu.emulate(&mut bus);

    assert_eq!(cpu.regs.get_sp(), STACK - 4);
}

/// An NMI pulse that ends while interrupts are held off (here, after DI) is still taken once
/// they are allowed again.
#[test]
fn nmi_pulse_during_deferral_is_latched() {
    let (mut cpu, mut bus) = machine(&[DI, NOP, NOP]);

    cpu.emulate(&mut bus);
    bus.set_nmi(true);
    cpu.emulate(&mut bus);
    bus.set_nmi(false);
    assert_eq!(cpu.regs.get_pc(), PROGRAM + 2);

    cpu.emulate(&mut bus);

    assert_eq!(cpu.regs.get_pc(), NMI_HANDLER + 1);
    assert_eq!(
        u16::from_le_bytes([
            bus.read_memory(cpu.regs.get_sp()),
            bus.read_memory(cpu.regs.get_sp() + 1)
        ]),
        PROGRAM + 2
    );
}

/// A clone carries a latched NMI with it.
#[test]
fn clone_keeps_latched_nmi() {
    let (mut cpu, mut bus) = machine(&[DI, NOP, NOP]);
    cpu.emulate(&mut bus);
    bus.set_nmi(true);
    cpu.emulate(&mut bus);
    bus.set_nmi(false);

    let mut copy = cpu.clone();
    let mut copy_bus = bus.clone();
    cpu.emulate(&mut bus);
    copy.emulate(&mut copy_bus);

    assert_eq!(copy.regs.get_pc(), NMI_HANDLER + 1);
    assert_eq!(snapshot(&copy), snapshot(&cpu));
}

// --- LD A,I and LD A,R ---
//
// On an NMOS Z80, if a maskable interrupt is accepted straight after LD A,I or LD A,R, the P/V
// flag they set from IFF2 reads as 0, because IFF2 is reset while it is being read (Fuse
// emulates this; the CMOS Z80 does not do it).

fn pv_after_interrupt(program: &[u8], int_after: usize, nmi: bool) -> bool {
    let (mut cpu, mut bus) = machine(program);
    bus.load_to_memory(&[HALT], IM1_HANDLER);
    bus.load_to_memory(&[HALT], NMI_HANDLER);
    for _ in 0..int_after {
        cpu.emulate(&mut bus);
    }
    assert!(
        cpu.regs.get_flags() & FLAG_PV != 0,
        "P/V is set from IFF2 first"
    );
    if nmi {
        bus.set_nmi(true);
    } else {
        bus.set_interrupt(true);
    }
    assert_eq!(cpu.step(&mut bus), Step::Interrupt);
    cpu.regs.get_flags() & FLAG_PV != 0
}

#[test]
fn interrupt_right_after_ld_a_i_resets_pv() {
    assert!(!pv_after_interrupt(&[PREFIX_ED, LD_A_I, NOP], 1, false));
}

#[test]
fn interrupt_right_after_ld_a_r_resets_pv() {
    assert!(!pv_after_interrupt(&[PREFIX_ED, LD_A_R, NOP], 1, false));
}

#[test]
fn interrupt_an_instruction_later_keeps_pv() {
    assert!(pv_after_interrupt(&[PREFIX_ED, LD_A_I, NOP], 2, false));
}

#[test]
fn nmi_right_after_ld_a_i_keeps_pv() {
    // An NMI leaves IFF2 alone, so there is nothing to race with.
    assert!(pv_after_interrupt(&[PREFIX_ED, LD_A_I, NOP], 1, true));
}

#[test]
fn ld_a_i_then_interrupt_same_both_ways() {
    let (mut cpu, mut bus) = machine(&[PREFIX_ED, LD_A_I, NOP]);
    bus.load_to_memory(&[NOP, NOP], IM1_HANDLER);
    cpu.emulate(&mut bus);
    bus.set_interrupt(true);
    let (mut other, mut other_bus) = (cpu.clone(), bus.clone());

    cpu.emulate(&mut bus);
    assert_eq!(other.step(&mut other_bus), Step::Interrupt);
    assert_eq!(other.step(&mut other_bus), Step::Instruction);

    assert_eq!(snapshot(&cpu), snapshot(&other));
    let events = bus.take_events();
    let mut other_events = other_bus.take_events();
    other_events.retain(|e| *e != Event::Pc(IM1_HANDLER));
    assert_eq!(events, other_events);
}
