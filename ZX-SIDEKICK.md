# rustzx for ZX Sidekick

This is a fork of [rustzx/rustzx](https://github.com/rustzx/rustzx), kept by the [zx-sidekick](https://github.com/zx-sidekick) organisation for one crate in it: `rustzx-z80`, the Z80 processor.

ZX Sidekick runs original ZX Spectrum games, from the player's own copy, in an emulated machine the player never sees, and adds guidance around them (a map, what is still missing, and so on). It uses `rustzx-z80` as the processor inside its own bus. Nothing else in this repository is used.

- `master` follows upstream unchanged.
- `zx-sidekick` is `master` plus the patches below. It is the default branch here so this page is what you see first.

## Why a fork

`rustzx-z80` 0.16.0 is a good processor: it passes zexall and the z80test suites, and inside ZX Sidekick's bus it matches the Fuse test corpus's bus activity in all 1,335 cases. But ZX Sidekick needed three workarounds to use it, each because the crate kept something to itself:

1. A machine could not be copied without `unsafe` code.
2. An interrupt could not be seen before its handler had started to run.
3. The halted and just-after-`EI` state could not be read or restored.

Each fix is small, keeps the existing API and behaviour, and is written so it can be offered upstream. Upstream is quiet: the last release is 0.16.0 (31 March 2023), the last change to `rustzx-z80` on `master` was in July 2024, and pull requests have been open since 2023. So the patches are carried here rather than waited for.

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

Called repeatedly, `step()` runs a program as `emulate()` does, with the same timing, provided the bus stops reporting an NMI once it has been taken. After an interrupt it calls `Z80Bus::pc_callback` with the handler's address, as it does after an instruction, so a breakpoint on the handler is hit before the handler runs.

`emulate()` is unchanged: it now calls the same two halves (`check_interrupt` and `execute_instruction`, both private) that `step()` does.

Tested in `tests/integration/interrupt.rs`: the handler not having run after `Step::Interrupt`, the 13 T-states of an IM 1 interrupt, the instruction after `EI`, leaving a `HALT`, and a program with `EI`, `HALT` and a steady interrupt run both ways, compared on registers, stack and clocks.

### 3. What `halted` and `skip_interrupt` mean

**Problem.** Both fields were `pub(crate)` in 0.16.0, so a caller could not put the processor into a halted state (a snapshot taken on a `HALT`, a test starting there) and had to work out for itself where an interrupt taken on a `HALT` returns to.

**Already upstream.** Upstream `master` made both fields public (for SZX snapshot loading) after 0.16.0; it has not been released. Their comments described them from inside the crate.

**Change.** The comments now say what a caller needs: while halted, the program counter stays at the `HALT` and moves past it when an interrupt is taken; `skip_interrupt` is set by `EI`, `DI` and chained `DD`/`FD` prefixes, and holds off interrupts for one step. `tests/integration/interrupt.rs` restores a halted state and checks where the interrupt returns.

### 4. No `unsafe`, and pedantic clippy

**Change.** `rustzx-z80/Cargo.toml` forbids `unsafe` code (there was none) and denies clippy's pedantic lints, so `cargo clippy -p rustzx-z80 --all-targets` fails on any warning. The code was brought in line without changing behaviour: explicit `u16::from` widening, `cast_signed()` for displacement bytes, `wrapping_add_signed` for relative addresses, `#[must_use]` on getters, reasons on the ignored zexall tests. Two lints are relaxed, each with its reason written next to it: truncating casts (a Z80 takes the low byte of wider results everywhere), and the length of the two opcode dispatch functions (one match arm per opcode group).

## Using it

The crate's name and version are unchanged, so it replaces the published crate for a project that depends on `rustzx-z80 = "0.16"`:

```toml
[patch.crates-io]
rustzx-z80 = { git = "https://github.com/zx-sidekick/rustzx", rev = "<commit>" }
```

Pin a commit rather than the branch, so a rebase here cannot change a build. This brings in upstream `master`'s unreleased `rustzx-z80` changes too: the public `halted` and `skip_interrupt` fields, and `Regs::set_q`.

## How it is checked

```
cargo clippy -p rustzx-z80 --all-targets
cargo test --release -p rustzx-z80 -- --include-ignored
cargo test --release -p rustzx-test -- --ignored z80full z80ccf z80memptr
```

On the `zx-sidekick` branch, on 24 September 2026: 121 tests in `rustzx-z80` pass, including all of zexall and the 54 tests of the patches' behaviour, and the three z80test suites pass. `tests/integration/emulate.rs` uses only upstream's interface and passes unchanged on `master` too, which shows `emulate()` behaves as upstream's does. ZX Sidekick's Fuse corpus harness gives the same result on this crate as on 0.16.0: 1,329 of 1,335 cases match exactly (the other 6 differ only in the undocumented bits 3 and 5 of F), and bus activity matches in all 1,335.

## Keeping up with upstream

```
git fetch upstream
git switch zx-sidekick
git rebase upstream/master
```

A patch upstream has taken is dropped in the rebase. If upstream releases all three, ZX Sidekick goes back to the published crate and this fork has no reason to exist.

## Licence

MIT, as upstream: `LICENSE.md` is unchanged and the copyright in it stays with its holder. The patches here are under the same licence.
