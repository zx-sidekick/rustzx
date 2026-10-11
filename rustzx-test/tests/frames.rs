//! `emulate_frames` stopped at a breakpoint carries on the same frame when called again.

use rustzx_core::{
    host::{
        BufferCursor, DebugInterface, FrameBuffer, FrameBufferSource, Host, HostContext, Snapshot,
        StubIoExtender,
    },
    zx::video::colors::{ZXBrightness, ZXColor},
    EmulationStopReason, Emulator,
};
use rustzx_test::framework::presets;
use rustzx_utils::stopwatch::InstantStopwatch;
use std::time::Duration;

const LOOP: u16 = 0x8000;
const STACK: u16 = 0x9000;
/// T-states in a 48K frame, over the 12 of a taken `JR`.
const STOPS_PER_FRAME: usize = 69_888 / 12;

struct NoPicture;

impl FrameBuffer for NoPicture {
    type Context = ();
    fn new(_: usize, _: usize, _: FrameBufferSource, (): ()) -> Self {
        Self
    }
    fn set_color(&mut self, _: usize, _: usize, _: ZXColor, _: ZXBrightness) {}
}

struct Context;

impl HostContext<LoopHost> for Context {
    fn frame_buffer_context(&self) {}
}

/// Stops on every arrival at `LOOP`.
struct StopAtLoop;

impl DebugInterface for StopAtLoop {
    fn check_pc_breakpoint(&mut self, addr: u16) -> bool {
        addr == LOOP
    }
}

struct LoopHost;

impl Host for LoopHost {
    type Context = Context;
    type TapeAsset = BufferCursor<Vec<u8>>;
    type FrameBuffer = NoPicture;
    type EmulationStopwatch = InstantStopwatch;
    type IoExtender = StubIoExtender;
    type DebugInterface = StopAtLoop;
}

/// A 48K SNA snapshot running `JR $` at `LOOP` (uncontended, 12 T-states a turn), interrupts off.
fn jr_loop_snapshot() -> Vec<u8> {
    let mut sna = vec![0u8; 27 + 48 * 1024];
    sna[23..25].copy_from_slice(&STACK.to_le_bytes()); // SP; PC is popped from it
    sna[25] = 1; // IM 1; IFF2 (byte 19) stays 0, so no interrupts
    let ram = |addr: u16| 27 + usize::from(addr - 0x4000);
    sna[ram(LOOP)..ram(LOOP) + 2].copy_from_slice(&[0x18, 0xFE]);
    sna[ram(STACK)..ram(STACK) + 2].copy_from_slice(&LOOP.to_le_bytes());
    sna
}

#[test]
fn a_frame_is_not_lost_when_its_last_instruction_hits_a_breakpoint() {
    let mut emulator: Emulator<LoopHost> =
        Emulator::new(presets::settings_48k_nosound(), Context).expect("a 48K emulator");
    emulator
        .load_snapshot(Snapshot::Sna(BufferCursor::new(jr_loop_snapshot())))
        .expect("the snapshot loads");
    emulator.set_debug_interface(StopAtLoop);

    // Every step stops, so the step that ends each frame stops too. Count the stops between
    // frame ends.
    let mut stops = 0;
    let mut frames = Vec::new();
    for _ in 0..STOPS_PER_FRAME * 4 {
        match emulator
            .emulate_frames(Duration::from_secs(1))
            .expect("emulation runs")
            .stop_reason
        {
            EmulationStopReason::Breakpoint => stops += 1,
            EmulationStopReason::Completed => frames.push(std::mem::take(&mut stops)),
            EmulationStopReason::Timeout => panic!("a frame timed out"),
        }
    }

    assert!(frames.len() >= 3, "frames ended: {frames:?}");
    // The first frame began wherever the snapshot left the frame's clock.
    assert!(
        frames[1..].iter().all(|&n| n == STOPS_PER_FRAME),
        "stops between frame ends: {frames:?}"
    );
}
