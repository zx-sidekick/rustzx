# rustzx for ZX Sidekick

This is a fork of [rustzx/rustzx](https://github.com/rustzx/rustzx), kept by the [zx-sidekick](https://github.com/zx-sidekick) organisation for two crates in it: `rustzx-z80`, the Z80 processor, and `rustzx-core`, the Spectrum around it.

[ZX Sidekick](https://github.com/zx-sidekick/zx-sidekick) runs original ZX Spectrum games, from the player's own copy, in an emulated Spectrum, and adds training switches, gamepad controls and guidance around them, as each game's definition describes. Its machine is `rustzx-core` with this fork's `rustzx-z80`, and the Spectrum ROMs `rustzx-core` bundles (Amstrad allows emulators to distribute them). It depends on both crates by git, pinned to a commit of this branch. `aym` comes in through `rustzx-core`; the frontend, `rustzx-utils`, `rustzx-test` and `vtx` aren't used.

The patches below were written for an earlier ZX Sidekick, which used only `rustzx-z80`, inside its own bus and without the ROM; their problem statements describe that. Patches to `rustzx-core` follow, for what ZX Sidekick needs of the machine: a frame that can end at a given address, writing memory and registers, cloning the machine, and saving SZX.

- `master` follows upstream unchanged.
- `zx-sidekick` is `master` plus the patches below. It is the default branch here so this page is what you see first.

## Why carry patches

`rustzx-z80` 0.16.0 is a good processor: it passes zexall and the z80test suites, and inside ZX Sidekick's bus it matches the Fuse test corpus's bus activity in all 1,335 cases. But ZX Sidekick needed three workarounds to use it, each because the crate kept something to itself:

1. A machine could not be copied without `unsafe` code.
2. An interrupt could not be seen before its handler had started to run.
3. The halted and just-after-`EI` state could not be read or restored.

Each fix is small, keeps the existing API and behaviour, and is written so it can be offered upstream. Patches 4 and 5 came later, from a review of the whole crate: 4 keeps behaviour, and 5 changes it on purpose, where the crate did not do what the Z80 does. The patches after them add what ZX Sidekick needed next. Upstream is quiet: the last release is 0.16.0 (31 March 2023), the last change to `rustzx-z80` on `master` was in July 2024, and pull requests have been open since 2023. So the patches are carried here rather than waited for.

## The patches

### 1. `Z80` and `Regs` derive `Clone`

**Problem.** The processor holds only numbers, flags and small enums, but neither `Z80` nor `Regs` implemented `Clone`. ZX Sidekick copies whole machines: to read every room of a game on copies of the machine, and in its checks, to run a ROM routine two ways from the same state and compare.

**Workaround it replaces.** A bitwise copy with `std::ptr::read`, guarded by a compile-time assertion that `Z80` has no drop glue. That catches a future `Box` or `Vec` in the struct, but not every way a bitwise copy could go wrong.

**Change.** `#[derive(Clone)]` on both. Tested in `tests/integration/state.rs`.

### 2. `Z80::step`: an interrupt as a step of its own

**Problem.** `emulate()` takes a due interrupt and then runs one instruction, in the same call. So a caller that looks at the program counter between calls never sees it at the interrupt handler: by the time the call returns, the handler's first instruction has already run.

That mattered to ZX Sidekick because it then ran games without the Spectrum ROM. The few ROM routines a game uses (for Starquake: the interrupt handler, printing a character, and a multiply) are answered in Rust, by checking the program counter before each instruction. The interrupt handler at `0x0038` could not be answered that way.

**Workaround it replaces.**

- `JR $` (a jump to itself) placed at `0x0038`, so the instruction that runs there with the interrupt does nothing harmful.
- The bus noting which address the last opcode was fetched from, so the program counter being at `0x0038` can be told apart from having jumped there.
- The jump's 12 T-states and one R register increment given back afterwards, so the timing matches a machine with the ROM.
- In ZX Sidekick's check against the real ROM: a copy of the whole machine before every instruction in the 32 T-state interrupt window, to have the state from before the interrupt once it turned out one had been taken.

**Change.** A new method, `step()`, does one or the other and says which:

```rust
match cpu.step(&mut bus) {
    Step::Interrupt => { /* program counter at the handler; nothing of it has run */ }
    Step::Instruction => { /* one instruction ran, as before */ }
}
```

Called repeatedly, `step()` runs a program as `emulate()` does, with the same timing. After an interrupt it calls `Z80Bus::pc_callback` with the handler's address, as it does after an instruction, so a breakpoint on the handler is hit before the handler runs.

`emulate()` is unchanged: it now calls the same two halves (`check_interrupt` and `execute_instruction`, both private) that `step()` does.

Tested in `tests/integration/interrupt.rs`: the handler not having run after `Step::Interrupt`, the 13 T-states of an IM 1 interrupt, the instruction after `EI`, leaving a `HALT`, and a program with `EI`, `HALT` and a steady interrupt run both ways, compared on registers, stack and clocks.

### 3. What `halted` and `skip_interrupt` mean

**Problem.** Both fields were `pub(crate)` in 0.16.0, so a caller could not put the processor into a halted state (a snapshot taken on a `HALT`, a test starting there) and had to work out for itself where an interrupt taken on a `HALT` returns to.

**Already upstream.** Upstream `master` made both fields public (for SZX snapshot loading) after 0.16.0; it has not been released. Their comments described them from inside the crate.

**Change.** The comments now say what a caller needs: while halted, the program counter stays at the `HALT` and moves past it when an interrupt is taken; `skip_interrupt` is set by `EI`, `DI` and chained `DD`/`FD` prefixes, and holds off interrupts for one step. (Patch 5 narrows it to maskable interrupts, which is what the SZX format's "EI last" flag means, and also sets it after a `RETI`/`RETN` that changes IFF1. `Z80`'s doc comment lists the state between steps that the public fields don't carry: a pending prefix, the NMI latch, and `LD A,I`/`LD A,R` just run.) `tests/integration/interrupt.rs` restores a halted state and checks where the interrupt returns.

### 4. No `unsafe`, and pedantic clippy

**Change.** `rustzx-z80/Cargo.toml` forbids `unsafe` code (there was none) and denies clippy's pedantic lints, so `cargo clippy -p rustzx-z80 --all-targets` fails on any of them (CI adds `-D warnings` for the rest). It also declares `rust-version = "1.87"`, which CI checks. The code was brought in line without changing behaviour: explicit `u16::from` widening, `cast_signed()` for displacement bytes, `wrapping_add_signed` for relative addresses, `#[must_use]` on getters, reasons on the ignored zexall tests. Two lints are relaxed, each with its reason written next to it: truncating casts (a Z80 takes the low byte of wider results everywhere), and the length of the two opcode dispatch functions (one match arm per opcode group).

### 5. Corrections

**Problem.** A review of the whole crate found places where it does not do what the Z80 does. None affects a game ZX Sidekick runs today, but each is behaviour a program or snapshot can depend on.

**Change.** Each is fixed and tested in `tests/integration/corrections.rs`, against the hardware rather than against upstream:

- `Regs::get_h_alt` and `get_l_alt` returned `H` and `L`, not `H'` and `L'`, so `rustzx-core` saved SNA snapshots with the wrong `HL'`.
- MEMPTR after `LD (nn),A` kept the high byte of `nn + 1`, and after `OUT (n),A` carried `n + 1` into the high byte when `n` was `0xFF`. Both are now `A` in the high byte and the low byte of the address plus one, as documented in "MEMPTR, esoteric register of the Zilog Z80 CPU". It shows in `F3`/`F5` after a following `BIT n,(HL)`.
- NMI is edge-triggered, as on the Z80: the line going active latches one NMI, however long it then stays active. Upstream took an NMI on every instruction while the line was active, so a bus had to release it at exactly the right moment.
- `EI` and `DI` hold off only the maskable interrupt; an NMI is taken straight after them. Only an unfinished chain of `DD`/`FD` prefixes holds off both, and an NMI that arrives during one is kept until the chain has its instruction. Upstream held the NMI off after `EI` and `DI` too.
- A second NMI is not taken straight after an NMI response: at least one instruction of the handler runs first (found in 2022 by Manuel Sainz de Baranda y Goñi). On the chip, an NMI edge during the response itself is lost; here it waits for that instruction, since `emulate()` cannot tell the two apart and `step()` has to run a program as `emulate()` does.
- `RETI` and `RETN` copy IFF2 into IFF1 during the next opcode fetch, so when that changes IFF1 (only after an NMI) a maskable interrupt is not taken straight after them (found by Andre Weissflog in 2021).
- After `LD A,I` or `LD A,R`, a maskable interrupt accepted straight away leaves P/V at 0, as on an NMOS Z80 (Zilog, *Z80 Family Data Book*, 1989).
- A `DD` or `FD` prefix that doesn't apply to the next opcode is an instruction of its own that leaves the flags alone, so it clears Q: a prefixed `SCF` or `CCF` takes flags 3 and 5 from `F | A`, not `(Q ^ F) | A` (as in redcode/Z80 and SingleStepTests).
- A repeating `INIR`, `INDR`, `OTIR` or `OTDR` sets MEMPTR to the instruction's address + 1, as `LDIR` does (found by rofl0r in 2022 and Manuel Sainz de Baranda y Goñi in 2023). z80test 1.2a corrected its `INIR->NOP'`/`INDR->NOP'` MEMPTR checksums for this; `rustzx-test` now bundles 1.2a's `z80memptr`, which ZX Sidekick's `rustzx-z80` passes and upstream's fails.
- `CodegenMemorySpace::write_word` wrote both bytes to the same address; `CodeGenerator` panicked in a debug build when it wrote past `0xFFFF`; and code it writes into a bus is stored with `write_internal`, so it no longer waits (or is contended) as if the processor had written it.

`emulate.rs` still passes unchanged on upstream `master`: none of these is on a path it pins. Its long run's NMI handler re-enables interrupts with `EI` before `RETN`, so that `RETN` leaves IFF1 alone and the run stays on behaviour both share.

The corrections were checked against the MEMPTR document (Boo-boo, trans. Vladimir Kladov), and against [redcode/Z80](https://github.com/redcode/Z80), whose interrupt handling cites Zilog's documentation and checks with Visual Z80 Remix; the `LD A,I` bug is in Zilog's *Z80 Family Data Book* (1989), pp. 412-413.

### 6. `alu::add16_flags`: the 16-bit add's flags, public

**Problem.** ZX Sidekick answers the ROM's multiply (`HL = HL * DE`) itself, and has to leave F exactly as the ROM's `ADD HL,HL` and `ADD HL,DE` would. It kept a hand-written copy of `ADD HL,ss`'s flag rules, which only a test on its side kept in step with this crate's.

**Change.** A public `alu` module with `add16_flags(flags, a, b) -> (sum, flags)`, the arithmetic of `ADD HL,ss`, `ADD IX,ss` and `ADD IY,ss`. The instruction calls it, so the two cannot drift apart. MEMPTR, which the instruction also sets, is left to the caller. Tested in `tests/integration/alu.rs`: values worked out by hand from the flag rules, and agreement with the instruction for every prefix, source register and starting F.
### 7. Faster register access in the block instructions

**Problem.** The block instructions (`LDI`/`LDIR`, `CPI`/`CPIR`, `INI`/`INIR`, `OUTI`/`OTIR` and their decrementing forms) and `DJNZ` name fixed registers, but reached them through `RegName16`/`RegName8`, decoding the name on every byte moved. A profile of ZX Sidekick's Manic Miner, which copies about 8.5 KB a pass with `LDIR`, had those accessors at about 7% of a frame.

**Change.** Direct crate-internal accessors for HL, DE, BC and B, used where those registers are named, and `#[inline]` on the generic accessors. No behaviour change: the tests, zexall, z80test and SingleStepTests give the same results. `examples/speed.rs` measures it; with code alignment held fixed, a block copy runs about 2× as fast and a mix of other instructions about 2% faster.

### 9. `Z80::run_until`: the step loop, with breakpoints and a limit

**Problem.** ZX Sidekick runs a frame by calling `step` in a loop, checking the program counter against the addresses it acts at, and stopping at the frame's end.

**Change.** `Z80::run_until(bus, &breakpoints, limit) -> Stop` runs that loop: before each step it asks `limit` (a closure over the bus, since only the bus knows how many T-states have passed), and it stops when an instruction leaves the program counter on an address in `breakpoints` (a `Breakpoints` set, a bit for each of the 65,536 addresses) or when an interrupt is taken. `Stop` says which. Tested in `tests/integration/run_until.rs`, including stop-for-stop agreement with the same loop written around `step`. It's no faster than that loop (`step` is generic over the bus, so it's already compiled into the caller); what it adds is saying the loop once.

### 10. `Z80::push`, `pop` and `ret`: stack operations for callers

**Problem.** ZX Sidekick answers ROM routines itself and steers games between instructions, which means pushing, popping and returning from outside the processor. It kept its own copies of those stack operations.

**Change.** `Z80::push(bus, value)`, `Z80::pop(bus)` and `Z80::ret(bus)`. They use `read_internal`/`write_internal`, so no time passes and nothing is contended. `ret` leaves the processor as `RET` does (PC, SP, MEMPTR = PC, Q = 0 for a following `SCF`/`CCF`, the end of the moment after `LD A,I`, and a `pc_callback` to the bus), except what fetching an opcode does (R and time). The existing `push_pc_to_stack`/`pop_pc_from_stack`, which `rustzx-core` uses for SNA snapshots and tape fast-loading and which go through the contended bus, are unchanged. Tested in `tests/integration/stack.rs`.

### 11. `emulate_frames` carries a frame over a breakpoint stop

**Problem.** ZX Sidekick stops the machine at the top of a game's main loop, through `rustzx-core`'s `DebugInterface`, and then carries on to the end of the frame. `emulate_frames` resets its frame counter at the start of every call, so when the instruction that hits a breakpoint is also the one that ends a frame, that frame's end is lost: the next call runs a whole extra frame before it returns `Completed`. With a breakpoint that stops every step, `Completed` never comes at all.

**Change.** A call that stops at a breakpoint leaves a note, and the next call carries on that frame instead of starting a new count: if the frame already ended on the instruction that hit the breakpoint, it returns `Completed` straight away, having run nothing. Loading a snapshot clears the note. Tested in `rustzx-test/tests/frames.rs`: a one-instruction loop (`JR $`, 12 T-states, interrupts off) with a breakpoint on it stops every step, and `Completed` comes exactly every 69,888 / 12 = 5,824 stops. The unit-test job runs it.

### 12. `rustzx-core` steps the processor with `Z80::step`

**Problem.** `emulate_frames` ran the processor with `emulate()`, which takes a due interrupt and runs the handler's first instruction in the same call (patch 2). So a breakpoint on an interrupt handler's first address was never hit: `rustzx-core` never saw the program counter there. ZX Sidekick reports which addresses a frame ran, for its definitions' states, and stops at a game's main loop, which may be an interrupt handler.

**Change.** `emulate_frames` calls `step()` instead, which takes an interrupt as a step of its own and then reports the handler's address to the bus, with `emulate()`'s timing. `EmulationStopReason` derives `Debug`. Tested in `rustzx-test/tests/frames.rs`: a breakpoint on the 48K ROM's handler at `0x0038` stops once a frame, before the handler runs. Every `rustzx-test` suite (the screens and sound compared by hash, the tape, keyboard and joystick tests, z80test) passes unchanged, and the unit-test job now runs them all.

### 13. Writing memory, the processor and RAM banks from outside

**Problem.** ZX Sidekick writes the emulated machine between frames (training switches, crafted situations, saves) and reads its registers and the 128K's banks. `rustzx-core` offered `peek` and `execute_poke`, which writes the ROM too and leaves the picture stale when it writes display memory, and kept the processor private.

**Change.** `Emulator::write(addr, value)` writes as the processor does, through the controller's `write_internal`: the ROM is left alone and display memory reaches the picture, with no time passing. `Emulator::cpu()` and `cpu_mut()` lend the `Z80`, whose registers, `halted`, `skip_interrupt` and interrupt mode the earlier patches made public. `Emulator::ram_bank(bank)` reads a 16K RAM bank whether paged in or not, `None` past the last. Tested in `rustzx-test/tests/access.rs` (the picture test fails if a write skips the screen).

### 14. `Emulator` is `Clone`

**Problem.** ZX Sidekick keeps whole machines in memory (quick states, reading facts on a copy without disturbing play), and patch 1 made only the processor cloneable.

**Change.** `Emulator<H>` and the controller implement `Clone` when the host's frame buffer, tape asset, IO extender and debug interface do; everything under them (memory, screen, border, tape, joystick, mouse, mixer, beeper, the AY chip in `aym`, `BufferCursor`, the stub extender and debug interface) derives it. The copy is the whole machine, and runs apart from the original. The controller's pending emulation error isn't copied: it's set and taken within one step, so it's always empty between calls. Tested in `rustzx-test/tests/clone.rs`: a 48K and a 128K copied after start-up run 50 more frames exactly as the original (memory, registers, picture and every sound sample), and a change to a copy leaves the original alone.

### 15. SZX snapshots are saved, and load as saved

**Problem.** `rustzx-core` loaded SZX but saving returned `NotSupported`, and a 48K SNA save pushes the program counter onto the program's own stack, so ZX Sidekick couldn't save a game without touching its memory. Writing the saver showed three faults in the loader: it read the creator block one byte past the 36 the format gives it (a panic on any snapshot with a spec-sized `CRTR`); it set port `0xFE` through the bus after setting the frame's T-state count, so a loaded machine ran 4 T-states late; and it put the AY's registers in the chip's copy without writing them to the sound chip.

**Change.** `szx::save` writes `CRTR`, `Z80R`, `SPCR`, `AY` (when the AY is on), `KEYB`, `AMXM`, and a `RAMP` block for each RAM page, zlib-compressed with the `zlib` feature: the inverse of the loader, so what it saves loads back as it was. A halted processor's PC is written one back, since the loader moves it past the `HALT`. The loader no longer reads the creator block (it only names the program that wrote it), sets the beeper and border directly, and `ZXAyChip::set_regs` writes each register to the chip. `Regs::get_q` reads Q, for the `FSET` flag. The tape and its position aren't saved. Tested in `rustzx-test/tests/szx_save.rs`: a 48K, a 128K with bank 3 paged in and the AY sounding, and a halted machine each load back with the same memory, registers and interrupt mode, then run 50 frames as the original (the same picture, and sound but for the AY, whose tone and envelope position SZX doesn't hold), and save again to the same bytes.

### 16. A 320×256 border as a setting

**Problem.** The precise border's frame buffer was 320×240 (24 lines above and below the screen). ZX Sidekick shows the Spectrum's 320×256, 32 lines above and below, as its window design does.

**Change.** `RustzxSettings::border_size` takes a `zx::video::BorderSize`: `Standard` (320×240, the default, as before) or `Tall` (320×256). The border's beam timing starts 8 × the size's border rows before the first screen line, so a tall border is the standard one with eight more lines each side. `BorderSize::canvas_y` says where the screen sits in the border buffer. Tested in `rustzx-test/tests/access.rs`: the buffer sizes, a red border reaching a tall border's last line, and, with the border changing every 27 T-states, a tall border's lines 8 to 247 matching a standard border's 0 to 239 pixel for pixel. The tape test's border hashes are unchanged.

### 17. The core's dependencies at their latest releases

**Problem.** `rustzx-core` depended on old releases (`bitflags` 1.3, `strum` 0.22, `from_variants` 0.6, `miniz_oxide` 0.7), which brought three versions of `syn` into ZX Sidekick's build; ZX Sidekick takes the latest version of each dependency.

**Change.** `bitflags` 2.13, `strum` 0.28, `miniz_oxide` 0.9, `lazy_static` 1.5, `enum_dispatch` 0.3.13, and `aym`'s `num-traits` 0.2.19 and `libm` 0.2.16. `EmulationEvents` derives what bitflags 2 no longer gives it, and `take` is `mem::take`. `from_variants` (whose latest still used `syn` 1) and `displaydoc` (`syn` 3) are gone: the error types' `Display` and `From` impls are written out, with each message as before (a wrapping `Error` now also shows its cause), and every error type implements `core::error::Error`, so hosts can use them as a source. One `syn` remains. Every `rustzx-test` suite passes unchanged.

## Using it

ZX Sidekick depends on the crates directly, both from the same commit, so `rustzx-core` is built on this `rustzx-z80`:

```toml
[workspace.dependencies]
rustzx-core = { git = "https://github.com/zx-sidekick/rustzx", rev = "<commit>", features = ["embedded-roms", "ay", "zlib"] }
rustzx-z80 = { git = "https://github.com/zx-sidekick/rustzx", rev = "<commit>" }
```

The crates' names and versions are unchanged, so for a project that depends on the published `rustzx-z80 = "0.16"`, this fork's replaces it:

```toml
[patch.crates-io]
rustzx-z80 = { git = "https://github.com/zx-sidekick/rustzx", rev = "<commit>" }
```

Pin a commit rather than the branch, so a rebase here cannot change a build. This brings in upstream `master`'s unreleased `rustzx-z80` changes too: the public `halted` and `skip_interrupt` fields, and `Regs::set_q`.

## How it is checked

`.github/workflows/test-rustzx-z80.yml` runs these on every pull request, and on every push to `zx-sidekick` that touches the processor or the core: format, clippy, the unit tests (in a debug build, so overflow checks are on) with `rustzx-core`'s tests, zexall in two batches, and the z80test suites with the block instruction flags test; and, not required, a build with the minimum Rust version. Clippy runs on a pinned toolchain, moved on purpose, so a new clippy lint can't fail every PR at once. Upstream's workspace workflow (`ci.yml`) is left as it is and only runs for `master` and pull requests into it. `zx-sidekick` requires all five jobs to pass, on a branch up to date with it, before a pull request can be merged.

```
cargo fmt -p rustzx-z80 -- --check
cargo clippy -p rustzx-z80 --all-targets -- -D warnings
cargo test -p rustzx-z80
cargo test -p rustzx-test
cargo test --release -p rustzx-z80 -- --include-ignored
cargo test --release -p rustzx-test --test z80test -- --include-ignored
```

On the `zx-sidekick` branch, on 24 September 2026: all of `rustzx-z80`'s tests pass, including all of zexall and the tests of the patches' behaviour, and the three z80test suites pass (`z80memptr` from 1.2a). `tests/integration/emulate.rs` uses only upstream's interface and passes unchanged on `master` too, which shows `emulate()` behaves as upstream's does outside the corrections in patch 5. To re-check that: in a worktree of `master`, copy in `tests/integration/main.rs` and `emulate.rs`, delete `main.rs`'s `mod` lines for the other test files, and run `cargo test -p rustzx-z80 --test integration emulate`. Against other emulators' test data (24 September 2026, harnesses not in this repository):

- Fuse's own core tests (`z80/tests`, 1,356 cases, registers, memory, T-states and every timed memory access): 1,346 match exactly, up from 1,345 on upstream (these patches fix the two MEMPTR cases Fuse checks, `32` and `d3_4`). Of the other 10, 5 are Fuse's log leaving out the displacement read of a `JR cc`/`DJNZ` that doesn't jump (the Z80 does read it), and 5 are repeating block instructions, whose flags and MEMPTR Fuse doesn't emulate as later research found them.
- SingleStepTests/z80 (1,604,000 cases, generated from the Ares core): 1,594,038 match. The rest are `HALT`, where they have PC move past it while this crate keeps it on the `HALT` (as Fuse and floooh's chips do; the address an interrupt pushes is the same), and their `ei` field, which means "last instruction was `EI`" where `skip_interrupt` is also set by `DI` and by the `RETN` hold-off.

Before those corrections, ZX Sidekick's Fuse corpus harness gave the same result on this crate as on 0.16.0 (it has not been run since): 1,329 of 1,335 cases match exactly (the other 6 differ only in the undocumented bits 3 and 5 of F), and bus activity matches in all 1,335. Those 6 are not differences with Fuse today: the harness runs an older copy of Fuse's tests (1,335 cases, with no MEMPTR column), from before Fuse modelled Q and MEMPTR there, and the current tests (1,356 cases) expect what this crate does. `tests/integration/fuse_flag_cases.rs` pins the 6 cases with the current tests' values; updating ZX Sidekick's copy of the tests should make its harness match on them too.

## Keeping up with upstream

```
git fetch upstream
git switch zx-sidekick
git rebase upstream/master
```

A patch upstream has taken is dropped in the rebase. If upstream releases every patch here, ZX Sidekick goes back to the published crate and this repository has no reason to exist.

## Licence

MIT, as upstream: `LICENSE.md` is unchanged and the copyright in it stays with its holder. The patches here are under the same licence.
