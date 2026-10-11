//! Writing memory, the processor and RAM banks from between frames.

use rustzx_core::{
    host::{
        BufferCursor, FrameBuffer, FrameBufferSource, Host, HostContext, StubDebugInterface,
        StubIoExtender,
    },
    zx::video::colors::{ZXBrightness, ZXColor},
    Emulator, RustzxSettings,
};
use rustzx_test::framework::presets;
use rustzx_utils::stopwatch::InstantStopwatch;
use std::time::Duration;

/// The picture, one byte a pixel: its color plus 8 when bright.
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

impl HostContext<AccessHost> for Context {
    fn frame_buffer_context(&self) {}
}

struct AccessHost;

impl Host for AccessHost {
    type Context = Context;
    type TapeAsset = BufferCursor<Vec<u8>>;
    type FrameBuffer = Picture;
    type EmulationStopwatch = InstantStopwatch;
    type IoExtender = StubIoExtender;
    type DebugInterface = StubDebugInterface;
}

fn frames(emulator: &mut Emulator<AccessHost>, n: usize) {
    for _ in 0..n {
        emulator
            .emulate_frames(Duration::from_secs(1))
            .expect("emulation runs");
    }
}

/// A machine whose ROM has started up.
fn booted(settings: RustzxSettings) -> Emulator<AccessHost> {
    let mut emulator = Emulator::new(settings, Context).expect("an emulator");
    frames(&mut emulator, 100);
    emulator
}

#[test]
fn a_write_to_the_rom_is_ignored() {
    let mut emulator = booted(presets::settings_48k_nosound());
    let before = emulator.peek(0x0000);
    emulator.write(0x0000, !before);
    assert_eq!(emulator.peek(0x0000), before);
}

#[test]
fn a_write_to_ram_is_read_back() {
    let mut emulator = booted(presets::settings_48k_nosound());
    emulator.write(0x9000, 0xA5);
    assert_eq!(emulator.peek(0x9000), 0xA5);
}

#[test]
fn a_write_to_display_memory_reaches_the_picture() {
    let mut emulator = booted(presets::settings_48k_nosound());
    frames(&mut emulator, 1);
    let before = emulator.screen_buffer().pixels.clone();
    // The first byte of display memory: eight pixels at the top left of the screen, all ink.
    emulator.write(0x4000, 0xFF);
    frames(&mut emulator, 1);
    assert_ne!(emulator.screen_buffer().pixels, before);
}

#[test]
fn registers_set_between_frames_are_what_the_processor_runs_with() {
    const LOOP: u16 = 0x9000;
    let mut emulator = booted(presets::settings_48k_nosound());
    emulator.write(LOOP, 0x18); // JR $
    emulator.write(LOOP + 1, 0xFE);
    emulator.cpu_mut().regs.set_pc(LOOP);
    frames(&mut emulator, 3);
    // The interrupt handler runs at the start of each frame and returns to the loop.
    assert_eq!(emulator.cpu().regs.get_pc(), LOOP);
}

#[test]
fn a_128k_bank_reads_the_same_paged_in_or_not() {
    let mut emulator = booted(presets::settings_128k_nosound());
    // Bank 5 is always paged in at 0x4000.
    emulator.write(0x4000, 0x5A);
    assert_eq!(emulator.ram_bank(5).expect("bank 5")[0], 0x5A);
    assert_eq!(emulator.ram_bank(7).expect("bank 7").len(), 16 * 1024);
    assert!(emulator.ram_bank(8).is_none());
}

#[test]
fn a_48k_has_three_banks_from_0x4000() {
    let mut emulator = booted(presets::settings_48k_nosound());
    emulator.write(0x8000, 0x3C);
    assert_eq!(emulator.ram_bank(1).expect("bank 1")[0], 0x3C);
    assert!(emulator.ram_bank(3).is_none());
}

mod border {
    use super::*;
    use rustzx_core::zx::video::BorderSize;

    fn border_settings(size: BorderSize) -> RustzxSettings {
        RustzxSettings {
            border_size: size,
            ..presets::settings_48k_nosound()
        }
    }

    #[test]
    fn the_standard_border_is_320_by_240() {
        let emulator = booted(border_settings(BorderSize::Standard));
        assert_eq!(emulator.border_buffer().pixels.len(), 320 * 240);
    }

    #[test]
    fn a_tall_border_is_320_by_256_and_drawn_to_its_last_line() {
        let mut emulator = booted(border_settings(BorderSize::Tall));
        assert_eq!(emulator.border_buffer().pixels.len(), 320 * 256);
        assert_eq!(BorderSize::Tall.canvas_y(), 32);
        // LD A,2; OUT (254),A; JR $: a red border from here on
        for (addr, byte) in (0x9000..).zip([0x3E, 0x02, 0xD3, 0xFE, 0x18, 0xFE]) {
            emulator.write(addr, byte);
        }
        emulator.cpu_mut().regs.set_pc(0x9000);
        frames(&mut emulator, 2);
        let red = ZXColor::Red as u8;
        assert!(
            emulator.border_buffer().pixels.iter().all(|&p| p == red),
            "every border pixel red, the first and last 32 lines included"
        );
    }

    #[test]
    fn a_tall_border_is_the_standard_one_with_eight_more_lines_each_side() {
        // DI; loop: OUT (254),A; INC A; JR loop: the border changes every 27 T-states.
        let program = [0xF3, 0xD3, 0xFE, 0x3C, 0x18, 0xFB];
        let mut borders = [BorderSize::Standard, BorderSize::Tall].map(|size| {
            let mut emulator = booted(border_settings(size));
            for (addr, &byte) in (0x9000..).zip(&program) {
                emulator.write(addr, byte);
            }
            emulator.cpu_mut().regs.set_pc(0x9000);
            frames(&mut emulator, 3);
            emulator.border_buffer().pixels.clone()
        });
        let tall = borders[1].split_off(8 * 320);
        let standard = &borders[0];
        assert!(
            tall[..240 * 320] == standard[..],
            "the tall border's lines 8 to 247 are the standard border's 0 to 239"
        );
    }
}
