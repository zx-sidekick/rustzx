//! Z80 CPU module

use crate::{
    opcode::{
        execute_bits, execute_extended, execute_normal, execute_pop_16, execute_push_16, Opcode,
        Prefix,
    },
    RegName16, Regs, Z80Bus,
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

/// Z80 Processor struct
#[derive(Clone)]
pub struct Z80 {
    /// Contains Z80 registers data
    pub regs: Regs,
    /// Set while the processor is halted, waiting for an interrupt. The program counter stays at
    /// the `HALT` instruction, which runs again on every step, and moves past it when an
    /// interrupt is taken; so the handler returns to the instruction after `HALT`.
    pub halted: bool,
    /// Set by an instruction after which an interrupt may not be taken yet (`EI`, `DI`, or a
    /// `DD`/`FD` prefix followed by another prefix). The next step runs an instruction without
    /// checking for interrupts, and clears it.
    pub skip_interrupt: bool,
    /// type of interrupt
    pub(crate) int_mode: IntMode,
    active_prefix: Prefix,
}

impl Default for Z80 {
    fn default() -> Self {
        Self {
            regs: Regs::default(),
            halted: false,
            skip_interrupt: false,
            int_mode: IntMode::Im0,
            active_prefix: Prefix::None,
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
    pub fn is_halted(&self) -> bool {
        self.halted
    }

    /// Returns current interrupt mode
    pub fn get_im(&self) -> IntMode {
        self.int_mode
    }

    /// Changes interrupt mode
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

    /// Takes an NMI or maskable interrupt if one is due. Returns whether one was taken.
    fn handle_interrupt(&mut self, bus: &mut impl Z80Bus) -> bool {
        if bus.nmi_active() {
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
        } else if bus.int_active() && self.regs.get_iff1() {
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
                    let addr = (((self.regs.get_i() as u16) << 8) & 0xFF00)
                        | ((bus.read_interrupt() as u16) & 0x00FF);
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
        if !self.skip_interrupt {
            self.handle_interrupt(bus)
        } else {
            // allow interrupts again
            self.skip_interrupt = false;
            false
        }
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
    /// the same timing, as long as the bus stops reporting an NMI once it has been taken. The
    /// difference is that after an interrupt the program counter is at the handler before any
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
        // Actions to be performed before any opcode execution
        let before_execute_opcode = |cpu: &mut Self| {
            // Save Q register value from previous emulation step, which is later used to
            // properly calculate flags in some instructions
            cpu.regs.step_q();
        };

        let byte1 = if self.active_prefix != Prefix::None {
            let tmp = self.active_prefix.to_byte().unwrap();
            self.active_prefix = Prefix::None;
            tmp
        } else {
            self.regs.inc_r();
            self.fetch_byte(bus, 4)
        };
        let prefix_hi = Prefix::from_byte(byte1);
        if prefix_hi != Prefix::None {
            match prefix_hi {
                prefix_single @ Prefix::DD | prefix_single @ Prefix::FD => {
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
                    };
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
            };
        } else {
            let opcode = Opcode::from_byte(byte1);
            before_execute_opcode(self);
            execute_normal(self, bus, opcode, Prefix::None);
        };
        // Allow bus implementation to process pc-based events
        bus.pc_callback(self.regs.get_pc());
    }
}
