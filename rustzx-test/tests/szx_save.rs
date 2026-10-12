//! A machine saved as SZX loads back as it was, and runs on exactly as the original.

use rustzx_core::{
    host::{
        BufferCursor, DataRecorder, FrameBuffer, FrameBufferSource, Host, HostContext, Snapshot,
        SnapshotRecorder, StubDebugInterface, StubIoExtender,
    },
    zx::video::colors::{ZXBrightness, ZXColor},
    Emulator, RustzxSettings,
};
use rustzx_test::framework::presets;
use rustzx_utils::stopwatch::InstantStopwatch;
use rustzx_z80::IntMode;
use std::time::Duration;

#[derive(Clone, PartialEq, Eq, Debug)]
struct Picture {
    width: usize,
    pixels: Vec<u8>,
}

impl FrameBuffer for Picture {
    type Context = ();
    fn new(width: usize, height: usize, _: FrameBufferSource, (): ()) -> Self {
        Self {
            width,
            pixels: vec![0; width * height],
        }
    }
    fn set_color(&mut self, x: usize, y: usize, color: ZXColor, brightness: ZXBrightness) {
        self.pixels[x + y * self.width] = color as u8 + brightness as u8 * 8;
    }
}

struct Context;

impl HostContext<SaveHost> for Context {
    fn frame_buffer_context(&self) {}
}

struct SaveHost;

impl Host for SaveHost {
    type Context = Context;
    type TapeAsset = BufferCursor<Vec<u8>>;
    type FrameBuffer = Picture;
    type EmulationStopwatch = InstantStopwatch;
    type IoExtender = StubIoExtender;
    type DebugInterface = StubDebugInterface;
}

struct Bytes<'a>(&'a mut Vec<u8>);

impl DataRecorder for Bytes<'_> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, rustzx_core::error::IoError> {
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }
}

/// Runs `n` frames, returning the sound they made.
fn frames(emulator: &mut Emulator<SaveHost>, n: usize) -> Vec<(f32, f32)> {
    let mut sound = Vec::new();
    for _ in 0..n {
        emulator
            .emulate_frames(Duration::from_secs(1))
            .expect("emulation runs");
        while let Some(sample) = emulator.next_audio_sample() {
            sound.push((sample.left, sample.right));
        }
    }
    sound
}

fn ram(emulator: &Emulator<SaveHost>) -> Vec<u8> {
    (0..8)
        .map_while(|bank| emulator.ram_bank(bank))
        .flatten()
        .copied()
        .collect()
}

fn registers(emulator: &Emulator<SaveHost>) -> Vec<u16> {
    let cpu = emulator.cpu();
    let mut regs = cpu.regs.clone();
    let mut all = vec![
        regs.get_af(),
        regs.get_bc(),
        regs.get_de(),
        regs.get_hl(),
        regs.get_ix(),
        regs.get_iy(),
        regs.get_sp(),
        regs.get_pc(),
        regs.get_ir(),
        regs.get_mem_ptr(),
        u16::from(regs.get_iff1()),
        u16::from(regs.get_iff2()),
        u16::from(regs.get_q()),
        u16::from(cpu.halted),
        u16::from(cpu.skip_interrupt),
    ];
    regs.swap_af_alt();
    regs.exx();
    all.extend([regs.get_af(), regs.get_bc(), regs.get_de(), regs.get_hl()]);
    all
}

fn im(emulator: &Emulator<SaveHost>) -> u8 {
    match emulator.cpu().get_im() {
        IntMode::Im0 => 0,
        IntMode::Im1 => 1,
        IntMode::Im2 => 2,
    }
}

fn saved(emulator: &mut Emulator<SaveHost>) -> Vec<u8> {
    let mut szx = Vec::new();
    emulator
        .save_snapshot(SnapshotRecorder::Szx(Bytes(&mut szx)))
        .expect("the snapshot saves");
    szx
}

/// Saves, loads into a fresh machine, and compares the two now and 50 frames on. `same_sound`
/// is false with the AY on: SZX holds its registers, not its tone counters or envelope position,
/// so its sound starts afresh from the registers.
fn round_trip(settings: RustzxSettings, mut original: Emulator<SaveHost>, same_sound: bool) {
    let szx = saved(&mut original);
    let mut loaded = Emulator::new(settings, Context).expect("an emulator");
    loaded
        .load_snapshot(Snapshot::Szx(BufferCursor::new(szx)))
        .expect("the snapshot loads");

    assert!(ram(&loaded) == ram(&original), "the same memory");
    assert_eq!(registers(&loaded), registers(&original));
    assert_eq!(im(&loaded), im(&original));

    let original_sound = frames(&mut original, 50);
    let loaded_sound = frames(&mut loaded, 50);
    assert!(
        ram(&loaded) == ram(&original),
        "the same memory 50 frames on"
    );
    assert_eq!(registers(&loaded), registers(&original));
    assert_eq!(loaded.screen_buffer(), original.screen_buffer());
    if same_sound {
        assert!(loaded_sound == original_sound, "the same sound");
    }
    assert!(
        saved(&mut loaded) == saved(&mut original),
        "saved again, the same snapshot"
    );
}

fn booted(settings: RustzxSettings) -> Emulator<SaveHost> {
    let mut emulator = Emulator::new(settings, Context).expect("an emulator");
    frames(&mut emulator, 100);
    emulator
}

#[test]
fn a_48k_saved_loads_back_as_it_was() {
    let mut settings = presets::settings_48k();
    settings.ay_enabled = false;
    round_trip(settings, booted(settings), true);
}

#[test]
fn a_128k_saved_loads_back_as_it_was() {
    let settings = presets::settings_128k();
    let mut emulator = booted(settings);
    // Page bank 3 in, and give the AY a tone, so paging and the AY are in the snapshot.
    emulator.cpu_mut().regs.set_pc(0x9000);
    for (addr, byte) in (0x9000..).zip([
        0x01, 0xFD, 0x7F, 0x3E, 0x13, 0xED, 0x79, // LD BC,0x7FFD; LD A,0x13; OUT (C),A
        0x01, 0xFD, 0xFF, 0x3E, 0x00, 0xED, 0x79, // LD BC,0xFFFD; LD A,0; OUT (C),A
        0x01, 0xFD, 0xBF, 0x3E, 0x40, 0xED, 0x79, // LD BC,0xBFFD; LD A,0x40; OUT (C),A
        0x01, 0xFD, 0xFF, 0x3E, 0x08, 0xED, 0x79, // LD BC,0xFFFD; LD A,8; OUT (C),A
        0x01, 0xFD, 0xBF, 0x3E, 0x0F, 0xED, 0x79, // LD BC,0xBFFD; LD A,15; OUT (C),A
        0x01, 0xFD, 0xFF, 0x3E, 0x07, 0xED, 0x79, // LD BC,0xFFFD; LD A,7; OUT (C),A
        0x01, 0xFD, 0xBF, 0x3E, 0x3E, 0xED, 0x79, // LD BC,0xBFFD; LD A,0x3E; OUT (C),A
        0x18, 0xFE, // JR $
    ]) {
        emulator.write(addr, byte);
    }
    frames(&mut emulator, 2);
    assert_eq!(emulator.cpu().regs.get_pc(), 0x9000 + 49, "the program ran");
    round_trip(settings, emulator, false);
}

#[test]
fn a_halted_machine_saved_loads_back_halted() {
    let settings = presets::settings_48k_nosound();
    let mut emulator = booted(settings);
    // HALT; JR -3: halted from just after each interrupt until the next.
    for (addr, byte) in (0x9000..).zip([0x76, 0x18, 0xFD]) {
        emulator.write(addr, byte);
    }
    emulator.cpu_mut().regs.set_pc(0x9000);
    frames(&mut emulator, 2);
    assert!(emulator.cpu().halted, "halted at the frame's end");
    round_trip(settings, emulator, true);
}

/// A block as a snapshot holds it: its id, its size and its bytes.
fn block(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let size = u32::try_from(data.len()).expect("a small block");
    [&id[..], &size.to_le_bytes(), data].concat()
}

/// Where each block of a snapshot starts, and its id.
fn blocks(szx: &[u8]) -> Vec<(usize, [u8; 4])> {
    let mut at = 8;
    let mut found = Vec::new();
    while at + 8 <= szx.len() {
        let id = [szx[at], szx[at + 1], szx[at + 2], szx[at + 3]];
        let size = u32::from_le_bytes([szx[at + 4], szx[at + 5], szx[at + 6], szx[at + 7]]);
        found.push((at, id));
        at += 8 + size as usize;
    }
    found
}

#[test]
fn a_damaged_snapshot_is_an_error_and_changes_nothing() {
    let mut settings = presets::settings_48k();
    settings.ay_enabled = false;
    let szx = saved(&mut booted(settings));
    let header = &szx[..8];
    let starts = blocks(&szx);
    let z80r = starts
        .iter()
        .find(|(_, id)| id == b"Z80R")
        .expect("a Z80R")
        .0;

    let mut bad_id = szx.clone();
    bad_id[z80r..z80r + 4].copy_from_slice(&[0xFF, 0xFE, 0xFD, 0xFC]);
    let mut huge = szx.clone();
    huge[z80r + 4..z80r + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut short_page = vec![0, 0, 0];
    short_page.extend_from_slice(&[0x55; 100]);
    let damaged: Vec<(&str, Vec<u8>)> = vec![
        ("cut in the header", szx[..5].to_vec()),
        ("cut in a block's header", szx[..z80r + 3].to_vec()),
        ("cut in a block", szx[..szx.len() - 100].to_vec()),
        ("an id that isn't text", bad_id),
        ("a size past the end", huge),
        (
            "a Z80R too short",
            [header, &block(b"Z80R", &[0; 10])].concat(),
        ),
        (
            "a RAMP too short",
            [header, &block(b"RAMP", &[0, 0])].concat(),
        ),
        (
            "a page a 48K doesn't have",
            [header, &block(b"RAMP", &[0, 0, 9, 0])].concat(),
        ),
        (
            "less than a page of data",
            [header, &block(b"RAMP", &short_page)].concat(),
        ),
    ];

    for (what, bytes) in damaged {
        let mut machine = booted(settings);
        let (memory, regs) = (ram(&machine), registers(&machine));
        let loaded = machine.load_snapshot(Snapshot::Szx(BufferCursor::new(bytes)));
        assert!(loaded.is_err(), "{what}: refused");
        assert!(ram(&machine) == memory, "{what}: the same memory");
        assert_eq!(registers(&machine), regs, "{what}: the same registers");
    }
}
