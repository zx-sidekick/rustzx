# rustzx for ZX Sidekick

This is a fork of [rustzx/rustzx](https://github.com/rustzx/rustzx), kept by the [zx-sidekick](https://github.com/zx-sidekick) organisation for one crate in it: `rustzx-z80`, the Z80 processor.

ZX Sidekick runs original ZX Spectrum games, from the player's own copy, in an emulated machine the player never sees, and adds guidance around them (a map, what is still missing, and so on). It uses `rustzx-z80` as the processor inside its own bus. Nothing else in this repository is used.

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

That matters to ZX Sidekick because it runs games without the Spectrum ROM, which is not its to distribute. The few ROM routines a game uses (for Starquake: the interrupt handler, printing a character, and a multiply) are answered in Rust, by checking the program counter before each instruction. The interrupt handler at `0x0038` could not be answered that way.

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

**Change.** The comments now say what a caller needs: while halted, the program counter stays at the `HALT` and moves past it when an interrupt is taken; `skip_interrupt` is set by `EI`, `DI` and chained `DD`/`FD` prefixes, and holds off interrupts for one step. (Patch 5 narrows it to maskable interrupts, which is what the SZX format's "EI last" flag means.) `tests/integration/interrupt.rs` restores a halted state and checks where the interrupt returns.

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

## Using it

The crate's name and version are unchanged, so it replaces the published crate for a project that depends on `rustzx-z80 = "0.16"`:

```toml
[patch.crates-io]
rustzx-z80 = { git = "https://github.com/zx-sidekick/rustzx", rev = "<commit>" }
```

Pin a commit rather than the branch, so a rebase here cannot change a build. This brings in upstream `master`'s unreleased `rustzx-z80` changes too: the public `halted` and `skip_interrupt` fields, and `Regs::set_q`.

## How it is checked

`.github/workflows/test-rustzx-z80.yml` runs these on every pull request, and on every push to `zx-sidekick` that touches the processor: format, clippy, the unit tests (in a debug build, so overflow checks are on), zexall in two batches, and the z80test suites with the block instruction flags test; and, not required, a build with the minimum Rust version. Clippy runs on a pinned toolchain, moved on purpose, so a new clippy lint can't fail every PR at once. Upstream's workspace workflow (`ci.yml`) is left as it is and only runs for `master` and pull requests into it. `zx-sidekick` requires all five jobs to pass, on a branch up to date with it, before a pull request can be merged.

```
cargo fmt -p rustzx-z80 -- --check
cargo clippy -p rustzx-z80 --all-targets -- -D warnings
cargo test -p rustzx-z80
cargo test --release -p rustzx-z80 -- --include-ignored
cargo test --release -p rustzx-test --test z80test -- --include-ignored
```

On the `zx-sidekick` branch, on 24 September 2026: all of `rustzx-z80`'s tests pass, including all of zexall and the tests of the patches' behaviour, and the three z80test suites pass (`z80memptr` from 1.2a). `tests/integration/emulate.rs` uses only upstream's interface and passes unchanged on `master` too, which shows `emulate()` behaves as upstream's does outside the corrections in patch 5. Against other emulators' test data (24 September 2026, harnesses not in this repository):

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
