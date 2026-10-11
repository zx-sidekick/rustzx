use crate::{
    emulator::Emulator,
    error::SnapshotLoadError,
    host::{DataRecorder, Host, LoadableAsset, SeekFrom, SeekableAsset},
    zx::{
        joy::kempston, machine::ZXMachine, mouse::kempston::KempstonMouse, video::colors::ZXColor,
    },
    Result,
};

use alloc::vec::Vec;
use alloc::{str::from_utf8, vec};
#[cfg(feature = "zlib")]
use miniz_oxide::inflate::decompress_to_vec_zlib_with_limit;

use rustzx_z80::IntMode;

const ZXST_MID_128K: u32 = 2;

const ZXSTZF_EILAST: u32 = 1;
const ZXSTZF_HALTED: u32 = 2;
const ZXSTZF_FSET: u32 = 4;

#[cfg(all(feature = "sound", feature = "ay"))]
const ZXSTAYF_128AY: u32 = 2;

const ZXSTKJT_KEMPSTON: u32 = 1;

const ZXSTM_KEMPSTON: u32 = 2;

const ZXSTRF_COMPRESSED: u32 = 1;

const ZXST_HEADER_SIZE: usize = 8; // The zx-state header
const ZXST_BLOCK_HEADER_SIZE: usize = 8; // The header for each block

// The Creator (CRTR) block (szCreator[32], chMajorVersion, chMinorVersion, chData[]) only says
// which program wrote the snapshot, so nothing of it is read. It was read one byte past the
// spec's 36, which panicked on a block of exactly that size.

// Process ZXSTZ80REGS (Z80R) block
fn process_z80r_block<H: Host>(emulator: &mut Emulator<H>, block_data: &[u8]) {
    // AF
    emulator
        .cpu
        .regs
        .set_af(u16::from_le_bytes([block_data[0], block_data[1]]));

    // BC
    emulator
        .cpu
        .regs
        .set_bc(u16::from_le_bytes([block_data[2], block_data[3]]));

    // DE
    emulator
        .cpu
        .regs
        .set_de(u16::from_le_bytes([block_data[4], block_data[5]]));

    // HL
    emulator
        .cpu
        .regs
        .set_hl(u16::from_le_bytes([block_data[6], block_data[7]]));

    // Set alternate regs by swapping with main.
    // AF1
    emulator.cpu.regs.swap_af_alt();
    emulator
        .cpu
        .regs
        .set_af(u16::from_le_bytes([block_data[8], block_data[9]]));
    emulator.cpu.regs.swap_af_alt();

    emulator.cpu.regs.exx();
    // BC1
    emulator
        .cpu
        .regs
        .set_bc(u16::from_le_bytes([block_data[10], block_data[11]]));

    // DE1
    emulator
        .cpu
        .regs
        .set_de(u16::from_le_bytes([block_data[12], block_data[13]]));

    // HL1
    emulator
        .cpu
        .regs
        .set_hl(u16::from_le_bytes([block_data[14], block_data[15]]));

    emulator.cpu.regs.exx();

    // IX
    emulator
        .cpu
        .regs
        .set_ix(u16::from_le_bytes([block_data[16], block_data[17]]));

    // IY
    emulator
        .cpu
        .regs
        .set_iy(u16::from_le_bytes([block_data[18], block_data[19]]));

    // SP
    emulator
        .cpu
        .regs
        .set_sp(u16::from_le_bytes([block_data[20], block_data[21]]));

    // PC
    emulator
        .cpu
        .regs
        .set_pc(u16::from_le_bytes([block_data[22], block_data[23]]));

    // I
    emulator.cpu.regs.set_i(block_data[24]);

    // R
    emulator.cpu.regs.set_r(block_data[25]);

    // IFF1
    emulator.cpu.regs.set_iff1(block_data[26] > 0);

    // IFF2
    emulator.cpu.regs.set_iff2(block_data[27] > 0);

    // IM
    emulator.cpu.set_im(block_data[28]);

    // dwCyclesStart
    emulator.controller.frame_clocks = u32::from_le_bytes([
        block_data[29],
        block_data[30],
        block_data[31],
        block_data[32],
    ]) as usize;

    // chHoldIntReqCycles
    // Ignored block_data 33

    // chFlags
    let flags = block_data[34] as u32;
    emulator.cpu.skip_interrupt = flags & ZXSTZF_EILAST != 0;
    emulator.cpu.halted = flags & ZXSTZF_HALTED != 0;

    if emulator.cpu.halted {
        emulator.cpu.regs.inc_pc();
    }

    // v1.5
    if flags & ZXSTZF_FSET != 0 {
        emulator.cpu.regs.set_q()
    } else {
        emulator.cpu.regs.clear_q()
    };

    // wMemPtr
    emulator
        .cpu
        .regs
        .set_mem_ptr(u16::from_le_bytes([block_data[35], block_data[36]]));
}

// Process ZXSTSPECREGS (SPCR) block
fn process_spcr_block<H: Host>(emulator: &mut Emulator<H>, machine_id: u32, block_data: &[u8]) {
    // ch7ffd
    if machine_id < ZXST_MID_128K {
        emulator.controller.write_7ffd(0); // Always 0 for 16k and 48k
    } else {
        emulator.controller.write_7ffd(block_data[1]);
    }

    // ch1ffd
    // For +2a/+3 and Scorpion models.
    // Only 128 and 48k models supported currently. Skipping block_data[2] (union)

    // chEff7
    // For Pentagon 1024 model.
    // Only 128 and 48k models supported currently. Skipping block_data[2] (union)

    // chFe: the beeper's EAR and MIC bits. Set directly: an OUT through the bus would charge
    // its I/O time to the frame, whose T-state count the Z80R block has already set.
    #[cfg(feature = "sound")]
    {
        let fe = block_data[3];
        emulator
            .controller
            .mixer
            .beeper
            .change_state(fe & 0x10 != 0, fe & 0x08 != 0);
    }

    // chBorder, through the border's own setter so a precise border is drawn from it too
    let clocks = emulator.controller.frame_clocks;
    emulator
        .controller
        .set_border_color(clocks, ZXColor::from_bits(block_data[0]));
}

// Process ZXSTAYBLOCK (AY00)
#[cfg(all(feature = "sound", feature = "ay"))]
fn process_ay_block<H: Host>(emulator: &mut Emulator<H>, machine_id: u32, block_data: &[u8]) {
    // chFlags
    let flags = block_data[0] as u32;
    if machine_id < ZXST_MID_128K {
        // If AY needs enabling and it isn't enabled already, enable it.
        if (flags & ZXSTAYF_128AY != 0) && (!emulator.settings.ay_enabled) {
            emulator.set_ay_enabled(true);
        }
        // AY needs disabling and is enabled currently, disable it.
        else if (flags & ZXSTAYF_128AY == 0) && (emulator.settings.ay_enabled) {
            emulator.set_ay_enabled(false);
        }
    }

    if emulator.settings.ay_enabled {
        // chCurrentRegister
        let ay_reg = block_data[1];
        emulator.controller.mixer.ay.select_reg(ay_reg);

        // chAyRegs
        emulator.controller.mixer.ay.set_regs(&block_data[2..]);
    }
}

// Process ZXSTKEYB (KEYB)
fn process_keyb_block<H: Host>(emulator: &mut Emulator<H>, block_data: &[u8]) {
    // dwFlags
    // ignored for now as only issue 2 is emulated
    let _flags = u32::from_le_bytes([block_data[0], block_data[1], block_data[2], block_data[3]]);

    // chKeyboardJoystick
    let joystick = block_data[4] as u32;
    if joystick & ZXSTKJT_KEMPSTON != 0 {
        emulator.controller.kempston = Some(kempston::KempstonJoy::default())
    } else {
        emulator.controller.kempston = None;
    }
}

// Process ZXSTMOUSE (AMXM)
fn process_amxm_block<H: Host>(emulator: &mut Emulator<H>, block_data: &[u8]) {
    // chType
    // Only Kempston mouse is supported
    let mouse = block_data[0] as u32;
    if mouse != 0 {
        if mouse & ZXSTM_KEMPSTON > 0 {
            emulator.controller.mouse = Some(KempstonMouse::default());
        } else {
            emulator.controller.mouse = None;
        }
    } else {
        emulator.controller.mouse = None;
    }
}

// Process ZXSTRAMPAGE (RAMP)
fn process_ramp_block<H: Host>(
    emulator: &mut Emulator<H>,
    machine_id: u32,
    block_data: &[u8],
) -> Result<()> {
    // wFlags
    let flags = u16::from_le_bytes([block_data[0], block_data[1]]) as u32;

    // chPageNo
    let mut page_num = block_data[2];
    // Remap page numbers for 16k/48k machines
    if machine_id < ZXST_MID_128K {
        page_num = match page_num {
            5 => 0,
            2 => 1,
            0 => 2,
            _ => page_num,
        };
    }

    let page_data = emulator.controller.memory.ram_page_data_mut(page_num);

    if flags & ZXSTRF_COMPRESSED != 0 {
        if cfg!(not(feature = "zlib")) {
            return Err(SnapshotLoadError::ZlibNotSupported.into());
        }
        #[cfg(feature = "zlib")]
        {
            let compressed_data: Vec<u8> = block_data[3..].to_vec();
            match decompress_zlib_stream(&compressed_data) {
                Ok(data) => {
                    return {
                        page_data.copy_from_slice(&data[..page_data.len()]);
                        Ok(())
                    }
                }
                Err(_) => return Err(SnapshotLoadError::InvalidSZXFile.into()),
            }
        }
    } else {
        let uncompressed_data: Vec<u8> = block_data[3..].to_vec();
        page_data.copy_from_slice(&uncompressed_data[..page_data.len()]);
    }

    Ok(())
}

#[cfg(feature = "zlib")]
fn decompress_zlib_stream(bytes: &[u8]) -> Result<Vec<u8>> {
    const MAX_SIZE: usize = 65535;
    if let Ok(data) = decompress_to_vec_zlib_with_limit(bytes, MAX_SIZE) {
        Ok(data)
    } else {
        Err(SnapshotLoadError::InvalidSZXFile.into())
    }
}

/// SZX snapshot loading function
pub fn load<H, A>(emulator: &mut Emulator<H>, mut asset: A) -> Result<()>
where
    H: Host,
    A: LoadableAsset + SeekableAsset,
{
    let _ = asset.seek(SeekFrom::End(0))?;
    let mut cursor_pos = 0;
    asset.seek(SeekFrom::Start(0))?;

    // ZXST Header
    let mut header = [0u8; ZXST_HEADER_SIZE];
    asset.read_exact(&mut header)?;
    cursor_pos += ZXST_HEADER_SIZE;
    let magic_bytes = &[header[0], header[1], header[2], header[3]];
    let magic_str = from_utf8(magic_bytes);

    match magic_str {
        Ok(magic_data) => {
            if !magic_data.eq("ZXST") {
                return Err(SnapshotLoadError::InvalidSZXFile.into());
            }
        }
        Err(_) => return Err(SnapshotLoadError::InvalidSZXFile.into()),
    }
    let _ = header[4];
    let _ = header[5];

    let machine_id = header[6] as u32;
    if machine_id > ZXST_MID_128K {
        return Err(SnapshotLoadError::MachineNotSupported.into());
    }

    // ZXST Block Header
    asset.seek(SeekFrom::Start(cursor_pos))?;
    let mut block_header = [0u8; ZXST_BLOCK_HEADER_SIZE];
    while asset.read_exact(&mut block_header).is_ok() {
        let size: u32 = u32::from_le_bytes([
            block_header[4],
            block_header[5],
            block_header[6],
            block_header[7],
        ]);
        let id_bytes = &[
            block_header[0],
            block_header[1],
            block_header[2],
            block_header[3],
        ];
        let id_str = from_utf8(id_bytes).unwrap().to_uppercase();
        cursor_pos += ZXST_BLOCK_HEADER_SIZE;

        // ZXST Block Data
        asset.seek(SeekFrom::Start(cursor_pos))?;
        let mut block_data = vec![0; size as usize];

        if asset.read_exact(&mut block_data).is_err() {
            return Err(SnapshotLoadError::InvalidSZXFile.into());
        }

        match id_str.as_str() {
            "Z80R" => {
                process_z80r_block(emulator, &block_data);
            }
            "SPCR" => {
                process_spcr_block(emulator, machine_id, &block_data);
            }
            #[cfg(all(feature = "sound", feature = "ay"))]
            "AY\0\0" => {
                process_ay_block(emulator, machine_id, &block_data);
            }
            "KEYB" => {
                process_keyb_block(emulator, &block_data);
            }
            "AMXM" => {
                process_amxm_block(emulator, &block_data);
            }
            "RAMP" => {
                process_ramp_block(emulator, machine_id, &block_data)?;
            }

            _ => (),
        }
        // skip block data
        cursor_pos += size as usize;

        asset.seek(SeekFrom::Start(cursor_pos))?;
    }
    emulator.controller.refresh_memory_dependent_devices();
    Ok(())
}

/// The creator name written in the CRTR block
const CREATOR: &[u8] = b"rustzx";

const ZXST_MAJOR: u8 = 1;
/// 1.5 for the FSET flag (Q)
const ZXST_MINOR: u8 = 5;
const ZXST_MID_48K: u8 = 1;

/// SZX snapshot saving function: the inverse of [`load`], so a saved machine loads back as it was
///
/// Writes CRTR, Z80R, SPCR, AY (when the AY is on), KEYB, AMXM and a RAMP block for each RAM
/// page, compressed with zlib when the `zlib` feature is on. The tape and its position aren't
/// saved.
pub fn save<H, R>(emulator: &mut Emulator<H>, mut recorder: R) -> Result<()>
where
    H: Host,
    R: DataRecorder,
{
    let is_48k = emulator.controller.machine == ZXMachine::Sinclair48K;
    let mut out = Vec::new();

    // ZXST header
    out.extend_from_slice(b"ZXST");
    out.push(ZXST_MAJOR);
    out.push(ZXST_MINOR);
    out.push(if is_48k {
        ZXST_MID_48K
    } else {
        ZXST_MID_128K as u8
    });
    out.push(0); // chFlags

    // CRTR: szCreator[32], chMajorVersion, chMinorVersion
    let mut crtr = vec![0u8; 36];
    crtr[..CREATOR.len()].copy_from_slice(CREATOR);
    push_block(&mut out, b"CRTR", &crtr);

    push_block(&mut out, b"Z80R", &z80r_block(emulator));

    // SPCR: chBorder, ch7ffd, ch1ffd/chEff7, chFe, chReserved[4]
    let border = emulator.controller.border_color as u8;
    #[cfg(feature = "sound")]
    let (ear, mic) = emulator.controller.mixer.beeper.state();
    #[cfg(not(feature = "sound"))]
    let (ear, mic) = (false, false);
    let fe = border | (u8::from(mic) << 3) | (u8::from(ear) << 4);
    let port_7ffd = if is_48k {
        0
    } else {
        emulator.controller.read_7ffd()
    };
    push_block(&mut out, b"SPCR", &[border, port_7ffd, 0, fe, 0, 0, 0, 0]);

    #[cfg(all(feature = "sound", feature = "ay"))]
    if emulator.settings.ay_enabled {
        // AY: chFlags, chCurrentRegister, chAyRegs[16]
        let ay = &emulator.controller.mixer.ay;
        let mut block = vec![
            if is_48k { ZXSTAYF_128AY as u8 } else { 0 },
            ay.current_reg(),
        ];
        block.extend_from_slice(ay.regs());
        push_block(&mut out, b"AY\0\0", &block);
    }

    // KEYB: dwFlags, chKeyboardJoystick
    let joystick = if emulator.controller.kempston.is_some() {
        ZXSTKJT_KEMPSTON as u8
    } else {
        0
    };
    push_block(&mut out, b"KEYB", &[0, 0, 0, 0, joystick]);

    // AMXM: chType, chCtrlA[3], chCtrlB[3]
    let mouse = if emulator.controller.mouse.is_some() {
        ZXSTM_KEMPSTON as u8
    } else {
        0
    };
    push_block(&mut out, b"AMXM", &[mouse, 0, 0, 0, 0, 0, 0]);

    // RAMP: wFlags, chPageNo, the page's 16K
    let pages: &[u8] = if is_48k {
        &[0, 1, 2]
    } else {
        &[0, 1, 2, 3, 4, 5, 6, 7]
    };
    for &page in pages {
        // The 48K's pages are numbered as the 128K's that sit at the same addresses
        let page_no = if is_48k {
            [5, 2, 0][page as usize]
        } else {
            page
        };
        let data = emulator.controller.memory.ram_page_data(page);
        let (flags, data) = compress_page(data);
        let mut block = Vec::with_capacity(3 + data.len());
        block.extend_from_slice(&(flags as u16).to_le_bytes());
        block.push(page_no);
        block.extend_from_slice(&data);
        push_block(&mut out, b"RAMP", &block);
    }

    recorder.write_all(&out)?;
    Ok(())
}

fn push_block(out: &mut Vec<u8>, id: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
}

// ZXSTZ80REGS (Z80R), as `process_z80r_block` reads it
fn z80r_block<H: Host>(emulator: &Emulator<H>) -> Vec<u8> {
    let cpu = &emulator.cpu;
    let mut regs = cpu.regs.clone();
    let mut block = Vec::with_capacity(37);
    for value in [regs.get_af(), regs.get_bc(), regs.get_de(), regs.get_hl()] {
        block.extend_from_slice(&value.to_le_bytes());
    }
    regs.swap_af_alt();
    block.extend_from_slice(&regs.get_af().to_le_bytes());
    regs.swap_af_alt();
    regs.exx();
    for value in [regs.get_bc(), regs.get_de(), regs.get_hl()] {
        block.extend_from_slice(&value.to_le_bytes());
    }
    regs.exx();
    // The loader moves PC past a HALT when the halted flag is set, so it's written one back
    let pc = if cpu.halted {
        regs.get_pc().wrapping_sub(1)
    } else {
        regs.get_pc()
    };
    for value in [regs.get_ix(), regs.get_iy(), regs.get_sp(), pc] {
        block.extend_from_slice(&value.to_le_bytes());
    }
    block.push(regs.get_i());
    block.push(regs.get_r());
    block.push(u8::from(regs.get_iff1()));
    block.push(u8::from(regs.get_iff2()));
    block.push(match cpu.get_im() {
        IntMode::Im0 => 0,
        IntMode::Im1 => 1,
        IntMode::Im2 => 2,
    });
    block.extend_from_slice(&(emulator.controller.frame_clocks as u32).to_le_bytes());
    block.push(emulator.controller.machine.specs().interrupt_length as u8); // chHoldIntReqCycles
    let mut flags = 0;
    if cpu.skip_interrupt {
        flags |= ZXSTZF_EILAST;
    }
    if cpu.halted {
        flags |= ZXSTZF_HALTED;
    }
    if regs.get_q() != 0 {
        flags |= ZXSTZF_FSET;
    }
    block.push(flags as u8);
    block.extend_from_slice(&regs.get_mem_ptr().to_le_bytes());
    block
}

#[cfg(feature = "zlib")]
fn compress_page(data: &[u8]) -> (u32, Vec<u8>) {
    (
        ZXSTRF_COMPRESSED,
        miniz_oxide::deflate::compress_to_vec_zlib(data, 6),
    )
}

#[cfg(not(feature = "zlib"))]
fn compress_page(data: &[u8]) -> (u32, Vec<u8>) {
    (0, data.to_vec())
}
