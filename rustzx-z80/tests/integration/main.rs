mod interrupt;
mod state;
mod zexall;

use rustzx_z80::Z80Bus;
use std::collections::HashSet;

pub struct TestingBus {
    memory: Vec<u8>,
    breakpoints: HashSet<u16>,
    last_breakpoint: Option<u16>,
    interrupt: bool,
    clocks: usize,
}

impl TestingBus {
    pub fn new(memory_size: usize) -> Self {
        Self {
            memory: vec![0; memory_size as usize],
            breakpoints: Default::default(),
            last_breakpoint: None,
            interrupt: false,
            clocks: 0,
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

    /// Clocks (T-states) waited so far.
    pub fn clocks(&self) -> usize {
        self.clocks
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
        if self.breakpoints.contains(&addr) {
            self.last_breakpoint = Some(addr);
        }
    }

    fn read_io(&mut self, _port: u16) -> u8 {
        0
    }

    fn write_io(&mut self, _port: u16, _data: u8) {}

    fn wait_mreq(&mut self, _addr: u16, clk: usize) {
        self.clocks += clk;
    }

    fn wait_no_mreq(&mut self, _addr: u16, clk: usize) {
        self.clocks += clk;
    }

    fn wait_internal(&mut self, clk: usize) {
        self.clocks += clk;
    }

    fn read_interrupt(&mut self) -> u8 {
        0
    }

    fn reti(&mut self) {}

    fn halt(&mut self, _halted: bool) {}

    fn int_active(&self) -> bool {
        self.interrupt
    }

    fn nmi_active(&self) -> bool {
        false
    }
}
