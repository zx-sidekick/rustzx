//! What ZX Sidekick's rustzx-z80 corrects from upstream, each pinned to what the hardware does.
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
const EI: u8 = 0xFB;

const FLAG_PV: u8 = 0x04;
const IM1_HANDLER: u16 = 0x0038;
const NMI_HANDLER: u16 = 0x0066;
const PROGRAM: u16 = 0x8000;
const STACK: u16 = 0xC000;

fn return_address(cpu: &Z80, bus: &mut TestingBus) -> u16 {
    let sp = cpu.regs.get_sp();
    u16::from_le_bytes([bus.read_memory(sp), bus.read_memory(sp + 1)])
}

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

/// DD FD 21 34 12: a chain of prefixes and then LD IY,0x1234. No interrupt of either kind is taken
/// until the chain has its instruction.
const PREFIX_CHAIN: [u8; 6] = [0xDD, 0xFD, 0x21, 0x34, 0x12, NOP];

/// An NMI pulse that ends while a prefix chain holds interrupts off is still taken after it.
#[test]
fn nmi_pulse_during_prefix_chain_is_latched() {
    let (mut cpu, mut bus) = machine(&PREFIX_CHAIN);

    cpu.emulate(&mut bus);
    bus.set_nmi(true);
    cpu.emulate(&mut bus);
    bus.set_nmi(false);
    assert_eq!(cpu.regs.get_pc(), PROGRAM + 5);
    assert_eq!(cpu.regs.get_iy(), 0x1234);

    cpu.emulate(&mut bus);

    assert_eq!(cpu.regs.get_pc(), NMI_HANDLER + 1);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM + 5);
}

/// A clone carries a latched NMI with it.
#[test]
fn clone_keeps_latched_nmi() {
    let (mut cpu, mut bus) = machine(&PREFIX_CHAIN);
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

// --- EI and DI hold off only the maskable interrupt ---
//
// The Z80 does not accept a maskable interrupt straight after EI, but the NMI is not affected:
// redcode/Z80's NMI response has no such check, only its INT response does.

fn nmi_straight_after(first: u8) -> (Z80, TestingBus) {
    let (mut cpu, mut bus) = machine(&[first, NOP, NOP]);
    cpu.regs.set_iff1(false);
    cpu.regs.set_iff2(false);
    cpu.emulate(&mut bus);
    bus.set_nmi(true);
    cpu.emulate(&mut bus);
    (cpu, bus)
}

#[test]
fn nmi_is_taken_straight_after_ei() {
    let (cpu, mut bus) = nmi_straight_after(EI);

    assert_eq!(cpu.regs.get_pc(), NMI_HANDLER + 1);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM + 1);
    // EI set both; the NMI clears IFF1 only
    assert!(!cpu.regs.get_iff1());
    assert!(cpu.regs.get_iff2());
}

#[test]
fn nmi_is_taken_straight_after_di() {
    let (cpu, mut bus) = nmi_straight_after(DI);

    assert_eq!(cpu.regs.get_pc(), NMI_HANDLER + 1);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM + 1);
}

#[test]
fn int_still_waits_one_instruction_after_ei() {
    let (mut cpu, mut bus) = machine(&[EI, NOP, NOP]);
    cpu.regs.set_iff1(false);
    cpu.regs.set_iff2(false);
    cpu.emulate(&mut bus);
    bus.set_interrupt(true);

    assert_eq!(cpu.step(&mut bus), Step::Instruction);
    assert_eq!(cpu.step(&mut bus), Step::Interrupt);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM + 2);
}

// --- No second NMI straight after an NMI response ---
//
// The Z80 does not start a second NMI response straight after one; at least one instruction of
// the handler runs first (Manuel Sainz de Baranda y Goñi, 2022, checked with Visual Z80 Remix).

/// An edge seen at the first check after an NMI response (possible when that NMI was latched
/// earlier and the line had gone inactive) waits for one handler instruction, then is taken.
#[test]
fn nmi_edge_straight_after_nmi_waits_and_is_then_taken() {
    let (mut cpu, mut bus) = machine(&PREFIX_CHAIN);
    bus.load_to_memory(&[NOP; 4], NMI_HANDLER);
    cpu.emulate(&mut bus);
    bus.set_nmi(true);
    assert_eq!(cpu.step(&mut bus), Step::Instruction); // finishes the chain, NMI latched
    bus.set_nmi(false);
    assert_eq!(cpu.step(&mut bus), Step::Interrupt); // the latched NMI; the line is low
    let (mut other, mut other_bus) = (cpu.clone(), bus.clone());
    bus.set_nmi(true);
    other_bus.set_nmi(true);

    // step: the handler's first instruction, then the second NMI, then its first instruction
    assert_eq!(cpu.step(&mut bus), Step::Instruction);
    assert_eq!(cpu.regs.get_pc(), NMI_HANDLER + 1);
    assert_eq!(cpu.step(&mut bus), Step::Interrupt);
    assert_eq!(cpu.step(&mut bus), Step::Instruction);
    // emulate: the same in two calls
    other.emulate(&mut other_bus);
    other.emulate(&mut other_bus);

    assert_eq!(cpu.regs.get_sp(), STACK - 4);
    assert_eq!(snapshot(&cpu), snapshot(&other));
    assert_eq!(bus.clocks(), other_bus.clocks());
}

// --- RETI/RETN that change IFF1 hold off a maskable interrupt ---
//
// RETN and RETI copy IFF2 into IFF1 during the next instruction's opcode fetch, so when that
// changes IFF1 (only possible after an NMI) a maskable interrupt is not accepted straight after
// (Andre Weissflog, 2021; stardot.org.uk "New discovery on Z80 interrupts", 2022).

const RETN: u8 = 0x45;
const RETI: u8 = 0x4D;

/// Takes an NMI with interrupts enabled, returns from it with `ret` (RETN or RETI), and then
/// raises INT. Returns what the two steps after the return did, and where the interrupt returns
/// to.
fn int_after_return_from_nmi(ret: u8) -> (Step, Step, u16) {
    let (mut cpu, mut bus) = machine(&[NOP; 4]);
    bus.load_to_memory(&[PREFIX_ED, ret], NMI_HANDLER);
    bus.load_to_memory(&[NOP], IM1_HANDLER);

    bus.set_nmi(true);
    assert_eq!(cpu.step(&mut bus), Step::Interrupt);
    assert!(!cpu.regs.get_iff1());
    assert_eq!(cpu.step(&mut bus), Step::Instruction);
    assert_eq!(cpu.regs.get_pc(), PROGRAM);
    assert!(cpu.regs.get_iff1());

    bus.set_interrupt(true);
    let first = cpu.step(&mut bus);
    let second = cpu.step(&mut bus);
    (first, second, return_address(&cpu, &mut bus))
}

#[test]
fn retn_that_enables_interrupts_holds_int_one_instruction() {
    assert_eq!(
        int_after_return_from_nmi(RETN),
        (Step::Instruction, Step::Interrupt, PROGRAM + 1)
    );
}

#[test]
fn reti_that_enables_interrupts_holds_int_one_instruction() {
    assert_eq!(
        int_after_return_from_nmi(RETI),
        (Step::Instruction, Step::Interrupt, PROGRAM + 1)
    );
}

/// RETN that leaves IFF1 as it is holds nothing off.
#[test]
fn retn_that_keeps_iff1_does_not_hold_int() {
    let (mut cpu, mut bus) = machine(&[PREFIX_ED, RETN]);
    cpu.regs.set_sp(STACK - 2);
    bus.load_to_memory(&[0x00, 0x90], STACK - 2);
    bus.load_to_memory(&[NOP], IM1_HANDLER);

    assert_eq!(cpu.step(&mut bus), Step::Instruction);
    assert_eq!(cpu.regs.get_pc(), 0x9000);
    bus.set_interrupt(true);

    assert_eq!(cpu.step(&mut bus), Step::Interrupt);
    assert_eq!(return_address(&cpu, &mut bus), 0x9000);
}

// --- LD A,I and LD A,R ---
//
// On an NMOS Z80, if a maskable interrupt is accepted straight after LD A,I or LD A,R, the P/V
// flag they set from IFF2 reads as 0, because IFF2 is reset while it is being read (Zilog,
// "Z80 Family Data Book", 1989, pp. 412-413; the CMOS Z80 does not do it).

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

// --- A DD/FD prefix clears Q ---
//
// SCF and CCF set flags 3 and 5 from ((Q ^ F) | A), where Q is F if the previous instruction
// changed the flags and 0 if not. A DD or FD prefix that does not apply to the next opcode is an
// instruction of its own that leaves the flags alone, so it clears Q: redcode/Z80 runs a prefixed
// SCF/CCF through `xy_xcf`, which does `Q = 0` first, and SingleStepTests expects the same.

const F35: u8 = 0x28;

/// LD B,0x28; INC B (flags changed, F3 and F5 set, so Q = F); then `tail`. Returns F3/F5.
fn f35_after_inc_b_then(tail: &[u8]) -> u8 {
    let mut program = vec![0x06, 0x28, 0x04];
    program.extend_from_slice(tail);
    let (mut cpu, mut bus) = machine(&program);
    cpu.regs.set_iff1(false);
    while usize::from(cpu.regs.get_pc() - PROGRAM) < program.len() {
        cpu.emulate(&mut bus);
    }
    assert_eq!(cpu.regs.get_acc(), 0);
    cpu.regs.get_flags() & F35
}

#[test]
fn scf_after_flag_change_takes_f35_from_a_only() {
    // Q = F, so (Q ^ F) | A = A = 0
    assert_eq!(f35_after_inc_b_then(&[0x37]), 0);
    assert_eq!(f35_after_inc_b_then(&[0x3F]), 0);
}

#[test]
fn prefixed_scf_and_ccf_see_q_of_zero() {
    for tail in [
        &[0xDD, 0x37][..],
        &[0xFD, 0x37],
        &[0xDD, 0x3F],
        &[0xFD, 0x3F],
        &[0xDD, 0xFD, 0x37],
    ] {
        // Q = 0, so (Q ^ F) | A = F, which has F3 and F5 set
        assert_eq!(f35_after_inc_b_then(tail), F35, "{tail:02x?}");
    }
}

// --- Repeating block I/O sets MEMPTR to the instruction's address + 1 ---
//
// Like LDIR and CPIR, a repeating INIR, INDR, OTIR or OTDR sets MEMPTR to PC + 1 in the extra
// M-cycle that moves PC back (rofl0r 2022, Manuel Sainz de Baranda y Goñi 2023; redcode/Z80's
// INXR_OTXR_COMMON). When the instruction does not repeat, MEMPTR stays BC +/- 1 as for INI and
// friends.

/// Runs one step of ED `op` at PROGRAM with the given B and C. Returns (MEMPTR, PC).
fn block_io(op: u8, b: u8) -> (u16, u16) {
    let (mut cpu, mut bus) = machine(&[PREFIX_ED, op]);
    cpu.regs.set_bc(u16::from_be_bytes([b, 0x10]));
    cpu.regs.set_hl(0x9000);
    cpu.emulate(&mut bus);
    (cpu.regs.get_mem_ptr(), cpu.regs.get_pc())
}

#[test]
fn repeating_block_io_sets_mem_ptr_to_pc_plus_one() {
    for op in [0xB2, 0xBA, 0xB3, 0xBB] {
        assert_eq!(block_io(op, 2), (PROGRAM + 1, PROGRAM), "ED {op:02X}");
    }
}

#[test]
fn last_block_io_iteration_keeps_bc_mem_ptr() {
    // INIR/INDR: BC before B is decremented, +/- 1
    assert_eq!(block_io(0xB2, 1), (0x0111, PROGRAM + 2));
    assert_eq!(block_io(0xBA, 1), (0x010F, PROGRAM + 2));
    // OTIR/OTDR: BC after B is decremented, +/- 1
    assert_eq!(block_io(0xB3, 1), (0x0011, PROGRAM + 2));
    assert_eq!(block_io(0xBB, 1), (0x000F, PROGRAM + 2));
}

// --- More NMI cases ---

/// An NMI straight after a RETN that holds off the maskable interrupt is not held off itself.
#[test]
fn nmi_is_not_held_off_by_retn() {
    let (mut cpu, mut bus) = machine(&[NOP; 4]);
    bus.load_to_memory(&[PREFIX_ED, RETN], NMI_HANDLER);

    bus.set_nmi(true);
    assert_eq!(cpu.step(&mut bus), Step::Interrupt);
    bus.set_nmi(false);
    // RETN: IFF1 goes from 0 back to 1, which holds off INT for one instruction
    assert_eq!(cpu.step(&mut bus), Step::Instruction);
    assert!(cpu.skip_interrupt);

    bus.set_nmi(true);
    assert_eq!(cpu.step(&mut bus), Step::Interrupt);
    assert_eq!(cpu.regs.get_pc(), NMI_HANDLER);
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM);
}

/// An NMI line held active while the processor is halted takes it out of the HALT once, not
/// again on every step.
#[test]
fn held_nmi_leaves_halt_once() {
    let (mut cpu, mut bus) = machine(&[HALT]);
    bus.load_to_memory(&[NOP; 8], NMI_HANDLER);
    assert_eq!(cpu.step(&mut bus), Step::Instruction);
    assert!(cpu.is_halted());

    bus.set_nmi(true);
    assert_eq!(cpu.step(&mut bus), Step::Interrupt);
    assert!(!cpu.is_halted());
    assert_eq!(return_address(&cpu, &mut bus), PROGRAM + 1);
    for _ in 0..4 {
        assert_eq!(cpu.step(&mut bus), Step::Instruction);
    }
    assert_eq!(cpu.regs.get_sp(), STACK - 2);
}
