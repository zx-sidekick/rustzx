//! A rough speed check: runs a block copy (`LDIR`) and a mix of other instructions for a number
//! of steps, and prints how long each took, best of three.
//!
//! ```text
//! cargo run --release -p rustzx-z80 --example speed [steps]
//! ```
//!
//! Differences of a few percent between builds are often code alignment rather than the change
//! being measured; to compare two versions fairly, build both with
//! `RUSTFLAGS="-C llvm-args=-align-all-functions=6 -C llvm-args=-align-all-nofallthru-blocks=5"`.

use rustzx_z80::{Z80Bus, Z80};
use std::time::Instant;

/// 64K of RAM with no contention; counts T-states.
struct Bus {
    mem: Vec<u8>,
    clocks: usize,
}

impl Z80Bus for Bus {
    fn read_internal(&mut self, addr: u16) -> u8 {
        self.mem[addr as usize]
    }
    fn write_internal(&mut self, addr: u16, data: u8) {
        self.mem[addr as usize] = data;
    }
    fn wait_mreq(&mut self, _addr: u16, clk: usize) {
        self.clocks += clk;
    }
    fn wait_no_mreq(&mut self, _addr: u16, clk: usize) {
        self.clocks += clk;
    }
    fn wait_internal(&mut self, clk: usize) {
        self.clocks += clk;
    }
    fn read_io(&mut self, _port: u16) -> u8 {
        0xFF
    }
    fn write_io(&mut self, _port: u16, _data: u8) {}
    fn read_interrupt(&mut self) -> u8 {
        0xFF
    }
    fn reti(&mut self) {}
    fn halt(&mut self, _halted: bool) {}
    fn int_active(&self) -> bool {
        false
    }
    fn nmi_active(&self) -> bool {
        false
    }
    fn pc_callback(&mut self, _addr: u16) {}
}

/// Runs `program` at 0x8000 for `steps` calls of `emulate`; returns the seconds taken.
fn run(name: &str, program: &[u8], steps: usize) -> f64 {
    let mut bus = Bus {
        mem: vec![0; 0x10000],
        clocks: 0,
    };
    bus.mem[0x8000..0x8000 + program.len()].copy_from_slice(program);
    let mut cpu = Z80::default();
    cpu.regs.set_pc(0x8000);
    cpu.regs.set_sp(0xFF00);
    let start = Instant::now();
    for _ in 0..steps {
        cpu.emulate(&mut bus);
    }
    let secs = start.elapsed().as_secs_f64();
    // A checksum of the registers, memory and time, so two builds can be seen to have done the
    // same work: FNV-1a over every byte of memory, then the registers and the clock
    let r = &cpu.regs;
    let mut checksum: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bus.mem.iter().copied().chain(
        [
            r.get_af(),
            r.get_bc(),
            r.get_de(),
            r.get_hl(),
            r.get_sp(),
            r.get_pc(),
        ]
        .into_iter()
        .flat_map(u16::to_le_bytes),
    ) {
        checksum = (checksum ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    checksum ^= bus.clocks as u64;
    println!("{name}: {secs:.3}s (checksum {checksum:x})");
    secs
}

fn main() {
    let steps = std::env::args()
        .nth(1)
        .map_or(200_000_000, |s| s.parse().expect("steps: a number"));
    // LD HL,0x4000; LD DE,0xA000; LD BC,0x2200; LDIR; JR back
    let copy = [
        0x21, 0x00, 0x40, 0x11, 0x00, 0xA0, 0x01, 0x00, 0x22, 0xED, 0xB0, 0x18, 0xF3,
    ];
    // LD HL,0x4000; LD B,0; then LD A,(HL); ADD A,(HL); LD (HL),A; INC HL; PUSH BC; POP BC;
    // ADD HL,BC; DJNZ; JR back
    let mix = [
        0x21, 0x00, 0x40, 0x06, 0x00, 0x7E, 0x86, 0x77, 0x23, 0xC5, 0xC1, 0x09, 0x10, 0xF7, 0x18,
        0xF0,
    ];
    let mut best = [f64::MAX; 2];
    for _ in 0..3 {
        best[0] = best[0].min(run("block copy", &copy, steps));
        best[1] = best[1].min(run("mix       ", &mix, steps));
    }
    println!("best of 3: block copy {:.3}s, mix {:.3}s", best[0], best[1]);
}
