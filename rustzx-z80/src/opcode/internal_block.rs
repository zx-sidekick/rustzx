use crate::{
    opcode::BlockDir,
    tables::{lookup8_r12, HALF_CARRY_SUB_TABLE, PARITY_TABLE, SZF3F5_TABLE},
    Z80Bus, FLAG_CARRY, FLAG_F3, FLAG_F5, FLAG_HALF_CARRY, FLAG_PV, FLAG_SIGN, FLAG_SUB, FLAG_ZERO,
    Z80,
};

/// ldi/ldd instruction group
pub fn execute_ldi_ldd(cpu: &mut Z80, bus: &mut impl Z80Bus, dir: BlockDir) {
    let src = bus.read(cpu.regs.get_hl(), 3);
    let bc = cpu.regs.dec_bc();
    bus.write(cpu.regs.get_de(), src, 3);
    bus.wait_loop(cpu.regs.get_de(), 2);
    match dir {
        BlockDir::Inc => {
            cpu.regs.inc_hl();
            cpu.regs.inc_de();
        }
        BlockDir::Dec => {
            cpu.regs.dec_hl();
            cpu.regs.dec_de();
        }
    }
    let mut flags = cpu.regs.get_flags();
    flags &= !(FLAG_SUB | FLAG_HALF_CARRY | FLAG_PV | FLAG_F3 | FLAG_F5);
    flags |= u8::from(bc != 0) * FLAG_PV;
    let src_plus_a = src.wrapping_add(cpu.regs.get_acc());
    flags |= u8::from(src_plus_a & 0x08 != 0) * FLAG_F3;
    flags |= u8::from(src_plus_a & 0x02 != 0) * FLAG_F5;
    cpu.regs.set_flags(flags);
    // Clocks: <4 + 4> + 3 + 3 + 2 = 16
}

/// cpi/cpd instruction group
pub fn execute_cpi_cpd(cpu: &mut Z80, bus: &mut impl Z80Bus, dir: BlockDir) -> bool {
    let src = bus.read(cpu.regs.get_hl(), 3);
    bus.wait_loop(cpu.regs.get_hl(), 5);
    match dir {
        BlockDir::Inc => {
            cpu.regs.inc_hl();
            cpu.regs.set_mem_ptr(cpu.regs.get_mem_ptr().wrapping_add(1));
        }
        BlockDir::Dec => {
            cpu.regs.dec_hl();
            cpu.regs.set_mem_ptr(cpu.regs.get_mem_ptr().wrapping_sub(1));
        }
    }
    let bc = cpu.regs.dec_bc();
    let acc = cpu.regs.get_acc();
    let tmp = acc.wrapping_sub(src);
    let mut flags = cpu.regs.get_flags() & FLAG_CARRY;
    flags |= FLAG_SUB;
    flags |= u8::from(bc != 0) * FLAG_PV;
    flags |= u8::from(tmp == 0) * FLAG_ZERO;
    flags |= tmp & FLAG_SIGN;
    let lookup = lookup8_r12(acc, src, tmp);
    let half_borrow = HALF_CARRY_SUB_TABLE[(lookup & 0x07) as usize];
    flags |= half_borrow;
    let tmp2 = if half_borrow != 0 {
        tmp.wrapping_sub(1)
    } else {
        tmp
    };
    flags |= u8::from((tmp2 & 0x08) != 0) * FLAG_F3;
    flags |= u8::from((tmp2 & 0x02) != 0) * FLAG_F5;
    cpu.regs.set_flags(flags);
    // Clocks: <4 + 4> + 3 + 5 = 16
    tmp == 0
}

/// INI/IND instruction group
///
/// Returns last written value at (HL)
pub fn execute_ini_ind(cpu: &mut Z80, bus: &mut impl Z80Bus, dir: BlockDir) -> u8 {
    bus.wait_no_mreq(cpu.regs.get_ir(), 1);
    let src = bus.read_io(cpu.regs.get_bc());
    bus.write(cpu.regs.get_hl(), src, 3);
    match dir {
        BlockDir::Inc => {
            cpu.regs.inc_hl();
            cpu.regs.set_mem_ptr(cpu.regs.get_bc().wrapping_add(1));
        }
        BlockDir::Dec => {
            cpu.regs.dec_hl();
            cpu.regs.set_mem_ptr(cpu.regs.get_bc().wrapping_sub(1));
        }
    }
    let b = cpu.regs.dec_b();
    let mut flags = 0u8;
    flags |= SZF3F5_TABLE[b as usize];
    flags |= u8::from((src & 0x80) != 0) * FLAG_SUB;
    let c = match dir {
        BlockDir::Inc => cpu.regs.get_c().wrapping_add(1),
        BlockDir::Dec => cpu.regs.get_c().wrapping_sub(1),
    };
    // (HL) + ( C (+ or -) 1) & 0xFF
    let (k, k_carry) = c.overflowing_add(src);
    flags |= u8::from(k_carry) * (FLAG_CARRY | FLAG_HALF_CARRY);
    // Parity of (k & 7) xor B is PV flag
    flags |= PARITY_TABLE[((k & 0x07) ^ b) as usize];
    cpu.regs.set_flags(flags);

    src
}

/// OUTI/OUTD instruction group
/// Returns last read value at (HL)
pub fn execute_outi_outd(cpu: &mut Z80, bus: &mut impl Z80Bus, dir: BlockDir) -> u8 {
    bus.wait_no_mreq(cpu.regs.get_ir(), 1);
    let src = bus.read(cpu.regs.get_hl(), 3);
    let b = cpu.regs.dec_b();

    match dir {
        BlockDir::Inc => {
            cpu.regs.inc_hl();
            cpu.regs.set_mem_ptr(cpu.regs.get_bc().wrapping_add(1));
        }
        BlockDir::Dec => {
            cpu.regs.dec_hl();
            cpu.regs.set_mem_ptr(cpu.regs.get_bc().wrapping_sub(1));
        }
    }

    bus.write_io(cpu.regs.get_bc(), src);

    let l = cpu.regs.get_l();
    let mut flags = 0u8;
    flags |= SZF3F5_TABLE[b as usize];
    flags |= u8::from((src & 0x80) != 0) * FLAG_SUB;
    // L + (HL)
    let (k, k_carry) = l.overflowing_add(src);
    flags |= u8::from(k_carry) * (FLAG_CARRY | FLAG_HALF_CARRY);
    // Parity of (k & 7) xor B is PV flag
    flags |= PARITY_TABLE[((k & 0x07) ^ b) as usize];
    cpu.regs.set_flags(flags);

    src
}
