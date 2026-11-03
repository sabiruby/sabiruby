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

* RITE 04.00 reader (IREP / LVAR / DBG: local variable names and line numbers), all 119 opcodes decoded, `EXT1..3` handled.
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
  `index { }`, `Array.new(n) { }`, a Hash's default proc and `Class.new { }` (since 0.7.0;
  0.6.x cannot wait inside `instance_exec`, `instance_eval` or `Method#call`). **Where it
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
* Checkpoint (feature `checkpoint`, off in the library, on in the `sabiruby` command): a program
  that C mruby stopped and wrote out with
  [mruby-checkpoint](https://github.com/family-mruby/mruby-checkpoint) (its format v1: objects,
  frames, registers, Tasks and Fibers) goes on here from the instruction after where it stopped —
  `Vm::checkpoint_resume`, `sabiruby --resume IMAGE`. One way only (C mruby to SabiRuby); what an
  image may not hold is refused with the reason
  ([`docs/plans/checkpoint-plan.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/plans/checkpoint-plan.md), in Japanese).

Not yet: `$!` (nil even inside `rescue`; use
`rescue => e`), the remaining mrbgems (`io`, …),
encodings other than UTF-8 (`Encoding` and `force_encoding` come with mruby-encoding;
`String#b` is here). Native code may re-enter the VM
(`Vm::funcall`, `Vm::call_block`); the Future-native design is a later step.

Known deviations from the reference: a NaN has no identity (Floats are immediates, so two
NaNs made apart are `equal?`), a hash pattern whose keys mutate the subject during
matching is not detected, and five string methods cut where their offset was measured rather
than where the reference's own byte-for-character slip cuts ([`docs/design/utf8.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/utf8.md)),
and `instance_variables` of a value other than a plain Object (a Hash, an exception, a class,
a Regexp, …) lists the names in the order they were set, where the reference sorts them by
symbol number (`iv_put` in `src/variable.c`), an order that depends on the build's presym
table and on the order names are first interned. `sort` hands its block other pairs, in another
order, than the reference does, and of more than 16 elements leaves the ones that compare equal
in another order (`(0...17).map { |i| [i % 3, i] }.sort { |x, y| x[0] <=> y[0] }` begins
`[0, 0], [0, 9], [0, 3]` there, `[0, 0], [0, 3], [0, 6]` here): the reference sorts
by insertion up to 16 elements and by heap sort above (`mrb_ary_sort_bang` in `src/array.c`, not
stable), SabiRuby by merge sort (stable; Ruby promises neither). `sort_by` is stable on both. A
block that is no order (`[3, 1, 2].sort { |*x| x[0] }`, which answers an element rather than -1, 0
or 1) is given other pairs too, so the two put the same elements in other orders, or raise the
same exception.
The values `hash` answers are SabiRuby's own
(`1.hash` already differs from the reference's), and so is whether values of different
classes hash alike: in 4.1.0-rc2 a Struct and an Array with the same elements have the same
`hash` (both are `Enumerable#hash` of mrblib), here they do not (the author's decision of
2026-10-17: `Array#hash` keeps the function its Hash keys are looked up with).

Taken under the exception of the compatibility policy (a clear defect of the reference):

* **`a[i, len] = v` with a length that is not a number** raises TypeError
  (`nil cannot be converted to Integer`). In 4.1.0-rc2, `mrb_ary_aset` (`src/array.c`) reads
  the length with `aget_index`, whose fallback `mrb_get_args(mrb, "i*!", ...)` converts the
  method's first argument rather than the value it was given: the index is used as the length
  and the length is never looked at, so `a = [1]; a[0, nil] = 1` answers `[1, 1]` and
  `[1, 2, 3][1, nil] = 9` makes `[1, 9, 3]` (master 158760853 still does). A length that is an
  Integer past 64 bits takes the same fallback: `[3, 1, 2][1, 2**70] = ""` makes `[3, "", 2]`
  there, and on a frozen Array rc2 raises FrozenError where SabiRuby has converted the length
  first (RangeError "integer out of range"). CRuby raises TypeError for the others and
  RangeError for the wide length, as SabiRuby does. Inconsistent within mruby itself: `Array#[]`
  with the same arguments raises (`mrb_get_args(mrb, "oi")` converts the length), as does
  `String#[]=`.
  Test: `tests/custom/ary_aset_length_not_number.rb`.
* **A Hash entry taken away by the Ruby a lookup or a scan calls** raises RuntimeError
  (`hash modified`). When a key's `eql?` (or `==`, `inspect`, `hash`) deletes the entry being
  looked at and stores another, the size is back where it was and `H_CHECK_MODIFIED` sees
  nothing; 4.1.0-rc2's `src/hash.c` (`ar_get`, `ib_it_find_by_key`, `ht_set`,
  `ea_get_by_key`, `mrb_hash_assoc`/`rassoc`, `mrb_hash_equal`/`eql`, `mrb_hash_to_s`,
  `mrb_hash_except_keys`, `ar_rehash`/`ht_rehash`) then reads the vacated slot: `delete`
  answers the value and drops the size below the entries the table holds, `[]` answers a
  value the collector no longer keeps, `[]=` stores nothing. mruby fixed it on master as a
  read of freed memory (`entry_key_eql` and `entry_check_vacated`, a54353ecf, 08a0432d1,
  eb7693857), and SabiRuby takes those three commits' answer. CRuby compares hash codes before
  `eql?`, so the lookups do not call it in the same case; its scans raise "can't add a new key
  into hash during iteration". Test: `tests/custom/hash_eql_vacated.rb`.
* **A Float that stands for no Integer** (NaN, an infinity, a value past the Integer range) as
  an Array index (`a[Float::NAN]`, `a[Float::INFINITY, 1]`, `a[1e30] = 1`) or a `rand` limit
  (`rand(Float::NAN)`, `Random#rand(Float::INFINITY)`) raises RangeError ("float NaN out of
  range", "integer out of range"). 4.1.0-rc2 narrows it with the C cast `(mrb_int)f`
  (`aget_index` in `src/array.c`, `random_rand_impl` in mruby-random), which is undefined
  behaviour: on x86-64 `a[Float::NAN]` is nil, `a[Float::NAN] = 1` says "index
  -9223372036854775808 out of array" and `rand(Float::NAN)` draws some Integer (on AArch64 the
  cast reads a NaN as 0, says e2148c1b1). mruby fixed it on master as a bug (e2148c1b1, the checked conversion
  `mrb_as_int`/`mrb_ensure_integer_type`), and SabiRuby takes that answer. CRuby raises
  RangeError for the index and FloatDomainError for `rand`. Test:
  `tests/custom/float_index_out_of_range.rb`.
* **`Integer#to_s` with a radix that is not an Integer** converts it as every other Integer
  argument is converted (`mrb_as_int`): `10.to_s(1.5)` is "invalid radix 1", `10.to_s(nil)` a
  TypeError. 4.1.0-rc2's `int_to_s` (`src/numeric.c`) reads the argument with `mrb_integer()`
  without looking at its type, so it reads the bits of whatever it was given: nil, a String
  and a wide Integer (`10.to_s(2**70)`, `10.inspect(2**70)`) are "invalid radix 0", 1.5 "invalid radix 4602678819172646913", 2.5 a
  negative radix — an answer that depends on the value representation (master 891d1d9d is the
  same). CRuby says "invalid radix 1" for 1.5 and raises TypeError for nil. Test:
  `tests/custom/int_to_s_radix_type.rb`.
* **`__method__`, `super` and `defined?(super)` in a string given to `eval`** answer for the
  method the string is written in: `eval("__method__")` at the top level is nil, and
  `eval("super(x)")` calls the superclass method. In 4.1.0-rc2 the string's frame takes the
  name of the C frame it is pushed over, which is `eval` (`mrb_exec_irep()`, mruby-eval), so
  they answer `:eval`, look for the superclass method of `Kernel#eval`, and `defined?(super)`
  is "super" in a method that has none. mruby fixed it on master as a bug (b0b1de54e); CRuby
  answers as SabiRuby does. Test: `tests/custom/eval_method_super.rb`.

Kept by the author's decision (2026-10-17) where mruby is inconsistent within itself:

* **A frozen singleton class of an object that is not frozen** (`o.singleton_class.freeze`)
  refuses `def o.x`, `define_method` and `extend` with FrozenError. 4.1.0-rc2 lets the first
  two through — `mt_writable` (`src/class.c`) checks the object the singleton class is
  attached to rather than the class, a branch ccdf75c1c added to name the object in the
  message — while `extend` (`mrb_include_module`) checks the class and refuses. CRuby refuses
  all three. Test: `tests/custom/frozen_singleton_class.rb`.
* **`__method__` in a block run by `instance_exec` or `class_exec`** at the top level is nil,
  as it is for `instance_eval` and `class_eval`. 4.1.0-rc2 (and master 891d1d9d) answers
  `:instance_exec` and `:class_exec` there and nil for the `_eval` forms; answering the method
  name would change the shape of the block's frame. CRuby answers nil. Test:
  `tests/custom/instance_exec_method_name.rb`.
* **`Integer#infinite?`** (and that of a wide Integer and a Rational) answers nil, as a finite
  Float does. 4.1.0-rc2 answers false: `num_infinite_p` (`src/numeric.c`, the Numeric entry) is
  `return mrb_false_value()`, while `flo_infinite_p` answers nil for a finite Float — the same
  question answers false or nil for a finite number by its class (master 891d1d9d is the same).
  CRuby answers nil for all of them. A Complex reaches the same `num_infinite_p`, so rc2 answers
  false for `Complex(1, 2)` and for `Complex(Float::INFINITY, 2)` too, a silently wrong value
  for the second; SabiRuby answers nil for a finite Complex and 1 for one with an infinite
  part, as CRuby does (rc2-sweep-fixes Y6b). `Complex#finite?` follows it: rc2's
  `num_finite_p` answers true for every Complex, `Complex(Float::INFINITY, 2)` and
  `Complex(1.5, Float::NAN)` included, so `infinite?` and `finite?` would disagree; SabiRuby
  answers false when a part is not finite, as CRuby's `rect.all?(&:finite?)` does
  (rc2-sweep-fixes Y7). Test: `tests/custom/integer_infinite.rb`.
* **The "given" count of a wrong number of arguments when keywords are involved** is the number
  of arguments the method was given: `def f1(a)` called as `f1(1, k: 1)` is "given 2, expected
  1", and `def f5(a, k: 1)` called as `f5(k: 2)` is "given 0, expected 1". 4.1.0-rc2 counts
  "given" from the positional arguments plus one when there are none and the keyword Hash is
  not empty (`argnum_error`, `src/vm.c`), while the check it reports counted them another way,
  so four such calls read "given 1, expected 1". CRuby says the numbers SabiRuby does. Test:
  `tests/custom/argnum_given_kw.rb`.
* **A `def` after a string `class_eval` or `instance_eval`** lands where it would without the
  eval: `def f; K.class_eval "def hi; end"; def g; end; end` defines `g` on Object. In 4.1.0-rc2
  the string's proc shares the caller's env and `MRB_PROC_SET_TARGET_CLASS` writes the receiver
  into it, so `g` is defined on `K`. mruby fixed it on master as a bug (300cc9532, in no tag
  yet); CRuby answers as SabiRuby does. Test: `tests/custom/eval_class_eval_scope.rb`.
* **A top-level local named in an `eval` string inside a method** is an unknown name
  (NameError, `e.name` is `:x`), as any name the method cannot see. 4.1.0-rc2 fails to compile
  the string ("generator error, Can't find local variables", SyntaxError): the parser is told
  about every scope up to the C frame (`mrc_pm_options_init`, mruby-eval) while codegen's
  `search_upvar` stops at the method's scope, so the two disagree. CRuby raises NameError.
  Test: `tests/custom/eval_outer_scope.rb`.
* **`Array#fill` with a Range the array cannot take** fills from a start past the end
  (`[].fill(0, 1..3)` is `[nil, 0, 0, 0]`) and raises RangeError for a start before the array
  (`[].fill(0, -5..-1)`). 4.1.0-rc2's `ary_fill_parse_arg` (mruby-array-ext) tests
  `mrb_range_beg_len(...)` as a boolean, so MRB_RANGE_OUT passes and the start and length it
  never set are read: `[].fill(0, 1..3)` answered `[nil, nil, nil, 0, 0, 0, 0, 0, 0, 0]` or
  "array size too big", and a block is handed uninitialised values — undefined behaviour.
  SabiRuby takes CRuby's answer for these (the author's decision of 2026-10-18); a Range the
  array can take keeps rc2's answer (`[1, 2, 3].fill(0, 1..5)` is `[1, 0, 0]`, where CRuby
  extends the array). Test: `tests/custom/fill_empty_range.rb`.

Taken under the compatibility policy (2026-10-18) where mruby is inconsistent within itself, loses
data, or answers a silently wrong value or one that depends on the collector's state:

* **`define_singleton_method(name, a_proc)`** defines the method from the Proc, as
  `define_method` does. 4.1.0-rc2 declares it `MRB_ARGS_REQ(1)|MRB_ARGS_BLOCK()`
  (`mrbgems/mruby-metaprog/src/metaprog.c:701`; master 6db46e7c4 the same at `:743`), so the
  VM's argument check refuses the second argument, although the body (`define_method_m`,
  `src/class.c`) reads it with `"n|o&"` and `Module#define_method`, the same body, declares two.
  CRuby accepts it. Test: `tests/custom/define_singleton_method_proc.rb`.
* **Keyword arguments given to a native method that takes none** (`Array#push` among them) are
  handed on as one more positional argument: `[].push(1, k: 2)` is `[1, {k: 2}]`,
  `[1].first(k: 1)` and `1.to_s(k: 1)` are TypeError ("Hash cannot be converted to Integer").
  4.1.0-rc2 has no single rule: `mrb_get_args` (`get_args_v`, `src/class.c:1554-1577`) counts a
  non-empty keyword Hash as a positional argument (`"a".center(k: 1)` is TypeError there too),
  but `mrb_get_argc`/`mrb_get_argv` (`src/class.c:1249`, `1277`) count only the positional
  ones, and a method that reads them first drops the Hash without an error: `push` answers
  `[1]` (`mrb_ary_push_m`), `first` 1, `to_s` "1", `Struct#a(k: 1)` 1 (26 methods in the
  sweep, master 891d1d9d the same). Arguments disappearing by how a method happens to read them
  is inconsistent within mruby itself (the author's decisions of 2026-10-18). CRuby answers as
  SabiRuby does. Test: `tests/custom/push_keywords.rb`.
* **Too many arguments given through a splat** to `String#chomp`, `chomp!`, `to_i` and
  `Module#alias_method` are counted before they are converted: `"abc".chomp(*[nil, nil])` is
  ArgumentError, as `"abc".chomp(nil, nil)` is on both. 4.1.0-rc2 declares them MRB_ARGS_ANY and
  counts in `mrb_get_args`, whose fast path (`get_args_fast`, `src/class.c:1387`) gives up on
  arguments packed by a splat, and whose slow path converts each argument before it counts
  (`src/class.c:1913`): TypeError "nil cannot be converted to String". The same call answers
  two ways by how it is written. CRuby raises ArgumentError. Test: `tests/custom/argc_splat.rb`.
* **A frozen Set** refuses to change (FrozenError "can't modify frozen Set"). In 4.1.0-rc2
  mruby-set's mutators (`set_add`, `set_delete`, `set_clear`, `merge`, …,
  `mrbgems/mruby-set/src/set.c`) do not check, only `reset` and `flatten!` do, so
  `Set[1].freeze << 2` is `Set[1, 2]`; every other collection refuses. CRuby raises
  FrozenError. `replace` is mrblib's `clear` + `merge`, so `Set[1].freeze.replace(nil)` empties
  the Set on rc2 before `merge` raises ArgumentError "value must be enumerable" (CRuby raises
  that ArgumentError and keeps the Set); SabiRuby's `clear` refuses first, FrozenError, and the
  Set keeps its element (taking the argument's error first would mean a native `replace` in
  place of the gem's Ruby one). Test: `tests/custom/frozen_set.rb`.
* **The union of an empty Set and an Array** is answered: `Set.new | [1, 2]` is `Set[1, 2]`.
  4.1.0-rc2's `union` (`|`, `+`; mruby-set `mrblib/set.rb:117`) is `dup.merge(enum)` for an
  Array, and `dup` of an empty Set has no table (`set_init_copy`, `set.c:233`, copies with
  `kh_replace`, which leaves `data` NULL for an empty source), so `merge` raises RuntimeError
  "uninitialized Set", while `Set[0] | [1]` and `Set.new | Set[1]` answer: a refusal that
  depends on state, not on the input. CRuby answers the union. Test:
  `tests/custom/set_uninit.rb`.
* **`Integer#div` and `ceildiv` by a Float that is not whole** divide by the Float and floor:
  `7.div(1.5)` is 4, `(-3).div(1.5)` -2. 4.1.0-rc2's `int_idiv` (`src/numeric.c:299`) narrows
  the divisor with `mrb_as_int` first, so 1.5 becomes 1 and the answers are 7 and -3 (`ceildiv`
  is `-div(-other)` in mruby-numeric-ext). The same narrowing refuses NaN, Infinity and 2**70
  with RangeError, and SabiRuby refuses them too (mruby's limits are the specification), but
  here it answers a silently wrong value. CRuby answers 4 and -2. Test:
  `tests/custom/int_div_float.rb`.
* **An Integer past 64 bits in `quo`, `remainder` and `Rational()`** keeps the meaning it has
  at machine width: `0.quo(2**70)` is `(0/1)`, `(2**70).quo(3)` `(1180591620717411303424/3)`,
  `0.remainder(2**70)` 0. 4.1.0-rc2's `int_quo` (`src/numeric.c:329`) answers a Rational only
  for a machine-Integer divisor and a Float otherwise (`0.0`), and reads a wide receiver with
  `mrb_integer` without looking at its type (`(2**70).quo(3)` is `(0/1)`); `int_remainder`
  (mruby-numeric-ext) takes the Float road for a wide divisor (`0.0`); `rational_new_b`
  (mruby-rational) narrows a Float denominator with `mrb_as_bint`, so `Rational(2**70, 1.5)` is
  `(1180591620717411303424/1)`. SabiRuby answers that one as rc2 answers `Rational(4, 1.5)` (a
  Float division made a Rational, `(787061080478274158592/1)`), which is not CRuby's
  `(2361183241434822606848/3)` either (the author's decision of 2026-10-18). CRuby answers the
  others as SabiRuby does. Test: `tests/custom/bigint_float.rb`.
* **A copy of an exception** keeps its message and backtrace: `RuntimeError.new("a").dup.message`
  is "a", and `e.exception("b")` of a raised `e` keeps `e`'s backtrace. 4.1.0-rc2's `init_copy`
  (`src/class.c:4349`) copies an exception's instance variables only, not the `mesg` and
  `backtrace` of `struct RException`, so `dup` and `clone` answer the class name and nil, and
  `exc_exception` (`src/error.c:87`, a clone) loses the backtrace. CRuby answers as SabiRuby
  does. Test: `tests/custom/exc_copy.rb`.
* **SystemStackError** always says "stack level too deep". In 4.1.0-rc2 it is one object made
  in advance, whose message and backtrace the collector clears once a GC has run
  (`clear_error_object`, `src/gc.c:1484`), so after a GC the message is "SystemStackError": an
  answer that depends on the collector's state. CRuby says "stack level too deep". Test:
  `tests/custom/stack_error_gc.rb`.
* **A Time whose `initialize` fails when it is called again** keeps its value:
  `t = Time.at(0).utc; t.__send__(:initialize, 0, 0) rescue nil; t.inspect` is
  "1970-01-01 00:00:00 UTC". 4.1.0-rc2's `time_init` (mrbgems/mruby-time/src/time.c:1216) frees
  the Time's data and sets it to NULL (`:1228`) before `time_mktime` checks the values, so after
  its ArgumentError every call on the receiver raises ArgumentError "uninitialized Time": a
  failed call loses the receiver's value. CRuby keeps it. Test:
  `tests/custom/time_init_failed.rb`.
* **An Integer past 64 bits with a Float in `&`, `|`, `^`, `allbits?`, `anybits?`, `nobits?`**
  raises TypeError "can't convert Float into Integer", as a small Integer does on both. In
  4.1.0-rc2 a wide receiver takes mruby-bigint's road (`mrb_bint_and`/`or`/`xor`,
  mrbgems/mruby-bigint/core/bigint.c:6076-6124), which narrows the Float with `mrb_as_bint`
  (`:5517`), so `(2**70) & 1.5` is 0 and `(2**70) | 1.5` is 2**70 + 1: a silently wrong value that
  depends on the receiver's size. CRuby raises TypeError for `&`, `|`, `^` and answers false,
  false, true for the other three. Test: `tests/custom/bigint_bitop_float.rb`.
* **`Integer#pow(n, m)` with an operand past 64 bits** answers as for small ones: a Float modulus
  is TypeError "int.pow(n,m): m must be integer", and `(2**70).pow(0, 1)` is 0. 4.1.0-rc2's
  `int_powm` (mrbgems/mruby-numeric-ext/src/numeric_ext.c:199) refuses the Float and answers 0 for
  a modulus of 1 on the small road, but hands a wide receiver, exponent or modulus to
  `mrb_bint_powm` (mrbgems/mruby-bigint/core/bigint.c:5982), which narrows a Float modulus with
  `mrb_as_bint` (`(2**70).pow(2, 1.5)` and `7.pow(2**70, 1.5)` are 0) and answers 1 for x**0 mod
  1: silently wrong values that depend on the operands' size. CRuby refuses the Float (RangeError
  for a negative exponent, which it looks at first) and answers 0. Test:
  `tests/custom/bigint_pow_mod.rb`.
* **`const_get("")`** raises NameError "wrong constant name ", as `const_get(:"")` does on both.
  4.1.0-rc2's `mrb_mod_const_get` (`src/class.c:4023`) walks a String path segment by segment
  (`while (off < len)`, `:4039`), so for an empty String the loop never runs and the receiver
  itself is answered (`Object.const_get("")` is Object): the same question answers two ways by
  how the name is written. CRuby raises NameError. Test: `tests/custom/const_get_empty.rb`.
* **`private :m` (or `public`, `protected`) in a class with a prepended module** changes the
  class's own `m`: with `module P; def m; "P#m+" + super; end; end` prepended to a class `C`
  that defines `m`, `C.new.m` after `private :m` is "P#m+C#m". 4.1.0-rc2's `mrb_mod_visibility`
  (`src/class.c:2482`) looks the method up from the class (`mrb_method_search`, `:2523`), which
  finds the prepended `P#m` first, and writes that copy into the class's origin (`:2518`), the
  table that holds `C#m`: `C#m` is replaced by a private `P#m`, and its `super` raises
  NoMethodError "no superclass method 'm'". Changing a visibility silently replaces another
  method. CRuby answers as SabiRuby does. Test: `tests/custom/vis_prepended_copy.rb`.

Kept as the reference has it, though a defect: `Float#remainder` (and `Integer#remainder` by a
Float) of a large receiver computes `a - b * trunc(a / b)` in Floats on both, so
`(2**70).remainder(1.5)` is 0.0 where CRuby answers 1.0 (`%` is 1.0 on all three). Test:
`tests/custom/float_remainder_precision.rb`.

## Rules

* **`no_std` + `alloc`.** The library must not use `std::` (only `core::`/`alloc::`,
  `hashbrown` for hash maps, `libm` for float math). `tools/check_no_std.sh` builds the
  library for `thumbv7em-none-eabi` and greps for `std::`; CI runs it. The `std` feature
  (default) only adds `std::error::Error` for `VmError`. The C compiler and the command line
  tool are the separate crates `sabiruby-compiler` and `sabiruby-cli`.
* **Behaviour is checked against the reference, not against memory.** Every claim about
  mruby semantics is verified with the 4.1.0-rc2 binary (fixtures, test suite below).
* **Follow the reference; deviate only from a clear defect.** Where the reference's behaviour
  is clearly a defect (undefined behaviour, inconsistent within mruby, or fixed upstream as a
  bug), SabiRuby takes the evidently right behaviour and records it under Known deviations.
  Deliberate differences from CRuby are followed as they are
  ([`docs/design/compatibility.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/compatibility.md)).

## Verification

`tests/fixtures/*.rb` are compiled and run by the reference mruby 4.1.0-rc2
(Docker image `kishima/mruby:4.1.0-rc2`, see `tools/fixtures.sh`); `.out` holds the
reference stdout, `.dump` the `mrbc --verbose` listing. `cargo test` runs every `.mrb`
on SabiRuby and compares stdout byte for byte. All 18 fixtures pass (`gc.rb` also under `SABIRUBY_GC_STRESS=1`); `utf8.rb` is compared with
the image of the build's own reading (`kishima/mruby:4.1.0-rc2-utf8` by default, built by `tools/utf8-image/build.sh`).

mruby's own test suite (`test/t`) plus the tests of the ported gems (`gem_*`) passes 2346 of
2508 in the default build and 2269 of 2453 in a byte-string one — each build runs the
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
arithmetic and 16x on array-heavy code, and the whole set of 27 benchmarks is **2.28x** by
total time, 2.70x by the median of the per-benchmark ratios (`bench/results/release-0.8/ref-e7a3381.md`,
taken for 0.8.0; the numbers here are whatever that file's summary table says, not a promise). What changed and why is
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
sabiruby = "0.9"                                          # std + utf8 + regexp (the defaults)
# sabiruby = { version = "0.9", default-features = false } # no_std + alloc, byte strings, no Regexp
# sabiruby = { version = "0.9", features = ["macros"] }    # + #[derive(RubyClass)] / #[ruby_methods]

# optional companions
sabiruby-compiler = { version = "0.6", features = ["host"] }  # compile Ruby source in the same program
sabiruby-serde = "0.5"                                        # serde, and a JSON class
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
sabiruby --resume foo.ckpt     # go on with a program C mruby stopped and wrote out (mruby-checkpoint)
sabiruby compile foo.rb -o foo.mrb   # like mrbc (-g, -c, --remove-lv, --no-ext-ops, --no-optimize)
sabiruby dump foo.rb           # instruction listing (.rb or .mrb)
sabiruby mrbtest tests/mrbtest/assert.mrb tests/mrbtest/hash.mrb   # test suite
```

The program's output goes out as it is written, like the reference `mruby`: flushed at every
write on a terminal, block buffered elsewhere.

A subcommand name wins over a file of the same name: run `./compile` (or `sabiruby run compile`)
for a program file called `compile`. In this repository, use `cargo run -p sabiruby-cli -- …`.

```rust
let mut vm = sabiruby::Vm::with_mrblib()?;   // core library loaded
vm.load_and_run(&bytes)?;                   // run a RITE binary to completion
let out = vm.take_output();                 // what puts/p printed
// or have it as it is printed: vm.output_sink = Some(|bytes| { /* write them */ });

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
