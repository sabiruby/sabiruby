# SabiRuby

A Rust implementation of the [mruby](https://github.com/mruby/mruby) virtual machine.
It executes RITE bytecode (`.mrb` files produced by `mrbc`) and aims at behavioural
compatibility with mruby 4.1. mruby 4.1.0 itself is not released yet: the reference everything
here is checked against is the release candidate **4.1.0-rc** (tag `4.1.0-rc`, commit
`3cf73ee`), and verification is against that binary rather than against a spec.

The VM runs bytecode only and is pure Rust (`no_std`). Five crates live in this repository:

| crate | what | |
|---|---|---|
| [`sabiruby`](https://crates.io/crates/sabiruby) | the VM library | pure Rust, `no_std` + `alloc`, wasm |
| [`sabiruby-compiler`](https://crates.io/crates/sabiruby-compiler) | the reference compiler (mruby 4.1.0-rc's `mruby-compiler`: Prism as its parser, mruby's code generator) built as C; output byte-identical to `mrbc` | needs a C compiler |
| [`sabiruby-cli`](https://crates.io/crates/sabiruby-cli) | the `sabiruby` command: `sabiruby foo.rb`, `-e`, `compile`, `dump`, with the switches of the reference `mruby` | depends on both |
| [`sabiruby-macros`](https://crates.io/crates/sabiruby-macros) | `#[derive(RubyClass)]` and `#[ruby_methods]`: a Rust struct and its `impl` block as a Ruby class ([`docs/design/macros.md`](docs/design/macros.md)) | depends on neither; reached through `sabiruby`'s feature `macros` |
| [`sabiruby-serde`](https://crates.io/crates/sabiruby-serde) | serde and a `JSON` class on top of the VM ([`docs/design/serde.md`](docs/design/serde.md)) | the VM itself never depends on serde |

The Bevy integration lives in a separate crate, [`rubevy`](https://github.com/sabiruby/rubevy).
Try it in the browser: **[SabiRuby Playground](https://sabiruby.github.io/sabiruby-playground/)**
(the VM and the reference compiler as WebAssembly; [`docs/design/playground.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/playground.md)).

The design follows the book *Deep dive into mruby* (in Japanese): the register
layout (`R0` of the callee is `R[a]` of the caller), `OP_ENTER`, environments,
the `RBreak`-based unwinding through `ensure`, `OP_CALL` as the body of
`Proc#call`, and so on are ported from the book's description of `src/vm.c`.

What changed in each release is [`CHANGELOG.md`](CHANGELOG.md).

## Status (2026-09-17, 0.5.0)

* RITE 04.00 reader (IREP / LVAR; DBG skipped), all 119 opcodes decoded, `EXT1..3` handled.
* Interpreter with methods, blocks/closures (attached/detached environments), `super`
  (incl. `ARGARY`), `rescue`/`ensure` with correct non-local exits (`return`/`break`/`JMPUW`
  through `ensure`), class/module/singleton classes, class variables, `method_missing`.
* Native core classes: Object, Module, Class, Kernel, NilClass/TrueClass/FalseClass,
  Integer (immediate while it fits in 64 bits, a heap value of arbitrary width beyond that,
  see mruby-bigint below), Float, Symbol, String (characters, as the reference built with
  `MRB_UTF8_STRING`; bytes without the feature `utf8` — [`docs/design/utf8.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/utf8.md)),
  Array, Hash, Range, Proc, Exception hierarchy.
* mruby's own `mrblib/*.rb` (Enumerable, Comparable, `Array#each`, `Integer#times`, …) is
  compiled by the reference `mrbc` and embedded (`src/mrblib/core.mrb`), so those run as bytecode.
* Step execution with an instruction budget (`Vm::start` / `Vm::step`) for host loops.

* Keyword parameters, visibility (`private`/`protected`/`module_function`), `prepend`,
  hooks (`inherited`, `included`, `method_added`, …), `defined?`, frozen objects.
* Gems: mruby-fiber (`Fiber`, contexts switched like mruby's `mrb->c`, see
  [`docs/design/fibers.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/fibers.md)), mruby-enumerator, and mruby-array-ext, -enum-ext,
  -hash-ext, -range-ext, -string-ext, mruby-sprintf, -metaprog, -proc-ext, -method, and the
  rest of the reference's `default.gembox` except the POSIX ones — 36 gems, plus
  mruby-cmath from outside it. **mruby-regexp** is there with `Regexp`, `MatchData`, the String
  and Symbol methods and `$~`; its pattern engine is Rust's `regex-automata` rather than a port
  of the reference's NFA, so backreference, lookaround, atomic group and subexpression call are
  refused with `RegexpError` (the list is in
  [`docs/design/gems.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/gems.md), "Deviations kept").
  **`require`/`load`** are there too (mruby has none; the shape is PicoRuby's, the file reading is
  the host's, and `$LOAD_PATH` is the program's directory and the working directory for the
  `sabiruby` command). **mruby-task** brings `Task`, `Task::Queue` and a task-aware `sleep`: a
  priority scheduler with preemption, each task a context of its own like a Fiber. The tick the
  reference gets from a timer is counted in instructions here, and `Vm::task_run_once` is the
  shape a host loop wants (one task per frame) where `Task.run` runs until every task is done.
  **mruby-sleep** and **mruby-strftime** came with them: a `sleep` outside a task waits through a
  host hook, and `Time#strftime` is written out rather than handed to the C library. The numeric tower is complete: **mruby-bigint**
  (an Integer that leaves the 64-bit range grows instead of raising), **mruby-rational** and
  **mruby-complex** (natives
  in `src/builtins/ext_*.rs`, the gems' Ruby parts embedded as `src/mrblib/<gem>.mrb` and
  loaded in the reference gembox order; see [`docs/design/gems.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/gems.md)). `send`/`__send__`
  from bytecode dispatch in place, as in mruby, so a `Fiber.yield` behind them is not a native
  boundary.
* Garbage collection: stop-the-world mark & sweep with a free list, run at instruction
  boundaries and never while a native is on the host stack (natives need no arena; a host
  keeping objects across calls uses `Vm::gc_register`). `GC.start`/`enable`/`disable`,
  `interval_ratio`, `malloc_threshold`, `GC.stat[:live]` are real. `SABIRUBY_GC_STRESS=1`
  collects after every allocation. See [`docs/design/gc.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/gc.md).
* `eval`, `instance_eval`/`class_eval` with a string, `Kernel#binding`, `Binding` and
  `Proc#binding`: the VM asks a host to compile (`Vm::set_host`, `src/host.rs`), which the
  `sabiruby-compiler` crate provides, so a string sees and writes the caller's local
  variables as it does in the reference.
* Compiler: the reference `mruby-compiler` (Prism as its parser, mruby's own code generator)
  linked as C, in the `sabiruby-compiler`
  crate; the VM crate does not depend on it, the `sabiruby` command (`sabiruby-cli`) does.
  Output is byte-identical to `mrbc` for every `.rb` in the repository. See
  [`docs/design/compiler.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/compiler.md).

Not yet: `$!` (nil even inside `rescue`; use
`rescue => e`), the remaining mrbgems (`io`, …),
encodings other than UTF-8 (`Encoding` and `force_encoding` come with mruby-encoding;
`String#b` is here). Native code may re-enter the VM
(`Vm::funcall`, `Vm::call_block`); the Future-native design is a later step.

Known deviations from the reference: a NaN has no identity (Floats are immediates, so two
NaNs made apart are `equal?`), a hash pattern whose keys mutate the subject during
matching is not detected, and five string methods cut where their offset was measured rather
than where the reference's own byte-for-character slip cuts ([`docs/design/utf8.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/utf8.md)).

## Rules

* **`no_std` + `alloc`.** The library must not use `std::` (only `core::`/`alloc::`,
  `hashbrown` for hash maps, `libm` for float math). `tools/check_no_std.sh` builds the
  library for `thumbv7em-none-eabi` and greps for `std::`; CI runs it. The `std` feature
  (default) only adds `std::error::Error` for `VmError`. The C compiler and the command line
  tool are the separate crates `sabiruby-compiler` and `sabiruby-cli`.
* **Behaviour is checked against the reference, not against memory.** Every claim about
  mruby semantics is verified with the 4.1.0-rc binary (fixtures, test suite below).

## Verification

`tests/fixtures/*.rb` are compiled and run by the reference mruby 4.1.0-rc
(Docker image `kishima/mruby:4.1.0-rc`, see `tools/fixtures.sh`); `.out` holds the
reference stdout, `.dump` the `mrbc --verbose` listing. `cargo test` runs every `.mrb`
on SabiRuby and compares stdout byte for byte. All 18 fixtures pass (`gc.rb` also under `SABIRUBY_GC_STRESS=1`); `utf8.rb` is compared with
the image of the build's own reading (`kishima/mruby:4.1.0-rc-utf8` by default).

mruby's own test suite (`test/t`) plus the tests of the ported gems (`gem_*`) passes 2344 of
2507 in the default build and 2267 of 2452 in a byte-string one — each build runs the
assertions written for it and has its own floor (see [`docs/verification/mrbtest.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/verification/mrbtest.md)
and [`docs/verification/mrbtest-bytes.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/verification/mrbtest-bytes.md), reasons for the rest in
[`docs/verification/mrbtest-notes.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/verification/mrbtest-notes.md)).
Most of what does not pass is mruby-regexp's engine: 82 crashes are patterns using a construct a
finite automaton has none of, and 39 KO are what the two engines answer differently. The rest:
11 need the C test fixtures of mruby-test (`env.c`, `vformat.c`, `sysfail.c`,
`ary_shared.c`), a handful are the deviations above, 1 is `(1..).last`, where the core test and
mruby-range-ext disagree (the reference `mruby` crashes on it too), and the remaining ones are
skips the reference makes too (build-dependent).
`tools/mrbtest.sh` compiles the gem tests and gem mrblibs too (`GEMS` in the script).

The reference image includes the default gembox (array-ext, hash-ext, compar-ext, …). The
gems listed under Status are ported; of the others, only `Comparable#clamp` (mruby-compar-ext)
is provided, natively.

### SabiRuby's own tests

`tests/custom/<case>.rb` are tests written for SabiRuby itself: behaviour the
reference `mruby` gets wrong in 4.1.0-rc (with the upstream fix named), things
the reference test suite does not cover, and features planned but not built
yet. Each case has a hand-decided `.expected` (its header says from what:
CRuby, an mruby master commit, or the reference), the reference output
`.rc.out` to show where they differ on purpose, and a `.mrb` compiled by the
reference `mrbc` (`tools/custom.sh`, Docker). A header line
`# pending: <feature>` marks a case that must fail until that feature exists;
the runner (`tests/custom.rs`, part of `cargo test`) fails when a pending case
starts passing, so the marker is removed with the feature. The fixtures and
mruby's suite above are the baseline and are not changed by this.

### mruby's own test suite

`tools/mrbtest.sh` copies `test/assert.rb` and `test/t/*.rb` from the reference tree,
compiles them with the reference `mrbc` (Docker) and runs each file on a fresh VM
(`sabiruby mrbtest`). The result is written to [`docs/verification/mrbtest.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/verification/mrbtest.md):
a per-file table of `report` counts (ok / ko / crash / warn / skip) and the list of
opcodes the suite never executed. `tests/mrbtest/baseline.txt` records the `ok` count
per file and `cargo test` fails if any file drops below it; refresh it with
`tools/mrbtest.sh --update` after an improvement. `tools/mrbtest.sh -v hash` prints the
individual assertion messages of one file.

Errors the VM raises for missing features surface as `NotImplementedError`, so the
suite keeps going and the table shows them as "crash".

## Performance

`tools/bench.sh` runs the benchmarks of `bench/` — mruby's own `benchmark/*.rb` and a set
grouped by what they ask about (instruction loop, calls, data structures, memory, whole
program; `bench/categories.tsv`) — on the reference `mruby` and on SabiRuby, and writes a
TSV and a Markdown report per category into `bench/results/`. `SABIRUBY_BIN` points it at a
binary already built, so any two commits can be measured the same way and put side by side
with `tools/bench_compare.sh`; without Docker the reference columns stay empty and SabiRuby's
own numbers still come out. The kept results are in
[`docs/verification/bench.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/verification/bench.md) (best and median of the runs, plus instruction counts and
ns/instruction from `sabiruby run --stats`). The ratio column is the number to watch: the
first baseline (2026-09-11) was 1.8x–3.5x slower on arithmetic and 16x on array-heavy code;
after the work of 2026-09-15 the whole set is 2.8x (median 3.05x), Hash 2.9x and `so_lists`
4.4x — what changed and why is [`docs/design/optimizations.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/optimizations.md) (Japanese).
The value representation (16-byte enum) and the heap (index into a `Vec`) are the
known structural costs; measure before changing them. Storage (registers, array elements,
hash entries, ivars, envs, constants, globals) holds `Slot`; computation works on `Value`;
`slot.get()` / `Slot::from(v)` are the only crossings, so an 8-byte representation can be
tried by changing `value.rs` alone. Predictions and measurements: [`docs/design/performance.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/performance.md).
Exception/break unwinding without longjmp: [`docs/design/exceptions.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/exceptions.md).
Compiler: [`docs/design/compiler.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/compiler.md) (the plan: [`docs/plans/compiler-plan.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/plans/compiler-plan.md)).
GC: [`docs/design/gc.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/gc.md) (the plan it was built from: [`docs/plans/gc-plan.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/plans/gc-plan.md)).
`eval`, `Binding` and `require` (the plan they were built from; `require` follows PicoRuby's approach): [`docs/plans/eval-require-plan.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/plans/eval-require-plan.md).
Looking inside the VM (snapshots, the trace of events, the DBG line numbers; what the playground's
debugger reads): [`docs/design/inspect.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/inspect.md).
Strings as characters, and what each build answers: [`docs/design/utf8.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/utf8.md)
(the plan it was built from: [`docs/plans/utf8-plan.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/plans/utf8-plan.md)).
Remaining gems and their order: [`docs/design/gems.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/gems.md).

## Usage

The library: `cargo add sabiruby` (`default-features = false` for `no_std`). The command:
`cargo install sabiruby-cli` (installs `sabiruby`; building it compiles the C sources of the
reference compiler, about 2 s in a debug build and 7 s in a release build, on one core).

How to depend on it:

```toml
[dependencies]
sabiruby = "0.5"                                          # the VM, std + utf8 + regexp
sabiruby = { version = "0.5", default-features = false }   # no_std + alloc, byte strings, no Regexp
sabiruby = { version = "0.5", features = ["macros"] }      # + #[derive(RubyClass)] / #[ruby_methods]

sabiruby-compiler = { version = "0.2", features = ["host"] }  # compile Ruby source in the same program
sabiruby-serde = "0.1"                                        # serde and a JSON class
```

The feature `macros` re-exports the two macros of `sabiruby-macros` at the crate root, so that
one dependency and one `use` are all a host writes (the arrangement serde and serde_derive
have). It is off by default: a proc macro is built for the host machine, and the VM must keep
building for targets that have no such toolchain in the picture.

```rust
use sabiruby::{RubyClass, ruby_methods, Vm};

#[derive(RubyClass)]
struct Player { hp: i64 }

#[ruby_methods]
impl Player {
    fn new(hp: i64) -> Self { Player { hp } }        // Player.new(100)
    fn damage(&mut self, n: i64) { self.hp -= n; }   // player.damage(10)
    fn hp(&self) -> i64 { self.hp }                  // player.hp
}
// Player::register(&mut vm)?;   the class, the store, the methods
```

In this repository:

The switches are the reference `mruby` command's (`sabiruby -h` lists them); `compile` is `mrbc`:

```
sabiruby foo.rb arg1 arg2      # Ruby source or a .mrb; the arguments go to ARGV
sabiruby -e 'p [1, 2].sum'     # one line of script (-e may be repeated)
echo 'puts 1' | sabiruby       # no program file: read it from standard input
sabiruby -c foo.rb             # check syntax only ("Syntax OK")
sabiruby -v foo.rb             # version, then the instruction listing, then run
sabiruby -b foo.mrb            # bytecode only; -d sets $DEBUG; --stats prints instructions/time/GC
sabiruby compile foo.rb -o foo.mrb   # like mrbc (-g, -c, --remove-lv, --no-ext-ops, --no-optimize)
sabiruby dump foo.rb           # instruction listing (.rb or .mrb)
sabiruby mrbtest tests/mrbtest/assert.mrb tests/mrbtest/hash.mrb   # test suite
```

A subcommand name wins over a file of the same name: run `./compile` (or `sabiruby run compile`)
for a program file called `compile`. In this repository, use `cargo run -p sabiruby-cli -- …`.

```rust
let mut vm = sabiruby::Vm::with_mrblib()?;   // core library loaded
vm.load_and_run(&bytes)?;                   // run a RITE binary to completion
let out = vm.take_output();                 // what puts/p printed

// or stepped, e.g. once per frame:
let irep = vm.load(&bytes)?;
vm.start(irep);
loop {
    match vm.step(10_000)? {               // at most 10k instructions
        sabiruby::Step::Paused => { /* next frame */ }
        sabiruby::Step::Finished(v) => break,
    }
}
```

## Layout

| path | content |
|---|---|
| `src/rite.rs` | RITE binary reader |
| `src/opcode.rs` | opcode table generated from mruby's `ops.h` |
| `src/vm.rs` | interpreter loop, frames, environments, unwinding |
| `src/bigint.rs` | integers wider than 64 bits (mruby-bigint's `mpz_*`) |
| `src/builtins/ext_rational.rs`, `ext_complex.rs`, `ext_cmath.rs` | the rest of the numeric tower |
| `src/builtins/ext_pack.rs` | `Array#pack` / `String#unpack` |
| `src/host.rs`, `src/builtins/ext_eval.rs`, `ext_binding.rs` | what the VM asks its host for (compiling a string), `eval` and `Binding` |
| `src/object.rs` | heap objects (classes, procs, envs, strings, arrays, hashes), mark & sweep |
| `src/builtins/` | native methods per class |
| `src/mrblib/core.mrb` | mruby's `mrblib/*.rb`, compiled by the reference `mrbc` |
| `tests/fixtures/` | reference programs, bytecode and expected output |
| `tools/fixtures.sh` | regenerates the fixtures and `mrblib.mrb` with Docker |
| `src/mrbtest.rs`, `tests/mrbtest/` | runner and compiled files of mruby's test suite |
| `compiler/` | crate `sabiruby-compiler`: the vendored reference compiler, C shim, golden tests |
| `cli/` | crate `sabiruby-cli`: the `sabiruby` command |
| `macros/` | crate `sabiruby-macros`: the derive and attribute macros, and their tests |
| `serde/` | crate `sabiruby-serde`: serde conversions and the `JSON` class |
| `tools/vendor_compiler.sh` | refreshes `compiler/vendor/` from the reference tree |
| `tools/mrbtest.sh`, `tools/check_no_std.sh` | test-suite report, no_std rule |

## License

MIT (`LICENSE`). The files in `src/mrblib/` are compiled from mruby's `mrblib`
and the Ruby parts of its bundled gems, and `tests/mrbtest/src/` holds copies of mruby's test
suite; those are MIT licensed, Copyright (c) 2010- mruby developers (`LICENSE-mruby`).
