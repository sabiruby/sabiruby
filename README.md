# SabiRuby

A Rust implementation of the [mruby](https://github.com/mruby/mruby) virtual machine.
It executes RITE bytecode (`.mrb` files produced by `mrbc`) and aims at behavioural
compatibility with mruby 4.1. mruby 4.1.0 itself is not released yet: the reference everything
here is checked against is the release candidate **4.1.0-rc2** (tag `4.1.0-rc2`, commit
`c17ffcc24`), and verification is against that binary rather than against a spec.

The VM runs bytecode only and is pure Rust (`no_std`). Five crates live in this repository:

| crate | what | |
|---|---|---|
| [`sabiruby`](https://crates.io/crates/sabiruby) | the VM library | pure Rust, `no_std` + `alloc`, wasm |
| [`sabiruby-compiler`](https://crates.io/crates/sabiruby-compiler) | the reference compiler (mruby 4.1.0-rc2's `mruby-compiler`: Prism as its parser, mruby's code generator) built as C; output byte-identical to `mrbc`, plus `highlight()` for an editor's colours | needs a C compiler |
| [`sabiruby-cli`](https://crates.io/crates/sabiruby-cli) | the `sabiruby` command: `sabiruby foo.rb`, `-e`, `-r`, `compile`, `dump`, with the switches of the reference `mruby` | depends on both |
| [`sabiruby-macros`](https://crates.io/crates/sabiruby-macros) | `#[derive(RubyClass)]` and `#[ruby_methods]`: a Rust struct and its `impl` block as a Ruby class ([`docs/design/macros.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/macros.md)) | depends on neither; reached through `sabiruby`'s feature `macros` |
| [`sabiruby-serde`](https://crates.io/crates/sabiruby-serde) | serde and a `JSON` class on top of the VM, and `declare`: data *written* in Ruby collected into a Rust table ([`docs/design/serde.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/serde.md)) | the VM itself never depends on serde |

The Bevy integration lives in a separate crate, [`rubevy`](https://github.com/sabiruby/rubevy).
Try it in the browser: **[SabiRuby Playground](https://sabiruby.github.io/sabiruby-playground/)**
(the VM and the reference compiler as WebAssembly; [`docs/design/playground.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/playground.md)).

The design follows the book *Deep dive into mruby* (in Japanese): the register
layout (`R0` of the callee is `R[a]` of the caller), `OP_ENTER`, environments,
the `RBreak`-based unwinding through `ensure`, `OP_CALL` as the body of
`Proc#call`, and so on are ported from the book's description of `src/vm.c`.

What changed in each release — every crate's version, what it added, and what a host has to
change to move up — is [`CHANGELOG.md`](https://github.com/sabiruby/sabiruby/blob/main/CHANGELOG.md).
This page says what is here now; the versions and the dates live there.

## Status

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
* Embedding from Rust: `Vm::define_fn` makes an ordinary Rust function or closure a Ruby
  method, reading its arguments through `FromRuby` and writing its answer through `IntoRuby`;
  a Rust value stays in Rust, in a `HostStore` the VM carries, and Ruby holds a `Data` handle
  the collector gives back (`Vm::set_on_free`). `#[derive(RubyClass)]` and `#[ruby_methods]`
  write that registration (see Usage below). `Vm::set_host_state`, `Vm::ivar_get` /
  `global_get` by name and the task entry points are the rest of the door.
* Line numbers for a host's own messages: `Vm::next_line` (the instruction that will run next,
  which is what a debugger stopped at a boundary highlights) and `Vm::backtrace_line` (the line
  a native was called *from*), next to `Vm::backtrace`, snapshots and an event trace
  ([`docs/design/inspect.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/inspect.md)).

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
  **Where a task can wait** (`Task::Queue#pop`, `sleep`, `Task.pass`): anywhere in Ruby code,
  including inside the blocks of `instance_exec`/`instance_eval`, `Method#call`, `Class#new`,
  `index { }`, `Array.new(n) { }`, a Hash's default proc and `Class.new { }` (on `main`, not yet
  released; 0.6.x cannot wait inside `instance_exec`, `instance_eval` or `Method#call`). **Where it
  cannot** — `sort { }`, `ObjectSpace.each_object { }`, `sub`/`gsub`/`scan` with a block, a block a
  host function calls back, and the calls a builtin makes on its own (`join`'s `to_s`, `include?`'s
  `==`, …) — it raises `can't wait inside <the builtin>'s call to <what> (<the wait>)` rather than
  going on silently. The full list and the reasons:
  [`docs/design/wait-anywhere.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/wait-anywhere.md), "What is still a boundary".
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
  Output is byte-identical to `mrbc` for every `.rb` in the repository. The same crate answers
  `highlight(src)` with one category byte per source byte — what an editor needs to colour Ruby
  without writing a tokeniser, and it answers for broken source too. See
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
  mruby semantics is verified with the 4.1.0-rc2 binary (fixtures, test suite below).

## Verification

`tests/fixtures/*.rb` are compiled and run by the reference mruby 4.1.0-rc2
(Docker image `kishima/mruby:4.1.0-rc2`, see `tools/fixtures.sh`); `.out` holds the
reference stdout, `.dump` the `mrbc --verbose` listing. `cargo test` runs every `.mrb`
on SabiRuby and compares stdout byte for byte. All 18 fixtures pass (`gc.rb` also under `SABIRUBY_GC_STRESS=1`); `utf8.rb` is compared with
the image of the build's own reading (`kishima/mruby:4.1.0-rc2-utf8` by default, built by `tools/utf8-image/build.sh`).

mruby's own test suite (`test/t`) plus the tests of the ported gems (`gem_*`) passes 2344 of
2508 in the default build and 2267 of 2453 in a byte-string one — each build runs the
assertions written for it and has its own floor (see [`docs/verification/mrbtest.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/verification/mrbtest.md)
and [`docs/verification/mrbtest-bytes.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/verification/mrbtest-bytes.md), reasons for the rest in
[`docs/verification/mrbtest-notes.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/verification/mrbtest-notes.md)).
Most of what does not pass is mruby-regexp's engine: 82 crashes are patterns using a construct a
finite automaton has none of, and 39 KO are what the two engines answer differently. The rest:
11 need the C test fixtures of mruby-test (`env.c`, `vformat.c`, `sysfail.c`,
`ary_shared.c`), a handful are the deviations above, 1 is `(1..).last`, where the core test and
mruby-range-ext disagree (the reference `mruby` crashes on it too), 1 is 4.1.0-rc2's new `hash`
assertion about an `eql?` that deletes entries mid-lookup (not decided yet; see the notes), and the remaining ones are
skips the reference makes too (build-dependent).
`tools/mrbtest.sh` compiles the gem tests and gem mrblibs too (`GEMS` in the script).

The reference image includes the default gembox (array-ext, hash-ext, compar-ext, …). The
gems listed under Status are ported; of the others, only `Comparable#clamp` (mruby-compar-ext)
is provided, natively.

### SabiRuby's own tests

`tests/custom/<case>.rb` are tests written for SabiRuby itself: behaviour the
reference `mruby` gets wrong in 4.1.0-rc2 (with the upstream fix named), things
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
ns/instruction from `sabiruby run --stats`). The ratio to the reference is the number to
watch, and the last baseline in that file is the current one: the first one was 1.8x–3.5x on
arithmetic and 16x on array-heavy code, and the whole set of 27 benchmarks is **2.57x** by
total time, 2.85x by the median of the per-benchmark ratios (`bench/results/96221fb.md`; the
numbers here are whatever that file's last table says, not a promise). What changed and why is
[`docs/design/optimizations.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/optimizations.md) (Japanese).
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
`cargo install sabiruby-cli` (installs `sabiruby`; it needs a C compiler, because building it
builds the reference compiler's C sources, which is most of the time a clean build takes).

How to depend on it:

```toml
[dependencies]
# the VM. Pick one line:
sabiruby = "0.6"                                          # std + utf8 + regexp (the defaults)
# sabiruby = { version = "0.6", default-features = false } # no_std + alloc, byte strings, no Regexp
# sabiruby = { version = "0.6", features = ["macros"] }    # + #[derive(RubyClass)] / #[ruby_methods]

# optional companions
sabiruby-compiler = { version = "0.3", features = ["host"] }  # compile Ruby source in the same program
sabiruby-serde = "0.2"                                        # serde, and a JSON class
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
sabiruby -r lib foo.rb         # require the library first (-r may be repeated)
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
