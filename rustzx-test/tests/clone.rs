//! A copy of the machine runs exactly as the original would, and apart from it.

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

impl HostContext<CloneHost> for Context {
    fn frame_buffer_context(&self) {}
}

struct CloneHost;

impl Host for CloneHost {
    type Context = Context;
    type TapeAsset = BufferCursor<Vec<u8>>;
    type FrameBuffer = Picture;
    type EmulationStopwatch = InstantStopwatch;
    type IoExtender = StubIoExtender;
    type DebugInterface = StubDebugInterface;
}

/// Runs `n` frames, returning the sound they made.
fn frames(emulator: &mut Emulator<CloneHost>, n: usize) -> Vec<(f32, f32)> {
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

fn ram(emulator: &Emulator<CloneHost>) -> Vec<u8> {
    (0..8)
        .map_while(|bank| emulator.ram_bank(bank))
        .flatten()
        .copied()
        .collect()
}

fn same_run(settings: RustzxSettings) {
    let mut original = Emulator::new(settings, Context).expect("an emulator");
    frames(&mut original, 100);
    let mut copy = original.clone();
    let original_sound = frames(&mut original, 50);
    let copy_sound = frames(&mut copy, 50);
    assert!(original_sound == copy_sound, "the same sound");
    assert!(ram(&original) == ram(&copy), "the same memory");
    assert_eq!(original.cpu().regs.get_pc(), copy.cpu().regs.get_pc());
    assert_eq!(original.cpu().regs.get_sp(), copy.cpu().regs.get_sp());
    assert_eq!(original.screen_buffer(), copy.screen_buffer());
}

#[test]
fn a_48k_copy_runs_as_the_original() {
    same_run(presets::settings_48k());
}

#[test]
fn a_128k_copy_runs_as_the_original() {
    same_run(presets::settings_128k());
}

#[test]
fn a_copy_runs_apart_from_the_original() {
    let mut original =
        Emulator::new(presets::settings_48k_nosound(), Context).expect("an emulator");
    frames(&mut original, 100);
    let before = ram(&original);
    let mut copy = original.clone();
    copy.write(0x9000, !copy.peek(0x9000));
    frames(&mut copy, 10);
    assert!(ram(&original) == before, "the original is untouched");
}
