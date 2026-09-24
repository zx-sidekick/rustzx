mod alu;
mod corrections;
mod emulate;
mod fuse_flag_cases;
mod interrupt;
mod run_until;
mod stack;
mod state;
mod step;
mod zexall;

use rustzx_z80::{Z80Bus, Z80};
use std::collections::HashSet;

/// Something the processor told the bus, other than memory and clocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Pc(u16),
    Halt(bool),
    Reti,
}

/// A wait the processor asked the bus for. Where the address is given, it is what a contended
/// machine would delay on, so it matters as much as the clocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    Mreq(u16, usize),
    NoMreq(u16, usize),
    Internal(usize),
}

#[derive(Clone)]
pub struct TestingBus {
    memory: Vec<u8>,
    breakpoints: HashSet<u16>,
    last_breakpoint: Option<u16>,
    interrupt: bool,
    /// INT active for `.1` T-states out of every `.0`, from the bus's own clock
    interrupt_period: Option<(usize, usize)>,
    nmi: bool,
    interrupt_data: u8,
    clocks: usize,
    events: Option<Vec<Event>>,
    waits: Option<Vec<Wait>>,
}

impl TestingBus {
    #[must_use]
    pub fn new(memory_size: usize) -> Self {
        Self {
            memory: vec![0; memory_size],
            breakpoints: HashSet::default(),
            last_breakpoint: None,
            interrupt: false,
            interrupt_period: None,
            nmi: false,
            interrupt_data: 0,
            clocks: 0,
            events: None,
            waits: None,
        }
    }

    pub fn load_to_memory(&mut self, data: &[u8], base_address: u16) {
        let start = base_address as usize;
        let end = start + data.len();
        self.memory.as_mut_slice()[start..end].copy_from_slice(data);
    }

    pub fn patch_memory(&mut self, address: u16, data: u8) {
        self.memory[address as usize] = data;
    }

    pub fn read_memory(&mut self, address: u16) -> u8 {
        self.memory[address as usize]
    }

    pub fn add_breakpoint(&mut self, address: u16) {
        self.breakpoints.insert(address);
    }

    pub fn last_breakpoint(&mut self) -> Option<u16> {
        self.last_breakpoint.take()
    }

    /// Holds the maskable interrupt line active, or releases it.
    pub fn set_interrupt(&mut self, active: bool) {
        self.interrupt = active;
    }

    /// Makes the maskable interrupt line active for `length` T-states out of every `period`,
    /// following the bus's clock, as a machine's frame interrupt does.
    ///
    /// # Panics
    ///
    /// Panics if `period` is 0.
    pub fn set_interrupt_period(&mut self, period: usize, length: usize) {
        assert!(period > 0, "an interrupt period of 0 T-states");
        self.interrupt_period = Some((period, length));
    }

    /// Holds the non-maskable interrupt line active, or releases it.
    pub fn set_nmi(&mut self, active: bool) {
        self.nmi = active;
    }

    /// The byte put on the data bus when a maskable interrupt is acknowledged.
    pub fn set_interrupt_data(&mut self, data: u8) {
        self.interrupt_data = data;
    }

    /// Clocks (T-states) waited so far.
    #[must_use]
    pub fn clocks(&self) -> usize {
        self.clocks
    }

    /// Starts recording events and waits. Off by default, so long runs such as zexall don't pile
    /// them up.
    pub fn record_events(&mut self) {
        self.events.get_or_insert_with(Vec::new);
        self.waits.get_or_insert_with(Vec::new);
    }

    /// The waits recorded since the last call, oldest first.
    pub fn take_waits(&mut self) -> Vec<Wait> {
        self.waits.as_mut().map(std::mem::take).unwrap_or_default()
    }

    fn wait(&mut self, wait: Wait) {
        self.clocks += match wait {
            Wait::Mreq(_, clk) | Wait::NoMreq(_, clk) | Wait::Internal(clk) => clk,
        };
        if let Some(waits) = &mut self.waits {
            waits.push(wait);
        }
    }

    /// The events recorded since the last call, oldest first.
    pub fn take_events(&mut self) -> Vec<Event> {
        self.events.as_mut().map(std::mem::take).unwrap_or_default()
    }

    fn push_event(&mut self, event: Event) {
        if let Some(events) = &mut self.events {
            events.push(event);
        }
    }

    /// All of memory.
    #[must_use]
    pub fn memory(&self) -> &[u8] {
        &self.memory
    }
}

/// Everything about a processor that its public interface shows.
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the processor's flip-flops and flags"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub af: u16,
    pub bc: u16,
    pub de: u16,
    pub hl: u16,
    pub af_alt: u16,
    pub bc_alt: u16,
    pub de_alt: u16,
    pub hl_alt: u16,
    pub ix: u16,
    pub iy: u16,
    pub sp: u16,
    pub pc: u16,
    pub i: u8,
    pub r: u8,
    pub mem_ptr: u16,
    pub last_q: u8,
    pub iff1: bool,
    pub iff2: bool,
    pub im: u8,
    pub halted: bool,
    pub skip_interrupt: bool,
}

#[must_use]
pub fn snapshot(cpu: &Z80) -> State {
    let r = &cpu.regs;
    let pair = |h: u8, l: u8| u16::from_be_bytes([h, l]);
    State {
        af: r.get_af(),
        bc: r.get_bc(),
        de: r.get_de(),
        hl: r.get_hl(),
        af_alt: pair(r.get_acc_alt(), r.get_flags_alt()),
        bc_alt: pair(r.get_b_alt(), r.get_c_alt()),
        de_alt: pair(r.get_d_alt(), r.get_e_alt()),
        hl_alt: pair(r.get_h_alt(), r.get_l_alt()),
        ix: r.get_ix(),
        iy: r.get_iy(),
        sp: r.get_sp(),
        pc: r.get_pc(),
        i: r.get_i(),
        r: r.get_r(),
        mem_ptr: r.get_mem_ptr(),
        last_q: r.get_last_q(),
        iff1: r.get_iff1(),
        iff2: r.get_iff2(),
        im: cpu.get_im().into(),
        halted: cpu.halted,
        skip_interrupt: cpu.skip_interrupt,
    }
}

impl Z80Bus for TestingBus {
    fn read_internal(&mut self, addr: u16) -> u8 {
        self.memory[addr as usize]
    }

    fn write_internal(&mut self, addr: u16, data: u8) {
        self.memory[addr as usize] = data;
    }

    fn pc_callback(&mut self, addr: u16) {
        self.push_event(Event::Pc(addr));
        if self.breakpoints.contains(&addr) {
            self.last_breakpoint = Some(addr);
        }
    }

    fn read_io(&mut self, _port: u16) -> u8 {
        0
    }

    fn write_io(&mut self, _port: u16, _data: u8) {}

    fn wait_mreq(&mut self, addr: u16, clk: usize) {
        self.wait(Wait::Mreq(addr, clk));
    }

    fn wait_no_mreq(&mut self, addr: u16, clk: usize) {
        self.wait(Wait::NoMreq(addr, clk));
    }

    fn wait_internal(&mut self, clk: usize) {
        self.wait(Wait::Internal(clk));
    }

    fn read_interrupt(&mut self) -> u8 {
        self.interrupt_data
    }

    fn reti(&mut self) {
        self.push_event(Event::Reti);
    }

    fn halt(&mut self, halted: bool) {
        self.push_event(Event::Halt(halted));
    }

    fn int_active(&self) -> bool {
        match self.interrupt_period {
            Some((period, length)) => self.clocks % period < length,
            None => self.interrupt,
        }
    }

    fn nmi_active(&self) -> bool {
        self.nmi
    }
}
