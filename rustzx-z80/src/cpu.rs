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
    /// of prefixes does.
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
    /// the same timing. The difference is that after an interrupt the program counter is at the handler before any
    /// of it has run. That is where a caller can see that an interrupt happened, keep the state
    /// from just before its handler, or stop at a breakpoint on the handler's first instruction.
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
