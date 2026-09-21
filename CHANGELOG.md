# Changelog

What changed in each release of the crates in this repository: `sabiruby` (the VM),
`sabiruby-compiler`, `sabiruby-cli`, `sabiruby-macros` and `sabiruby-serde`. They share a
repository and a history but not a version; each entry says which crate it belongs to.

Every claim here is traceable to a commit or to a document under `docs/`, and the commit is
named. Measurements are the ones in [`docs/verification/bench.md`](docs/verification/bench.md);
nothing is estimated.

## Unreleased

`sabiruby-serde` — a module for data *written* in Ruby, and the two things writing a game's
data stage on it turned up. The compiler and the macros are untouched; the VM gains one
read-only entry point, loses one deprecated alias and changes no behaviour. No version is
raised here — publishing is the author's — but the removal is a breaking change, so **the next
release of `sabiruby` is 0.6.0**, not a patch
([`docs/design/serde.md`](docs/design/serde.md) "Declarations",
[`docs/worklog/2026-09-20-declare.md`](docs/worklog/2026-09-20-declare.md),
[`docs/worklog/2026-09-21-serde-lines.md`](docs/worklog/2026-09-21-serde-lines.md)).

* **`sabiruby_serde::declare`** — `Declarations<T>` grows a method on the VM
  (`unit :metre, symbol: "m", scale: 1.0`), reads each call's keyword Hash as a `T` through
  `from_value`, and answers the host with `Vec<(String, T)>` in the order the script declared
  them once it has run. Because the deserialization happens inside the native, everything
  serde refuses — a missing field, a field of the wrong type, an unknown field under
  `#[serde(deny_unknown_fields)]` — raises at the line of that declaration in the script's own
  file. A name declared twice raises `ArgumentError`; the method that amends an earlier
  declaration instead is a second, differently named one (`define_replacing`), so overwriting
  is asked for rather than stumbled into. `expose` is the way back: a host table a script
  looks up by name, answering with a Hash with Symbol keys. The tables live in the type's
  `HostStore` in the VM — not in `Vm::set_host_state`, which an embedder may be using, and not
  behind a lock, which `no_std` has none of — hold no Ruby value, and `take` moves them out
  for good: a VM whose declarations have been taken and collected has exactly as many live
  objects as one that was given none (`serde/tests/declare.rs`).

* **`Options::symbol_map_keys` and `Options::symbols()`** (`491ec0e`) — a *map's* keys as
  Symbols where they serialize as Strings, so a table declared in Ruby as
  `recipe :iron_plate, in: { iron_ore: 1 }` reads back as
  `recipe_of(:iron_plate)[:in][:iron_ore]` and not `[:in]["iron_ore"]`. `symbol_keys` keeps
  its meaning — a struct's field names are finite and the type decides them, while a map's
  keys are runtime values with no bound on how many there are and the VM's symbol table is
  never collected — so this is a switch of its own, off by default, and `Options::symbols()`
  is both. `declare::expose` uses it: the keys of a published table's maps are the names of
  declared things, and the Ruby data file wrote them as Symbols in the first place. `Options`
  becomes `#[non_exhaustive]`, which is a **breaking change** for anything that built one with
  a struct literal (nothing in this repository, rubevy, rubevy_games, sabiruby-playground or
  mruby-porting-kit does); a host makes one with `default()`, `symbol_keys()` or `symbols()`
  and assigns to the public fields.

* **`Declarations::take_with_lines` and `Declared<T>`** (`d0bbde0`) — each declaration with the
  line of the script it was on, for the checks serde cannot make. What a declaration says
  about itself is refused inside the native, at that line; what needs two declarations to be
  wrong — a recipe naming an item nothing declared — is the host's to check afterwards, and
  `take`'s `Vec<(String, T)>` had nothing to point at. It is the same line serde's refusal
  reports, from the same place, and `take` is written on top of `take_with_lines` so there is
  one place that records it. An amended declaration (`define_replacing`) keeps its place in
  the order and takes the later line; bytecode built without debug information has `None`.

`sabiruby` — one entry point, one rename and the removal of the name it replaced, no behaviour
changed (`d0bbde0`).

* **`Vm::backtrace_line() -> Option<u32>`** — the line the first frame of `Vm::backtrace`
  carries, which from inside a native (which pushes no frame of its own) is the line of the
  call. Distinct from `Vm::next_line`, which answers with the line of the instruction that
  will run **next**, since `ci.pc` is past the instruction being executed: that is what a
  debugger stopped at an instruction boundary wants to highlight, and it is one line late for
  anything asking where it was called from (`tests/native.rs`).

* **`Vm::current_line` is renamed `Vm::next_line`; the old name is gone** — a breaking change,
  which is why the next release is **0.6.0** and not a patch on 0.5.2. Nothing about what the
  VM computes changed — the name did, because it invited the wrong reading: asked from inside
  a native it looks like it will say where the native was called from, and it says the line
  after. `next_line` says what it answers, and `backtrace_line` is the other question.

  **Coming from 0.5.2:** replace `current_line` with `next_line` — same answer, same type,
  nothing else to do; if what you actually wanted was the line an error names (where a native
  was called from, which is one line earlier), that is `backtrace_line`.

  The rename first landed with a `#[deprecated]` alias, because three repositories pin each
  other by commit and the old and the new name had to both work while those pins moved. The
  alias was removed on 2026-09-22, once rubevy_games' `Cargo.lock` and sabiruby-playground's
  `SABIRUBY_REF` both named a commit that has `next_line` and both Pages builds were green —
  the condition written down in
  [`docs/plans/serde-declare-lines-plan.md`](docs/plans/serde-declare-lines-plan.md) §4.

## 0.5.2 — 2026-09-18

`sabiruby-compiler` 0.2.2 → **0.2.3**: one new function, `highlight()`. `sabiruby` 0.5.1 →
**0.5.2**: the VM is unchanged and this is a republish, the way `sabiruby-compiler` 0.2.2 was
one in 0.5.0 — the repository releases under one name. Those two are what was published
(2026-09-18, tag `v0.5.2` at `7be7b86`); `sabiruby-cli` 0.5.0, `sabiruby-macros` 0.1.0 and
`sabiruby-serde` 0.1.0 have no change since `v0.5.1` and stay as they are on crates.io
([`docs/worklog/2026-09-18-release-0.5.2.md`](docs/worklog/2026-09-18-release-0.5.2.md)).

* **`sabiruby_compiler::highlight(src) -> Vec<u8>`** — one category byte per source byte, for
  an editor that wants to colour Ruby without writing a tokeniser. The map is as long as the
  source and every byte is 0..=8: 0 default, 1 keyword, 2 string, 3 comment, 4 number,
  5 symbol, 6 constant, 7 variable, 8 method name. Prism decides, in the two passes
  family-mruby's `picoruby-syntax-highlight` uses and in its order: its lexer
  (`pm_lex_callback_t`, one call per token, in `csrc/shim.c`), so interpolation, regular
  expressions, `%w[]` and heredocs are told apart the way the parser tells them apart; then
  its syntax tree (`pm_visit_node`) for what a token type cannot say — the name of a method
  (`p 1` and `sleep 0.5` as much as `a.b` and `def c`; an operator call is a call, so `=~` and
  `+` count) and the whole of a symbol (`:Plant`, quotes and all). **A source with syntax
  errors still gets a map** — the lexer runs ahead of the parser and the parser recovers —
  which is what an editor needs, since its text is broken most of the time it is looked at.
  Token and node boundaries are character boundaries, so a run never cuts a UTF-8 character
  in half (`compiler/tests/highlight.rs`, which spells out what Prism 1.9.0 answers for each
  case). There is no maximum source size, and
  [`docs/worklog/2026-09-18-highlight.md`](docs/worklog/2026-09-18-highlight.md) says why.
  Nothing else changed: the VM, the bytecode and the golden tests are untouched.

## 0.5.1 — 2026-09-17

`sabiruby` — two fixes to mruby-task's scheduler found by rubevy's garden demo, where replacing
ten scripts at once froze the VM for seconds
([`docs/worklog/2026-09-17-task-end-nil.md`](docs/worklog/2026-09-17-task-end-nil.md),
[`docs/design/gems.md`](docs/design/gems.md)).

* **A turn of `Vm::task_run_limits` no longer ends because a task ended with nil.** *Behaviour
  change.* `Vm::task_run_once` answers the result of a task that finished, and a task whose
  block is worth nil answers the same nil that an empty scheduler does; the budgeted loop read
  that as "nothing is runnable" and returned, spending one host frame per ending task while
  every other task stood still. It now asks the scheduler whether anything is runnable and
  stops only when nothing is (`ext_task::task_step`). A host that gave a budget now gets its
  budget spent where there is work for it: ten tasks ending together in one 200,000-instruction
  frame used to leave a ready worker task 0 instructions, and now leave it 199,997
  (`tests/task.rs`). `Vm::task_run_once` itself is unchanged, and its rustdoc now says what a
  nil answer does and does not mean. A host loop written as `while !vm.task_run_once()?.is_nil()`
  has the same bug; `while vm.task_pending()` is the loop to write.
* **Finished tasks are collected.** The scheduler's dormant queue is no longer a GC root; a
  collection drops from it, after marking and before the sweep, every finished task nothing else
  refers to. Before, only `Task#close` ever removed one, so a host that restarts scripts kept a
  Task object (with its result, name and queue) per restart for the life of the VM — 1000 spawned
  and finished tasks left 2000 live objects behind a `GC.start`, and now leave none
  (`tests/task.rs`). This is a deliberate departure from the reference, which pins every task
  twice (`mrb_task_mark_all` marks all four queues and `task_create_common` registers the
  object); nothing observable changes, since only a task that no Ruby code and no registered host
  handle can name is dropped. The three mrbtest baselines are unchanged.

## 0.5.0 — 2026-09-17

`sabiruby` 0.4.0 → **0.5.0**, `sabiruby-cli` 0.4.1 → **0.5.0**, `sabiruby-compiler` 0.2.1 →
**0.2.2**. `sabiruby-macros` **0.1.0** and `sabiruby-serde` **0.1.0** are published for the first
time. The release covers everything between `a82f163` (which set 0.4.0) and `1c747bf`.

Publishing order is `sabiruby-macros`, `sabiruby`, `sabiruby-compiler`, `sabiruby-serde`,
`sabiruby-cli`: the VM's new `macros` feature makes `sabiruby-macros` an optional dependency of
the VM, so the VM is no longer the first crate to go up
([`docs/design/compiler.md`](docs/design/compiler.md), "Publishing"). `sabiruby-compiler` 0.2.2
is a republish and not a change — nothing of the vendored compiler moved — but its optional
dependency on the VM has to name 0.5.0, or a `sabiruby-cli` would end up with two different
`sabiruby` crates in one program.

### Host bridge

The Rust side of embedding, built as stages 3–6a of
[`docs/plans/host-bridge-plan.md`](docs/plans/host-bridge-plan.md).

* **`Vm::define_fn`** (`src/convert.rs`, `ccc60de`): an ordinary Rust function or closure
  becomes a Ruby method. Arguments are read with **`FromRuby`**, the answer written with
  **`IntoRuby`**, the count is checked with the VM's own wording, and `Method#arity` answers
  with it. The VM (`&mut Vm`), the receiver (`This<T>`) and the caller's block (`Block`) are
  leading and trailing parameters rather than arguments, and a `Result` answer raises. 42
  `macro_rules!` impls of `RubyFn`, no procedural macro.
* **Native closures** (`93824b1`, `1472346`): `Method::Closure` next to `Method::Native`, so a
  native can carry an environment (`Vm::define_closure`) instead of reaching for a `static`.
  The payload is `Arc<ClosureBody>`, a thin pointer, so `Method` is 16 bytes as before.
* **Host state** (`93824b1`): `Vm::set_host_state` / `host_state` / `host_state_mut` /
  `take_host_state` — one `Box<dyn Any + Send + Sync>` the host leaves with the VM, the other
  direction from the `Host` trait.
* **Data objects** (`c667a9d`): `ObjKind::Data { tag, handle }` is mruby's `RData` without the
  pointer — the VM carries two numbers and never reads through them. `Vm::data_new`,
  `Vm::data_of`, `convert::DataRef`, and `Vm::set_on_free`, called at the end of `gc_collect`
  rather than from the sweep, with the two numbers and no `&mut Vm` so it cannot re-enter.
  `dup`/`clone` refuse; `==`/`eql?`/`hash` go by the handle, `equal?` stays identity
  ([`docs/design/gc.md`](docs/design/gc.md), "the free hook").
* **`HostStore`** (`e7ea20e`): a slab per host type inside the `Vm`, with the tag its `Data`
  objects carry. `Vm::install_host_store`, `host_store`, `host_store_mut`, `host_store_tag`,
  `next_data_tag`, and the `RubyClass` trait that ties a Rust type to its class, tag and store.
* **`sabiruby-macros`** (`33fa232`, `72b9fc7`): `#[derive(RubyClass)]` and `#[ruby_methods]` turn
  a struct and its `impl` block into a Ruby class — the class, the store, the tag and one
  `define_fn` per method. A `fn` with no `self` is a class method, a leading `&mut Vm` is the
  call's context, a trailing `Block` is the caller's block (`a9461d8`), `#[ruby(name = "…")]`
  and `#[ruby(skip)]` rename and exclude. What it does not cover is listed in
  [`docs/design/macros.md`](docs/design/macros.md).
* **The `macros` feature on `sabiruby`** (this release): `sabiruby = { version = "0.5",
  features = ["macros"] }` and `use sabiruby::{RubyClass, ruby_methods};` are all a host writes.
  Off by default — a proc macro is built for the host machine and the `no_std` library must keep
  building for targets that have none.
* **Host entry points** (`4e5b590`, stage 3b): `Vm::task_running`, `Vm::ivar_get` / `ivar_set`
  by name, `Vm::global_get` / `global_set` by name, and `Vm::is_exception`, which reads the
  representation rather than sending `is_a?`. Each replaces a place a host was reaching into
  `Vm`'s fields.
* **More of the same** (`6af8276`): `Vm::hash_keys` (insertion order, no Ruby run, `None` for
  anything that is not a Hash), `Vm::task_queue_len` and `Vm::task_queue_try_pop` — which
  answers `None` on an empty or closed queue instead of parking, because a host is not a task
  and has nothing to park.
* **Inspection for a host's UI**: `Vm::task_instructions` and `Vm::task_location` (`d9232a5`),
  `Vm::task_frames`, innermost first (`c086747`), and `Vm::task_context`, which snapshot context
  a task runs in (`ad8e7cb`).
* **`method_missing` and the index opcodes dispatch in the caller's frame** (`6314b84`,
  `ad54ed4`): a `method_missing` written in Ruby, and `[]`/`[]=` behind `OP_GETIDX`,
  `OP_GETIDX0` and `OP_SETIDX`, no longer run in a nested run loop. A `Fiber.yield` or a
  blocking `Task::Queue#pop` inside one used to come back as "can't cross C function boundary";
  it now works, which is what a proxy object written with `method_missing` needs.

### Scheduler

* **Time-based slices** (`47c3ef1`): `Vm::task_set_clock` gives the scheduler the host's
  monotonic clock — nothing reads one otherwise. `Timeslice::{Instructions, Time, Both}` says
  how a slice ends (`Instructions` is the default, so the instruction loop pays nothing more);
  the clock is read at the existing 10,000-instruction tick and after every 32 natives while a
  limit is kept on it.
* **`Vm::task_run_limits(RunLimits)`** (`47c3ef1`): an instruction budget as before, plus a time
  budget that cuts the running slice short and hard limits past which a task that cannot be
  switched out is given **`Task::Overrun`** — an `Exception`, not a `StandardError`, so an
  ordinary `rescue` does not swallow it. This is what answers a native that never gives the turn
  back (`sort { }`, `Array.new { loop { } }`), which an instruction-counted timeslice cannot
  interrupt.

### Gems and features

* **`regexp` is a Cargo feature**, on by default (`874c6d1`). Without it the crate does not
  depend on `regex-automata` at all, and what it answers is what a reference built without the
  gem answers, read out of the reference tree rather than guessed. It halves the thumbv7em
  `.text` (1,391,312 → 742,467 at `opt-level = "z"`;
  [`docs/verification/size.md`](docs/verification/size.md)). The rule for when a gem gets a
  feature — only one that drags a crate behind it — is in
  [`docs/design/gems.md`](docs/design/gems.md), "Gems as Cargo features".
* **`sabiruby-serde`** (`bd71d31`, `0be24aa`): Rust values as Ruby values and back through
  serde, `Serde<T>` in a `define_fn` signature, and a **`JSON`** class (`install_json`) built on
  `serde_json` with `preserve_order`, so a Hash keeps the order it was written in as CRuby's
  JSON does. The VM itself never depends on serde
  ([`docs/design/serde.md`](docs/design/serde.md)).
* **`Kernel#printf` and `Kernel#putc`** (`519eb17`). They come from mruby-io, not from a gem
  called mruby-print; there is no IO here, so both write where `print` writes. `putc` of an
  Integer is the one byte `c & 0xff`, of anything else the first character as the *build* counts
  characters. With them, mruby's own `bm_ao_render` and `bm_mandel_term` run and produce output
  byte for byte identical to the reference's.
* **Coverage gaps closed** (`ef4611f`, after the generated
  [`docs/verification/coverage.md`](docs/verification/coverage.md), `f57949b`): `Hash#default_proc=`,
  `Numeric#fdiv`, and the definition hooks `method_added` / `method_undefined` /
  `method_removed` / `const_added`; `singleton_method_added` (and `_removed`, `_undefined`)
  moved to `BasicObject`, where the reference has them. coverage.md's "in the reference, not
  here" list went 22 → 15.
* **`SABIRUBY_VERSION`** (`57f5b8c`): the constant a script reads to tell it is on SabiRuby.
  `RUBY_ENGINE` deliberately **stays `"mruby"`** — the reference's own tests branch on it — and
  `MRUBY_PLATFORM` stays `"rust-sabiruby"`.
* **`Vm::define_class_under`** (`bd71d31`): a class inside a module, which `sabiruby-serde`
  needed for `JSON::ParserError` and which the macros still do not do for you
  ([`docs/design/macros.md`](docs/design/macros.md), "What it does not cover").

### Performance

The whole benchmark set is measured against the reference `mruby` 4.1.0-rc on the same machine;
read the ratio and not the milliseconds. All of it is in
[`docs/verification/bench.md`](docs/verification/bench.md), and why each change worked is
[`docs/design/optimizations.md`](docs/design/optimizations.md) (Japanese).

* The ratio to the reference over the whole set went **4.32x** (the 0.4.0-era baseline
  `e9da768`, 20 benchmarks, sum of milliseconds) to **2.57x** (`96221fb`, 27 benchmarks; median
  of the per-benchmark ratios 2.85x). The two sets are not the same size — the set grew to 27
  files when `vm_optimization_bench` was cut into five and `printf`/`putc` made the two
  reference benchmarks run — so bench.md gives both a "25 shared" and a "27 all" column at every
  step since.
* Where it came from, in rough order of size: `Array#shift`/`unshift` in O(1) with a start
  offset and a Hash index past 16 entries (`35d58a5`, `8ba2336`, `e2b026b` — data structures
  10.85x → 4.57x in one step), the method cache and a dispatch path that does not clone
  (`4a6826e`, `68261b0`), opcode decoding by `match` (`5119773`), opcode counting off by default
  (`815ba7f`), Hash hashing a key once per store (`2521b79`, `07659aa`), the index opcodes
  answering in place (`ad54ed4`), `String#<<` growing in place and String reads borrowing
  (`a423a92`, `f83cc81`), one pending-check at the loop head (`e26ccff`), and `OP_ADD` and the
  four comparisons answering two numbers where they stand (`a0ef97e`).
* **`codegen-units = 1` is the release default** (`96221fb`). It is worth −2.0% over the 27
  benchmarks measured as an A/B of the same tree built twice, and it makes the `sabiruby`
  binary 16.8% smaller and the thumbv7em archives 8.9–11.8% smaller
  ([`docs/verification/size.md`](docs/verification/size.md)); a clean release build goes 9.2 s →
  18.1 s in wall clock while its CPU time falls 58 s → 39 s. Its real point is that with 16
  units the placement of functions changes with any one-line edit, which was half of this
  machine's ±15% swing between measurements
  ([`docs/worklog/2026-09-17-release-prep.md`](docs/worklog/2026-09-17-release-prep.md)).
* **`unsafe` is zero in the VM crate** (`354b6bb` took the last one out of the opcode decoder;
  `bf5608d` took the 14 raw-pointer reads out of mruby-regexp by sharing the compiled pattern as
  an `Arc<Pattern>`).

### Breaking changes

* **`T::register` answers `VmResult<ObjId>`** instead of panicking (`758e1a2`). The generated
  `register` used to `.expect()` when asking for the class's singleton class; it cannot fail
  there, but an `expect` is the host's choice to make, not the macro's. Callers write
  `Player::register(&mut vm)?`.
* **Opcode counting is off by default** (`815ba7f`). `op_counts` is a fixed `[u64; OP_COUNT]`
  indexed by the decoded opcode, and `Vm::set_op_counting(true)` turns it on; a host that reads
  per-opcode statistics must ask for them. Counting on every instruction cost 20–25% on the
  tightest loops, not the 1% the plan guessed.
* **`Vm::array` and `Vm::ary` answer `Option<&[Slot]>`**, not `Option<&Vec<Slot>>` (`35d58a5`).
  An `ArrayData` now carries a start offset so that `shift` is O(1), so there is no longer a
  `Vec` whose whole contents are the array.
* **`Method` has a new variant, `Method::Closure`** (`93824b1`, `1472346`). The enum is public
  and not `#[non_exhaustive]`, so an exhaustive `match` on it outside the crate stops compiling.
* **A `default-features = false` build has no `Regexp`** (`874c6d1`). `Regexp` and `MatchData`
  are not defined (so a literal `/re/` is a `NameError` on the constant the compiler emits),
  `String#match` / `match?` / `=~` / `scan` and `Symbol`'s three are gone, `$~` is an ordinary
  global and `require "regexp"` raises `LoadError`. `RegexpError` stays, because it is core.
  Add `features = ["regexp"]` to keep the previous behaviour; the default build is unchanged.
* **`Hash#default=` clears a default proc, and `default_proc=` clears a plain default**
  (`ef4611f`). mruby keeps both in one `ifnone` ivar with two flags; `Hash#default=` here used to
  leave a default proc in place.
* **An empty keyword Hash now reaches the callee** through `send` and `method_missing`
  (`ce3a134`), so `def one(x); end; send(:one, **{})` binds `x` to the empty Hash, as the
  reference does.
* **`singleton_method_added` is defined on `BasicObject`**, not on `Object` (`ef4611f`), which is
  where the reference has it.
* **The `TypeError` for an allocated-but-uninitialized host object changed wording** (`8d7bd9d`):
  `Player.allocate.hp` answered "wrong argument type Player (expected Player)" and now answers
  "uninitialized Player (expected Player): the object has no Player behind it — `Player.allocate`
  makes one without running `initialize`", worded after mruby's own `mrb_data_check_type`
  (`uninitialized %t (expected %s)`, `src/etc.c`), and naming the object's own class, so a Ruby
  subclass reads "uninitialized Ghost (expected Player)". A receiver of another class keeps
  "wrong argument type String (expected Player)".
* **A Ruby `method_missing`, and `[]`/`[]=` reached through the index opcodes, are no longer a
  native boundary** (`6314b84`, `ad54ed4`). Code that relied on a `Fiber.yield` inside one
  raising will no longer see the exception; it yields.
* **Built-in method visibility now matches the reference** (`835ac67`, `43bd12f`): the 50
  built-ins mruby defines private (`Module#private`/`module_function`/`included`/`method_added`,
  the `singleton_method_*`/`const_added` hooks, every `initialize`/`initialize_copy` except
  `Struct#` and `Random#`, which the reference itself leaves public, …) are private here too,
  and a top-level `def` is private, as the reference's base frame makes it. `respond_to?`
  no longer looks at visibility — the reference never did — so `Object.new.respond_to?(:puts)`
  is `true`; `public_send` and `Object#method` raise on a private method with the reference's
  wording. `Module#define_method` always defines public. `coverage.md` lists 0 visibility
  differences; the one kept is `` Kernel.` `` (`docs/design/gems.md`).
* **`sabiruby-compiler`'s optional dependency on the VM is `0.5.0`** (this release), so a program
  cannot mix `sabiruby-compiler` 0.2.2 with `sabiruby` 0.4.

### Known differences from the reference

Unchanged in substance, and listed method by method with the reason for each: see
[`docs/design/gems.md`](docs/design/gems.md), "Deviations kept" — including mruby-regexp's
constructs a finite automaton has none of (backreference, lookaround, atomic group,
subexpression call), which are refused with `RegexpError` because the engine here is Rust's
`regex-automata` and not a port of the reference's NFA. What the whole test suite says, file by
file, is [`docs/verification/mrbtest.md`](docs/verification/mrbtest.md) and its `-bytes` and
`-noregexp` companions, and every remaining failure has its reason in
[`docs/verification/mrbtest-notes.md`](docs/verification/mrbtest-notes.md).

## 0.4.0 (`sabiruby`), 0.2.1 (`sabiruby-compiler`), 0.4.1 (`sabiruby-cli`) — 2026-09-13

What a host needs to drive the scheduler itself (`a82f163`):
`Vm::task_next_wakeup_ticks` and `Vm::task_pending` (`4e7c35e`), `Vm::task_queue_new` and
`Vm::task_queue_push` (`d2a5f24`) — how a host answers a script parked on a question — and
`VERSION` and `REVISION` so an embedder can say which VM it is running.

`docs/design/compiler.md` used to list `task_queue_len`, `task_queue_try_pop` and
`Vm::hash_keys` here too. They are 0.5.0's: `git log -S'fn hash_keys' -- src/` and the two
others all answer `6af8276`, which is after `a82f163`. Corrected in this release.

## 0.3.0 (`sabiruby`), 0.2.0 (`sabiruby-compiler`), 0.3.0 (`sabiruby-cli`) — 2026-09-13

The gems (`1fe5f9d`): the numeric tower (mruby-bigint, -rational, -complex, -cmath), `Struct`,
`Set`, `Time`, `Array#pack` / `String#unpack`, `eval` and `Kernel#binding`, UTF-8 strings as a
feature, `require` / `load`, `Regexp` and `MatchData`, and the mruby-task scheduler — 36 gems,
the reference's `default.gembox` except the POSIX ones
([`docs/design/gems.md`](docs/design/gems.md)). The compiler's 0.2.0 adds the `host` feature,
which is what `eval` asks for a compile.

## 0.2.0 (`sabiruby`), 0.1.0 (`sabiruby-compiler`, `sabiruby-cli`) — 2026-09-12

The command moved out into `sabiruby-cli` and `sabiruby` became the VM library alone
(`4acc5db`); the reference `mruby-compiler` was vendored and built as C in `sabiruby-compiler`,
byte-identical to `mrbc` (`dc00a1f`, [`docs/design/compiler.md`](docs/design/compiler.md)).

## 0.1.0 (`sabiruby`) — 2026-09-11

The first crates.io release (`6c27a5e`): the RITE reader, the interpreter, the native core
classes, mruby's own `mrblib` compiled in, stop-the-world mark & sweep
([`docs/design/gc.md`](docs/design/gc.md)) and `no_std` + `alloc`.
