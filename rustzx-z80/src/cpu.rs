//! Z80 CPU module

use crate::{
    opcode::{
        execute_bits, execute_extended, execute_normal, execute_pop_16, execute_push_16, Opcode,
        Prefix,
    },
    RegName16, Regs, Z80Bus, FLAG_PV,
};

/// Interrupt mode enum
#[derive(Debug, Clone, Copy)]
pub enum IntMode {
    Im0,
    Im1,
    Im2,
}

impl From<IntMode> for u8 {
    fn from(mode: IntMode) -> Self {
        match mode {
            IntMode::Im0 => 0,
            IntMode::Im1 => 1,
            IntMode::Im2 => 2,
        }
    }
}

/// What one call to [`Z80::step`] did
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// An interrupt (NMI or maskable) was taken. The program counter is at
    /// its handler, whose first instruction has not run yet.
    Interrupt,
    /// One instruction ran.
    Instruction,
}

/// A set of addresses to stop at, for [`Z80::run_until`]: one bit for each of the 65,536.
///
/// It is 8 KB; keep one in a long-lived struct, or box it, rather than building one per call.
#[derive(Clone, PartialEq, Eq)]
pub struct Breakpoints {
    bits: [u64; 1024],
}

impl Default for Breakpoints {
    fn default() -> Self {
        Self { bits: [0; 1024] }
    }
}

impl Breakpoints {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `addr`.
    pub fn insert(&mut self, addr: u16) {
        self.bits[usize::from(addr >> 6)] |= 1 << (addr & 63);
    }

    /// Removes `addr`.
    pub fn remove(&mut self, addr: u16) {
        self.bits[usize::from(addr >> 6)] &= !(1 << (addr & 63));
    }

    /// Whether `addr` is in the set.
    #[must_use]
    #[inline]
    pub fn contains(&self, addr: u16) -> bool {
        self.bits[usize::from(addr >> 6)] & (1 << (addr & 63)) != 0
    }

    /// Removes every address.
    pub fn clear(&mut self) {
        self.bits = [0; 1024];
    }
}

impl core::fmt::Debug for Breakpoints {
    /// The addresses in the set, rather than 1,024 words of bits.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_set()
            .entries((0..=0xFFFF_u16).filter(|&addr| self.contains(addr)))
            .finish()
    }
}

impl FromIterator<u16> for Breakpoints {
    fn from_iter<I: IntoIterator<Item = u16>>(addrs: I) -> Self {
        let mut set = Self::new();
        for addr in addrs {
            set.insert(addr);
        }
        set
    }
}

/// Why [`Z80::run_until`] stopped
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The limit was reached; nothing ran after it was.
    Limit,
    /// An instruction left the program counter on this address, one of the breakpoints. It is
    /// the next instruction to run.
    Breakpoint(u16),
    /// An interrupt was taken: the program counter is at its handler, none of which has run.
    /// This is reported instead of a breakpoint on the handler's address, so a caller that
    /// wants one there should look at the program counter.
    Interrupt,
}

/// The NMI input, which is edge-triggered: the line going active latches one NMI, however long it
/// then stays active, and the NMI waits in the latch until it can be taken.
///
/// The Z80 does not start a second NMI response straight after one: at least one instruction of
/// the handler runs first. (An edge during the response itself is lost on the chip; here it waits
/// for that instruction instead, since `emulate` cannot tell it from an edge during the
/// instruction, and `step` must run a program as `emulate` does.)
#[derive(Clone, Default)]
struct NmiLatch {
    /// Level of the line when it was last sampled
    line: bool,
    /// The line has gone active since the last NMI was taken
    pending: bool,
    /// An NMI has just been taken and no instruction has run since
    responded: bool,
}

impl NmiLatch {
    fn sample(&mut self, line: bool) {
        if line && !self.line {
            self.pending = true;
        }
        self.line = line;
    }

    /// Returns whether an NMI is due now, and if so clears it.
    fn take(&mut self) -> bool {
        if self.responded || !self.pending {
            return false;
        }
        self.pending = false;
        self.responded = true;
        true
    }

    fn instruction_ran(&mut self) {
        self.responded = false;
    }
}

/// Z80 Processor struct
///
/// Besides the public fields, it keeps some state that only exists between two steps and that
/// no snapshot format records: a prefix still to be applied in a chain of `DD`/`FD` prefixes,
/// the NMI latch (the line's last level, a pending NMI, and whether one was just taken), and
/// whether the last instruction was `LD A,I` or `LD A,R`. `Clone` copies all of it. A processor
/// rebuilt from the public fields, starting from [`Z80::default`], starts with all of it clear,
/// which can differ from the original: for example, an NMI line held active across the rebuild
/// looks like a new edge to the rebuilt processor.
#[derive(Clone)]
pub struct Z80 {
    /// Contains Z80 registers data
    pub regs: Regs,
    /// Set while the processor is halted, waiting for an interrupt. The program counter stays at
    /// the `HALT` instruction, which runs again on every step, and moves past it when an
    /// interrupt is taken; so the handler returns to the instruction after `HALT`.
    pub halted: bool,
    /// Set by an instruction after which a maskable interrupt may not be taken yet: `EI`, `DI`,
    /// `RETI` or `RETN` when it changes IFF1 (which only happens after an NMI), or a `DD`/`FD`
    /// prefix followed by another prefix. The next step runs an instruction without taking a
    /// maskable interrupt, and clears it. It does not hold off an NMI; only an unfinished chain
    /// of prefixes does. (After `DI`, and inside a chain of prefixes, it changes nothing more,
    /// since IFF1 is already clear or the chain holds interrupts off itself; it is set there as
    /// upstream sets it.)
    pub skip_interrupt: bool,
    /// type of interrupt
    pub(crate) int_mode: IntMode,
    active_prefix: Prefix,
    nmi: NmiLatch,
    /// The last instruction was `LD A,I` or `LD A,R`, which copy IFF2 into P/V
    pub(crate) iff2_read: bool,
}

impl Default for Z80 {
    fn default() -> Self {
        Self {
            regs: Regs::default(),
            halted: false,
            skip_interrupt: false,
            int_mode: IntMode::Im0,
            active_prefix: Prefix::None,
            nmi: NmiLatch::default(),
            iff2_read: false,
        }
    }
}

impl Z80 {
    /// Reads byte from memory and increments PC
    #[inline]
    pub(crate) fn fetch_byte(&mut self, bus: &mut impl Z80Bus, clk: usize) -> u8 {
        let addr = self.regs.get_pc();
        self.regs.inc_pc();
        bus.read(addr, clk)
    }

    /// Reads word from memory and increments PC twice
    #[inline]
    pub(crate) fn fetch_word(&mut self, bus: &mut impl Z80Bus, clk: usize) -> u16 {
        let (hi_addr, lo_addr);
        lo_addr = self.regs.get_pc();
        let lo = bus.read(lo_addr, clk);
        hi_addr = self.regs.inc_pc();
        let hi = bus.read(hi_addr, clk);
        self.regs.inc_pc();
        u16::from_le_bytes([lo, hi])
    }

    /// Checks is cpu halted
    #[must_use]
    pub fn is_halted(&self) -> bool {
        self.halted
    }

    /// Returns current interrupt mode
    #[must_use]
    pub fn get_im(&self) -> IntMode {
        self.int_mode
    }

    /// Changes interrupt mode
    ///
    /// # Panics
    ///
    /// Panics if `value` is not 0, 1 or 2.
    pub fn set_im(&mut self, value: u8) {
        assert!(value < 3);
        self.int_mode = match value {
            0 => IntMode::Im0,
            1 => IntMode::Im1,
            2 => IntMode::Im2,
            _ => unreachable!(),
        }
    }

    /// Pops program counter to the stack. Exposed as a public crate interface to support
    /// 48K SNA loading in `rustzx-core` and fast tape loaders (Perform RET)
    pub fn pop_pc_from_stack(&mut self, bus: &mut impl Z80Bus) {
        execute_pop_16(self, bus, RegName16::PC, 0);
    }

    /// Pushes program counter from the stack. Exposed as a public crate interface to support
    /// 48K SNA saving in `rustzx-core`
    pub fn push_pc_to_stack(&mut self, bus: &mut impl Z80Bus) {
        execute_push_16(self, bus, RegName16::PC, 0);
    }

    /// Pushes `value` onto the stack, as `PUSH` does: SP goes down by 2, and `value` is stored
    /// with its low byte at SP and its high byte at SP + 1.
    ///
    /// For a caller acting on the processor between instructions (not while [`Z80::step`] has
    /// left a chain of `DD`/`FD` prefixes unfinished). Memory is written with
    /// [`Z80Bus::write_internal`], so no time passes and nothing is contended.
    pub fn push(&mut self, bus: &mut impl Z80Bus, value: u16) {
        let [low, high] = value.to_le_bytes();
        let sp = self.regs.dec_sp();
        bus.write_internal(sp, high);
        let sp = self.regs.dec_sp();
        bus.write_internal(sp, low);
    }

    /// Pops a value off the stack, as `POP` does: reads its low byte at SP and its high byte at
    /// SP + 1, and moves SP up by 2.
    ///
    /// For a caller acting on the processor between instructions (not while [`Z80::step`] has
    /// left a chain of `DD`/`FD` prefixes unfinished). Memory is read with
    /// [`Z80Bus::read_internal`], so no time passes and nothing is contended.
    pub fn pop(&mut self, bus: &mut impl Z80Bus) -> u16 {
        let low = bus.read_internal(self.regs.get_sp());
        let high = bus.read_internal(self.regs.inc_sp());
        self.regs.inc_sp();
        u16::from_le_bytes([low, high])
    }

    /// Returns from a subroutine as `RET` does: pops PC, sets MEMPTR to it, and tells the bus
    /// through [`Z80Bus::pc_callback`]. Like any instruction that leaves the flags alone, it
    /// makes a following `SCF` or `CCF` see Q = 0, ends the moment straight after `LD A,I` or
    /// `LD A,R` (so a maskable interrupt taken next keeps P/V), and counts as the instruction an
    /// NMI handler must run before a second NMI.
    ///
    /// For a caller that answers a routine itself and then returns from it, between
    /// instructions (not while [`Z80::step`] has left a chain of `DD`/`FD` prefixes unfinished).
    /// No opcode is fetched, so R is unchanged and no time passes; memory is read as by
    /// [`Z80::pop`].
    pub fn ret(&mut self, bus: &mut impl Z80Bus) {
        let pc = self.pop(bus);
        self.regs.set_pc(pc);
        self.regs.set_mem_ptr(pc);
        self.regs.clear_q();
        self.iff2_read = false;
        self.nmi.instruction_ran();
        bus.pc_callback(pc);
    }

    /// Takes an NMI if one is due, or else a maskable interrupt if one is due and not held off.
    /// Returns whether one was taken.
    fn handle_interrupt(&mut self, bus: &mut impl Z80Bus, int_held: bool) -> bool {
        if self.nmi.take() {
            // q resets during interrupt
            self.regs.clear_q();
            // Release halt line on the bus
            if self.halted {
                bus.halt(false);
                self.halted = false;
                self.regs.inc_pc();
            }
            // push pc and set pc to 0x0066
            bus.wait_loop(self.regs.get_pc(), 5);
            self.regs.set_iff1(false);
            // 3 x 2 clocks consumed
            execute_push_16(self, bus, RegName16::PC, 3);
            self.regs.set_pc(0x0066);
            self.iff2_read = false;

            // mem_ptr is set to PC
            self.regs.set_mem_ptr(self.regs.get_pc());

            self.regs.inc_r();
            // 5 + 3 + 3 = 11 clocks
            true
        } else if !int_held && bus.int_active() && self.regs.get_iff1() {
            // On an NMOS Z80, accepting the interrupt resets IFF2 while `LD A,I` or `LD A,R`
            // is still copying it into P/V, so P/V reads 0
            if self.iff2_read {
                self.regs.set_flags(self.regs.get_flags() & !FLAG_PV);
                self.iff2_read = false;
            }
            // q resets during interrupt
            self.regs.clear_q();
            // Release halt line on the bus
            if self.halted {
                bus.halt(false);
                self.halted = false;
                self.regs.inc_pc();
            }
            self.regs.inc_r();
            self.regs.set_iff1(false);
            self.regs.set_iff2(false);
            match self.int_mode {
                // For zx spectrum both Im0 and Im1 are same
                IntMode::Im0 | IntMode::Im1 => {
                    execute_push_16(self, bus, RegName16::PC, 3);
                    self.regs.set_pc(0x0038);

                    // 3 + 3 + 7 = 13 clocks
                    bus.wait_internal(7);
                }
                // jump using interrupt vector
                IntMode::Im2 => {
                    execute_push_16(self, bus, RegName16::PC, 3);
                    // build interrupt vector
                    let addr = ((u16::from(self.regs.get_i()) << 8) & 0xFF00)
                        | (u16::from(bus.read_interrupt()) & 0x00FF);
                    let addr = bus.read_word(addr, 3);
                    self.regs.set_pc(addr);
                    bus.wait_internal(7);
                    // 3 + 3 + 3 + 3 + 7 = 19 clocks
                }
            }
            // mem_ptr is set to PC
            self.regs.set_mem_ptr(self.regs.get_pc());
            true
        } else {
            false
        }
    }

    /// Takes an interrupt if one is due and may be taken now. Returns whether one was taken.
    fn check_interrupt(&mut self, bus: &mut impl Z80Bus) -> bool {
        self.nmi.sample(bus.nmi_active());
        let int_held = core::mem::take(&mut self.skip_interrupt);
        // No interrupt of either kind is taken until a chain of prefixes has its instruction
        if self.active_prefix != Prefix::None {
            return false;
        }
        self.handle_interrupt(bus, int_held)
    }

    /// Perform next emulation step
    ///
    /// Takes a pending interrupt first, if one is due, and then runs one instruction. When an
    /// interrupt is taken, the first instruction of its handler therefore runs in the same call.
    /// Use [`Z80::step`] to have the interrupt as a step of its own.
    pub fn emulate(&mut self, bus: &mut impl Z80Bus) {
        self.check_interrupt(bus);
        self.execute_instruction(bus);
    }

    /// Perform next emulation step, with an interrupt as a step of its own
    ///
    /// Either takes a pending interrupt, if one is due, or runs one instruction; never both.
    /// Calling `step` repeatedly runs a program as calling [`Z80::emulate`] repeatedly does, with
    /// the same timing. The difference is that after an interrupt the program counter is at the
    /// handler before any of it has run. That is where a caller can see that an interrupt
    /// happened, keep the state from just before its handler, or stop at a breakpoint on the
    /// handler's first instruction.
    ///
    /// After an interrupt, [`Z80Bus::pc_callback`] is called with the handler's address, as it
    /// is after an instruction.
    pub fn step(&mut self, bus: &mut impl Z80Bus) -> Step {
        if self.check_interrupt(bus) {
            bus.pc_callback(self.regs.get_pc());
            Step::Interrupt
        } else {
            self.execute_instruction(bus);
            Step::Instruction
        }
    }

    /// Runs [`Z80::step`] until `limit` says so, an instruction leaves the program counter on a
    /// breakpoint, or an interrupt is taken; says which.
    ///
    /// It is the loop a caller would write around `step`, written once here (it runs no faster
    /// than a caller's own, since `step` is compiled into the caller's crate either way):
    ///
    /// ```text
    /// loop {
    ///     if limit(bus) { return Stop::Limit }
    ///     match step(bus) {
    ///         Step::Interrupt => return Stop::Interrupt,
    ///         Step::Instruction if !mid_prefix_chain && breakpoints.contains(pc) => {
    ///             return Stop::Breakpoint(pc)
    ///         }
    ///         Step::Instruction => {}
    ///     }
    /// }
    /// ```
    ///
    /// `limit` is asked before each step; it usually compares the bus's clock with an end time,
    /// since only the bus knows how many T-states have passed (`|bus| bus.clocks() >= end`).
    /// Breakpoints are checked after each instruction, not before the first, so calling
    /// `run_until` again after it stopped at one carries on from there. They are not checked in
    /// the middle of a chain of `DD`/`FD` prefixes, where the program counter is inside an
    /// instruction that hasn't run yet. A breakpoint on a `HALT` stops the run on every step
    /// while halted (every 4 T-states), since the program counter stays on it.
    /// [`Z80Bus::pc_callback`] is still called after every step.
    #[must_use]
    pub fn run_until<B: Z80Bus>(
        &mut self,
        bus: &mut B,
        breakpoints: &Breakpoints,
        mut limit: impl FnMut(&B) -> bool,
    ) -> Stop {
        loop {
            if limit(bus) {
                return Stop::Limit;
            }
            if self.step(bus) == Step::Interrupt {
                return Stop::Interrupt;
            }
            let pc = self.regs.get_pc();
            if self.active_prefix == Prefix::None && breakpoints.contains(pc) {
                return Stop::Breakpoint(pc);
            }
        }
    }

    /// Runs one instruction, or one more prefix of a chain of them, without looking at
    /// interrupts
    fn execute_instruction(&mut self, bus: &mut impl Z80Bus) {
        self.iff2_read = false;
        self.nmi.instruction_ran();
        // Actions to be performed before any opcode execution
        let before_execute_opcode = |cpu: &mut Self| {
            // Save Q register value from previous emulation step, which is later used to
            // properly calculate flags in some instructions
            cpu.regs.step_q();
        };

        let byte1 = if self.active_prefix == Prefix::None {
            self.regs.inc_r();
            self.fetch_byte(bus, 4)
        } else {
            let tmp = self.active_prefix.to_byte().unwrap();
            self.active_prefix = Prefix::None;
            tmp
        };
        let prefix_hi = Prefix::from_byte(byte1);
        if prefix_hi == Prefix::None {
            let opcode = Opcode::from_byte(byte1);
            before_execute_opcode(self);
            execute_normal(self, bus, opcode, Prefix::None);
        } else {
            match prefix_hi {
                prefix_single @ (Prefix::DD | Prefix::FD) => {
                    let byte2 = self.fetch_byte(bus, 4);
                    self.regs.inc_r();
                    let prefix_lo = Prefix::from_byte(byte2);
                    match prefix_lo {
                        Prefix::DD | Prefix::ED | Prefix::FD => {
                            self.active_prefix = prefix_lo;
                            self.skip_interrupt = true;
                        }
                        Prefix::CB => {
                            before_execute_opcode(self);
                            execute_bits(self, bus, prefix_single);
                        }
                        Prefix::None => {
                            let opcode = Opcode::from_byte(byte2);
                            // The prefix is an instruction of its own that leaves the flags
                            // alone, so the opcode after it sees Q = 0 (it matters to SCF/CCF)
                            self.regs.clear_q();
                            before_execute_opcode(self);
                            execute_normal(self, bus, opcode, prefix_single);
                        }
                    }
                }
                Prefix::CB => {
                    // opcode will be read in the called
                    before_execute_opcode(self);
                    execute_bits(self, bus, Prefix::None);
                }
                Prefix::ED => {
                    let byte2 = self.fetch_byte(bus, 4);
                    self.regs.inc_r();
                    let opcode = Opcode::from_byte(byte2);
                    before_execute_opcode(self);
                    execute_extended(self, bus, opcode);
                }
                _ => unreachable!(),
            }
        }
        // Allow bus implementation to process pc-based events
        bus.pc_callback(self.regs.get_pc());
    }
}
