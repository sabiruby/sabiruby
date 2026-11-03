# Changelog

What changed in each release of the crates in this repository: `sabiruby` (the VM),
`sabiruby-compiler`, `sabiruby-cli`, `sabiruby-macros` and `sabiruby-serde`. They share a
repository and a history but not a version; each entry says which crate it belongs to.

Every claim here is traceable to a commit or to a document under `docs/`, and the commit is
named. Measurements are the ones in [`docs/verification/bench.md`](docs/verification/bench.md);
nothing is estimated.

## 0.9.0 — (unreleased; the date is written when it is published)

To publish: `sabiruby` 0.9.0, `sabiruby-compiler` 0.6.0, `sabiruby-serde` 0.5.0, `sabiruby-cli` 0.9.0. `sabiruby-macros` stays at 0.1.1 (no change since `v0.8.0`, and it does not depend on the VM). The compiler and serde have no change of their own since `v0.8.0` and move a minor only because `sabiruby` is a public dependency of both (the reasoning of 0.6.0, `docs/design/compiler.md` "Publishing"). The plan is `docs/plans/release-0.9-plan.md`.

In short:

- **Checkpoint** (`sabiruby`, feature `checkpoint`): a program that C mruby stopped and wrote out goes on in SabiRuby from the instruction after where it stopped. The writer is the C mruby gem [mruby-checkpoint](https://github.com/family-mruby/mruby-checkpoint) (`Checkpoint.save(path)` writes and goes on, `Checkpoint.suspend(path)` writes and ends); the image is its format v1, a logical graph rather than a copy of C structures (objects by number, symbols by name, code by its contents and an FNV-1a 64 hash, what the writer had built in by name), the same from a 64-bit and a 32-bit writer but for one byte. SabiRuby rebuilds the objects, the frames and registers of every context (the root's, each Task's, each Fiber's), the scheduler with its queues, the remaining timeslice of the running Task and the line numbers the image carries, then runs on. One way only (C mruby to SabiRuby): `Checkpoint.save` and `suspend` write nothing here. What it cannot carry is refused with the reason (a `Kernel#catch` frame, a Proc made from a C function, a C data type it does not know, keyword arguments before `OP_ENTER`, a name SabiRuby has no counterpart for, …). The design, what is refused and the tests: [`docs/plans/checkpoint-plan.md`](docs/plans/checkpoint-plan.md) (in Japanese; `79b80dc` to `27b91ab`).
- **The feature is off in the library and on in the command.** Without `checkpoint` the VM crate has none of this code (`src/checkpoint/` and the few functions it adds to the VM and the task scheduler are behind the feature); it stays `no_std` with it on. `sabiruby-cli` turns it on by default, so `cargo install sabiruby-cli` gives `sabiruby --resume IMAGE` (and `--checkpoint-names FILE`, which says for each of the writer's built-in names what it is here); `--no-default-features --features "utf8 regexp"` builds the command without it. A program run from its start sees no `Checkpoint` module either way; the module is defined when an image is resumed (or by `Vm::checkpoint_install`).
- **The command writes the program's output as it is produced** (`041e096`). Before, it collected everything `puts`/`p`/`print` wrote and wrote it after the program ended, so a program that drew and then waited showed nothing until it finished. Now it behaves as the reference `mruby` (`src/print.c`): on a terminal every write is flushed at once, elsewhere the output is block buffered (64 KiB, `docs/numbers.md`) and flushed before an error report and at the end. The bytes and their order are unchanged; only when they appear changes. On a terminal the program's output now comes before the `--stats` lines, which are written to stderr after the run.
- **API, added only** (no change breaks a host): `Vm::output_sink: Option<fn(&[u8])>`, `None` by default — a place the host gives for the program's output, in the shape of `wall_clock` and `sleep_hook`; with a sink the bytes go there as they are written and `take_output` stays empty, without one they collect for `take_output` as before. With the feature `checkpoint`: `Vm::checkpoint_resume`, `Vm::checkpoint_install`, `Vm::checkpoint_name`, `Vm::checkpoint_header`, `checkpoint::image_counts`, `checkpoint::data_types`.
- **Checked**: `cargo test --workspace` 282 tests with the CLI's default features (the workspace has `checkpoint` on through the CLI), 276 with `checkpoint` off everywhere (the 0.8.0 count); the 95 checkpoint images in `tests/checkpoint/` resume to the writer's output or are refused with the recorded reason, also with `SABIRUBY_GC_STRESS=1`; `no_std` with and without the feature (`docs/worklog/2026-11-03-release-0.9.md`).

## 0.8.0 — 2026-10-19

Published: `sabiruby` 0.8.0, `sabiruby-compiler` 0.5.0, `sabiruby-serde` 0.4.0, `sabiruby-cli` 0.8.0. `sabiruby-macros` stays at 0.1.1 (one line of a test changed since `v0.7.0`, and it does not depend on the VM). The compiler and serde move a minor because `sabiruby` is a public dependency of both (the reasoning of 0.6.0, `docs/design/compiler.md` "Publishing"); serde's own source has not changed. The plan was `docs/plans/release-0.8-plan.md`.

In short:

- **The reference is mruby 4.1.0-rc2**, the stable branch (the author's decision of 2026-10-16). What had been matched to mruby master `891d1d9d` after the branch point is taken back out.
- **Every difference from 4.1.0-rc2 was looked for** (`docs/verification/rc2-sweep/`): 977,521 calls of every method both VMs have, on 86 receivers, and 502 Ruby programs. The survey found 68,583 calls that differ. After the fixes (rc2-sweep-fixes Y1–Y8) no cause to follow rc2 (A) or of wording only (B) is left, and every cause where rc2 is clearly wrong (C) keeps SabiRuby's answer and is recorded in README "Known deviations" with a test in `tests/custom`; the 32,495 calls that still differ are how each VM is built (hash values, addresses, the gems in the build; 16,151) or differences already recorded (16,344).
- **Three of those could stop the process or let a script do what rc2 refuses**: a Rust panic in `bytesplice`, `A::Y = 2` on a frozen class, `def` in a block run by `instance_eval` on an Integer (below).
- **API that breaks a host** (each `API:` line below): `HashData` lost nine methods and `object::HASH_INDEX_THRESHOLD` is gone; `InstanceKind` has five variants more (`Set`, `Istruct`, `Module`, `Class`, `Unset`; it is not `#[non_exhaustive]`); `HeapObject` has the fields `no_ivars` and `tt_struct`, `vm::CallInfo` the field `method_block`; `Method::Native` and `MethodRef::Native` carry an argument spec, `ClosureBody::arity` became `ClosureBody::aspec` and `Vm::native_arity` is gone; a host function's `Vec<T>` argument takes only an Array; a VM that never calls `load_mrblib` lacks 38 methods mrblib writes in Ruby. **Added**: `object::Aspec` and the `define_*_args` functions, `Host::compile_eval` and `EvalError`.
- **Speed**: from `11e595c` (inside rc2-sweep-fixes, before Y4) to the end of Y8 the 27 benchmarks got 7.7% faster in total; the speed work in that range is Y7 and Y8 (`OP_ADDILV`/`OP_SUBILV`, a visibility cache, `OP_STRING`; `docs/worklog/2026-10-18-rc2-sweep-fixes.md`, "Y8"). Against 0.7.0 (`v0.7.0` → `e7a3381`, interleaved, 3 rounds; `docs/verification/bench.md`, "0.8.0 candidate"): the 27 benchmarks −8.5% in total (A/A −0.2%/+0.2%), the 21 block-path micro-benchmarks −7.5%, the 7 task micro-benchmarks −6.4%; 54 of the 55 are faster and `vmo_objects` is within its A/A. Instruction counts are the same in all 55. The ratio to mruby 4.1.0-rc2 over the 27 is 2.28x by total time (2.70x by median), from 2.57x (2.85x) at `96221fb`.
- **Checked**: mruby's test suite 2346 of 2508 (default build), 2269 of 2453 (byte strings), 1981 of 2012 (without mruby-regexp), every file at or above its baseline; SabiRuby's own `tests/custom` 87 cases (85 with byte strings, 79 without mruby-regexp); `cargo test --workspace` 276 tests (`docs/worklog/2026-10-19-release-0.8.md`).

`sabiruby-cli` — `sabiruby edge-spec` runs one test file under mruby/edge's compatibility driver
(its `helpers.rb`), with `--with-fixtures` for SabiRuby's ports of the reference test driver's C
fixtures; it is what `tools/edge_compare.sh` calls (`7cf80d3`, `7323650`,
[`docs/verification/edge-compare/README.md`](docs/verification/edge-compare/README.md)).

`sabiruby` — SabiRuby keeps the behaviour of mruby 4.1.0-rc2, the stable branch (the author's
decision of 2026-10-16). What `edge-diff-fix` had matched to mruby `891d1d9d` and that changed on
master after the stable branch point is not in this release; the record of both is
[`docs/worklog/2026-10-16-follow-master.md`](docs/worklog/2026-10-16-follow-master.md).
Behaviour changes, each as 4.1.0-rc2 answers:

- A Hash compares a key with its entries in the reference's order: up to 16 entries with every
  entry and without asking the key's `hash`, past that with every live entry on the probe of the
  reference's index; a key's `hash` is asked again when the index is rebuilt. An `eql?` that adds
  or removes entries raises `RuntimeError` ("hash modified") wherever it is called, `assoc` and
  `rassoc` compare with `eql?` (the stored key answering), and `merge` with a block is
  mrblib's (`src/hashtable.rs`).
- A Set compares an element with every element while it has four or fewer, the stored one
  answering `eql?`, and a lookup whose `eql?` makes the Set rebuild its table raises
  `RuntimeError` ("hash table modified during callback").
- `0 % x` is 0 for any x but a big Integer; a zero Float divisor of `%` raises
  ZeroDivisionError.
- String interpolation sends `to_s` to everything but a String, a Symbol, an Integer and a class;
  an answer that is no String gives way to `#<Class:0x…>`; without `to_s` it is a TypeError.
- `Array#index`, `include?` and the other `==` searches send a `==` that Integer, Float, Symbol,
  nil, true or false had redefined; `sym == other` is false without a send.

Differences from 4.1.0-rc2 that were SabiRuby's own before any of the above, taken out the same
day ([`docs/worklog/2026-10-16-rc2-leftovers.md`](docs/worklog/2026-10-16-rc2-leftovers.md),
`tests/custom/rc2_leftovers.rb`):

- A constant is looked up from the running proc (`mrb_vm_const_get`): the class it was written
  in and each enclosing one, without their ancestors, then the ancestors of the innermost (a
  singleton class's, then those of the class it is attached to). A block run by `class_eval`,
  `instance_eval` or `define_method` reads constants where it was written, not from the receiver;
  `const_missing` is asked of the class that names the scope (a singleton class's attached class).
  `Foo::Bar` that misses asks `Foo.const_missing`. A block given to `define_method` also reads
  class variables where it was written; a string given to `class_eval` reads constants from the
  class given.
- `attr_reader`, `attr_writer`, `attr_accessor` and `attr` define public methods under `private`
  and `module_function` too, fire no `method_added`, answer nil, and reject a name with "'@x?' is
  not allowed as an instance variable name".
- `method_defined?` takes one argument (two is an ArgumentError) and is true for a private method.
- A class in a NameError's message is spelled as `Module#to_s` spells it (`#<Class:Foo>` for a
  singleton class), quoted only where the reference quotes it (`undef`, `undef_method`,
  `instance_method`), and `uninitialized constant Nope` for Object's.
- `Array#[]=`: "index -5 out of array" for one index, "index -2 is out of array" (the index after
  the length was added) for an index and a length, `RangeError` ("-10..1 out of range") for a
  Range that starts before the array; a Range that starts past the end pads with nil.

Four more found while doing those, taken out the same day
([`docs/worklog/2026-10-16-rc2-leftovers2.md`](docs/worklog/2026-10-16-rc2-leftovers2.md),
`tests/custom/rc2_leftovers2.rb`):

- `Foo::X` and `Foo.const_get(:X)` look in `Foo` and its ancestors but not in Object, so a class
  does not answer a top-level constant (`Foo::String` is a NameError; `Object::String` and a
  module's lookup are as before; `defined?(Foo::String)` still says "constant").
- `1::X`, `1::X = v` and a segment of `const_get("A::B")` that is not a class or module raise
  TypeError "constant look-up for non class/module"; `class 1::Foo` and `module 1::Foo` raise
  "1 is not a class/module".
- `Array#[]=` takes a Range with a Float end, truncated (`a[1.0..2.0] = v`).
- `Array#[]=` with an index or a length that is not a number says "nil cannot be converted to
  Integer" (the class for anything but nil, true and false).

The last of that kind, and one deviation recorded under the compatibility policy
([`docs/worklog/2026-10-16-rc2-leftovers3.md`](docs/worklog/2026-10-16-rc2-leftovers3.md),
`tests/custom/rc2_leftovers3.rb`, `tests/custom/ary_aset_length_not_number.rb`):

- `Array#[]` and `slice` take a Range with Float or Rational ends, truncated
  (`[1, 2, 3, 4, 5][1.0..2.0]` is `[2, 3]`), and a Rational index or length.
- `String#[]`, `slice`, `[]=` and `byteslice` read a Float index or length through
  `mrb_as_int`: truncated, and NaN or an infinity is a RangeError ("float NaN out of range")
  where it used to read as 0 or as the largest Integer.
- `Array#[]`, `String#[]`, both `slice!`, `String#[]=`, `byteslice` and every method that takes
  a Range of indexes (`values_at`, `fill`, `MatchData#values_at`, `Struct#values_at`,
  `caller`) say "nil cannot be converted to Integer" (the class for anything but nil, true and
  false) for an argument that is not a number, "float NaN out of range" for NaN and the
  infinities, and "integer out of range" for a Float past the Integer range.
- `Array#[]` with an index and a length converts the length first, as the reference does.
- Known deviation, recorded in README.md: `a[i, len] = v` with a length that is not a number
  raises TypeError, where 4.1.0-rc2 uses the index as the length (`a = [1]; a[0, nil] = 1` is
  `[1, 1]` there). This was already SabiRuby's behaviour; it is now recorded as a deviation.

A deviation taken under the compatibility policy, and the small differences left from the
above ([`docs/worklog/2026-10-16-rc2-leftovers4.md`](docs/worklog/2026-10-16-rc2-leftovers4.md),
`tests/custom/hash_eql_vacated.rb`, `tests/custom/rc2_leftovers4.rb`, `rc2_leftovers4_regexp.rb`):

- Known deviation, recorded in README.md: where a key's `eql?` (or `==`, `inspect`, `hash`)
  takes the Hash entry being looked at out of the Hash, `[]`, `[]=`, `delete`, `key?`, `assoc`,
  `rassoc`, `==`, `eql?`, `inspect`, `__except` and `rehash` raise `RuntimeError` ("hash
  modified"), as mruby master does since a54353ecf, 08a0432d1 and eb7693857. 4.1.0-rc2 reads
  the vacated slot (`delete` answered the value and left the size short).
- A FrozenError says "can't modify frozen Array", without the object's `inspect` (the reference's
  `mrb_frozen_error`); a method defined on the singleton class of a frozen object names the
  object's class, and `extend` on one names Class.
- `s["z"] = v` and `s[9..10] = v` out of the string say "string not matched".
- `Array#delete_at` and `MatchData#[]` read their indexes through `mrb_as_int` ("nil cannot be
  converted to Integer", RangeError "float NaN out of range").

Instance variables and `Integer()`, as 4.1.0-rc2 answers (the author's decision of 2026-10-17;
[`docs/worklog/2026-10-17-rc2-ivar.md`](docs/worklog/2026-10-17-rc2-ivar.md),
`tests/custom/ivar_holders.rb`, `tests/custom/kernel_integer_wide.rb`):

- **A script can raise where it did not**: only an object, a class or module, a Hash, an
  exception and a value of the reference's CDATA kind (a host's value, a task, Time, Method,
  Regexp, MatchData) hold instance variables (`obj_iv_p`). `@x = v`, `instance_variable_set` and
  an `attr_writer` on a String, an Array (a subclass of either too), a Range, a Proc, a big
  Integer, a Struct, a Data, a Set, a Random, a Rational, a Complex or a Fiber raise ArgumentError
  "cannot set instance variable", where they used to store the value; on an Integer, a Float, a
  Symbol, nil, true and false the error is that ArgumentError instead of FrozenError, and a frozen
  String says it too. `@x`, `instance_variable_get` and `attr_reader` there answer nil,
  `instance_variable_defined?` and `defined?(@x)` false and nil, `instance_variables` `[]`, and
  `remove_instance_variable` NameError. An `attr_writer` on a frozen object raises FrozenError,
  which it did not.
- `Integer()` takes a big Integer, a Rational and a Complex with a zero imaginary part (what was
  "can't convert Integer into Integer"), turns a Float past the Integer range into a big Integer
  and NaN or an infinity into RangeError "float NaN out of range"; `Float()` takes a big Integer,
  a Rational and a Complex. Anything else says "Object cannot be converted to Integer" (or
  Float). `Complex#to_i` and `to_f` name the value with `to_s` ("can't convert 2+1i into
  Integer").

API: `HeapObject` has a field more, `no_ivars`, and `HeapObject::iv_p`; `InstanceKind` has two
variants more, `Set` and `Istruct`. `Vm::ivar_set` and `Vm::ivar_get` still read and write any
object, as before.

The differences left from the above, as 4.1.0-rc2 answers
([`docs/worklog/2026-10-17-rc2-leftovers5.md`](docs/worklog/2026-10-17-rc2-leftovers5.md),
`tests/custom/masgn_not_array.rb`, `remove_ivar_name.rb`, `regexp_ivar_order.rb`):

- **A script can get other values than it did**: a multiple assignment from a value that is not
  an Array gives that value to the first target and nil to the rest (`a, b, c = obj` was obj
  three times; `to_ary` is not called, as before). A Struct or a Data is not an Array there and
  is not spread over a block's parameters: `a, b = Struct.new(:x, :y).new(1, 2)` is the struct
  and nil, `[s].each { |a, b| }` gets the struct and nil, `a, *b = s` the struct and `[]`.
- **A script can raise where it did not**: `Kernel#instance_variable_names`, which the reference
  does not have, is gone (NoMethodError); `instance_variables` is the method.
- `remove_instance_variable` checks the name first, as `instance_variable_get` does: `:x` raises
  NameError "'x' is not allowed as an instance variable name" (it said "instance variable x not
  defined"), and the NameError's `name` is set.
- A Regexp lists its instance variables as `[:@flags, :@source]`, not `[:@source, :@flags]`.

API: `HeapObject` has a field more, `tt_struct`, and `Heap::plain_array` (`mrb_array_p`).

The Array methods read an argument as an Array only when it is one in the reference's sense
(`mrb_array_p`), as 4.1.0-rc2 answers
([`docs/worklog/2026-10-17-rc2-leftovers6.md`](docs/worklog/2026-10-17-rc2-leftovers6.md),
`tests/custom/struct_not_array_arg.rb`):

- **A script can get other values than it did**: a Struct or a Data is not an Array to the
  Array methods. `[1, 2] == s` and `eql?` are false, `[3, 4] <=> s` is nil, `[s].flatten`,
  `[s, 3].join`, `a[0, 2] = s`, `puts s`, `[s].assoc(1)` and `rassoc` keep the struct whole,
  `Hash[[s]]` raises ArgumentError, and `Array(s)` is `s.to_a` (it was the struct).
- **A script can raise where it did not**: `[1] + s`, `concat`, `replace`, `-`, `|`, `&`,
  `union`, `difference`, `intersection`, `intersect?` and `product` raise TypeError for a
  Struct or a Data ("S cannot be converted to Array", `product`: "wrong argument type S
  (expected Array)"); `Array.new(s)` raises "S cannot be converted to Integer". The same
  methods no longer call `to_ary` on their argument, `replace` and `initialize_copy` with a
  value that is not an Array raise instead of emptying the receiver, `concat` takes exactly one
  Array, `Array.new(ary, x)` and `Array.new(ary) { }` raise (the size goes through
  `mrb_as_int`, so `Array.new(2.5)` is `[nil, nil]`), `Exception#set_backtrace` takes only an Array of Strings,
  and the TypeError says "X cannot be converted to Array" (it said "no implicit conversion into
  Array").
- A splat, `case ... when *v` and `Array()` use `to_a` for a Struct (same elements as before)
  and keep a Data whole, which has no `to_a`: `[*d]` is `[d]`, `when *d` compares with `d`. A
  `to_a` that answers something other than an Array raises "X cannot be converted to Array" in
  a splat and in `when *`, and `Array()` wraps the argument instead.

API: `HashData` no longer has `first_candidate`, `next_candidate`, `hash_at`, `hashes_stale`,
`push_entry`, `set_entries`, `set_entries_with_hashes`, `set_hashes` or `from_entries`, and
`object::HASH_INDEX_THRESHOLD` is gone (`hashtable::AR_MAX_SIZE` is the reference's switch);
`HashData` is `hashtable::HashData`, re-exported where it was. `Vm::hash_get`, `hash_set`,
`hash_delete` and `hash_index` keep their signatures. `vm::CallInfo` has a field more,
`method_block` (a block run as a method, whose constants are read from the block's class).

The small differences left after those, as 4.1.0-rc2 answers, and three deviations recorded
under the compatibility policy
([`docs/worklog/2026-10-17-rc2-leftovers7.md`](docs/worklog/2026-10-17-rc2-leftovers7.md),
`tests/custom/rc2_leftovers7.rb`, `rc2_leftovers7_regexp.rb`, `float_index_out_of_range.rb`,
`int_to_s_radix_type.rb`, `eval_method_super.rb`, `frozen_singleton_class.rb`,
`instance_exec_method_name.rb`):

- **A script can raise where it did not**: `raise`, `fail` and `Kernel.raise` take at most two
  arguments (a third, which was taken and not used, is an ArgumentError); `Array#pop` takes none
  (`pop(n)` is an ArgumentError); `private`, `public`, `protected` and `module_function` take
  Symbols only ("wrong argument type Array (expected Symbol)" for an Array, a String, an
  Integer, nil); `private_class_method` and `public_class_method` are gone (NoMethodError);
  `values_at` of an Array or a Struct raises TypeError "invalid values selector: x" for a
  selector that is neither an Integer nor a Range inside the receiver (1.5 and "a" included;
  a Range that starts before the receiver too, which was `[]`); `Integer.__ensure` (behind
  `Enumerable#first(n)`, `take`, `each_slice`, `each_cons`, `Range#first`) refuses an
  Integer past `i64` with "integer out of range"; `extend` on an object whose singleton class
  is frozen raises FrozenError.
- **A script can get other values than it did**: `private`, `public`, `protected` and
  `module_function` answer the module (they answered the names or nil); `Float#floor`, `ceil`
  and `truncate` take a digit count (`1.25.floor(1)` is 1.2, `15.5.floor(-1)` 10; the argument
  was ignored); `1 << 1.5` and `Array#*` with a Float count truncate it (they raised);
  `String#setbyte` answers the byte stored (`setbyte(0, 321)` is 65); `split(sep, 1)` answers
  the receiver itself (`"a b".split(" ", 1)` was `["a", "b"]`); `String#to_i(0)` reads a
  prefix (`"0x1f".to_i(0)` is 31; it raised); `[x].shuffle(random: 1)` and `[].shuffle` do not
  look at their arguments.
- Messages: an Integer argument that is not one says "nil cannot be converted to Integer" (the
  class for anything but nil, true and false) and RangeError "float NaN out of range" / "integer
  out of range" (`mrb_as_int`) everywhere, where most methods said "no implicit conversion of nil
  into Integer" and FloatDomainError "NaN"; a String argument that is not one says "Integer
  cannot be converted to String" where it named the argument ("separator cannot be converted to
  String"); `"a" + nil` says "nil", `Object.const_get(1)` "Integer cannot be converted to
  String"; `sort` without a block says "comparison failed" for a pair it cannot order (with a
  block, as before, "comparison of X with Y failed"); `String#to_i` says "illegal radix".
- Known deviation, recorded in README.md: a Float that stands for no Integer (NaN, an
  infinity, a value past the Integer range) as an Array index or a `rand` limit raises
  RangeError, as mruby master does since e2148c1b1; 4.1.0-rc2's C cast answers nil, "index
  -9223372036854775808 out of array" and an arbitrary Integer. `rand(1e30)` draws below a big
  Integer limit.
- Known deviations, recorded in README.md, that were SabiRuby's behaviour already:
  `Integer#to_s` with a radix that is not an Integer (4.1.0-rc2 reads its bits);
  `eval("__method__")` and `eval("super")` (master b0b1de54e); a frozen singleton class
  refuses `def`, and `__method__` in an `instance_exec` block is nil (the author's decisions).
  README also says that `hash` values, and whether a Struct and an Array hash alike, are
  SabiRuby's own.

API: a host function's `i64`/`i32` argument (`FromRuby`) says "nil cannot be converted to
Integer" and RangeError "float NaN out of range", as `Vm::expect_int` now does; `Vm::expect_str`
says "%Y cannot be converted to String" (its second argument is no longer in the message). A
host function's `Vec<T>` argument takes only an Array, as `mrb_get_args`' `A` does: a Struct or a
Data, which it used to read as its members, is a TypeError "S cannot be converted to Array" (it
said "no implicit conversion into Array"). 83 natives that a Ruby method of mrblib or another
native of the same name replaced while the VM starts are gone; what a script reaches after
`load_mrblib` is the same, while a VM made with `Vm::new` that never loads mrblib no longer has
the 38 of them mrblib writes in Ruby (`Array#sort`, `Hash#merge`, …;
`docs/worklog/2026-10-17-rc2-leftovers7.md`, T4).

Three differences found after those, as 4.1.0-rc2 answers, and one recorded
([`docs/worklog/2026-10-17-rc2-leftovers8.md`](docs/worklog/2026-10-17-rc2-leftovers8.md),
`tests/custom/rc2_leftovers8.rb`):

- `module_function` is undefined in Class (`mrb_init_class`): `Class.new { module_function }`
  is a NoMethodError; a module keeps it.
- `String#hex` and `String#oct` read the text with the scanner of `String#to_i`
  (`mrb_str_to_integer(self, 16 / 8, FALSE)`): an underscore right after the sign stops the
  number (`"+_ff".hex` is 0, it was 255; `"-_17".oct` 0, it was -15); both refuse an argument.
- The Float and Integer methods whose entry is `MRB_ARGS_NONE` refuse an argument
  ("wrong number of arguments (given 1, expected 0)"): `Float#to_i` (and SabiRuby's
  `Float#to_int`), `to_f`, `to_s`, `to_r`, `abs`, `-@`, `nan?`, `finite?`, `infinite?`;
  `Integer#to_i`, `to_int`, `to_f`, `to_r`, `-@`, `~`, `bit_length`, `size`, `even?`,
  `odd?`, and Numeric's `finite?` and `infinite?` (and SabiRuby's `Numeric#nan?`). They
  ignored it.
- Known deviation, recorded in README.md: `sort` hands its block other pairs than the reference,
  and of more than 16 elements leaves equal elements in another order (merge sort, stable, where
  the reference's heap sort is not).

What rc2-leftovers8 found, as 4.1.0-rc2 answers, and one recorded
([`docs/worklog/2026-10-17-rc2-leftovers9.md`](docs/worklog/2026-10-17-rc2-leftovers9.md),
`tests/custom/rc2_leftovers9.rb`, `tests/custom/integer_infinite.rb`):

- `sabiruby`: the methods of the four number classes that 4.1.0-rc2 does not have are gone, so
  a script that called them now gets a NoMethodError: `Float#to_int`, `Integer#pred`,
  `Numeric#nan?` (`1.nan?`, `Rational(1, 2).nan?`; `nan?` is Float's own, as `flo_nan_p`),
  `Numeric#to_int` (`Rational(1, 2).to_int`), `Numeric#step` (it only raised
  NotImplementedError), `Integer#__num_to_a` (Range's, which stays) and
  `Float#__coerce_step_counter` (Integer's, which stays). `respond_to?(:to_int)` of a Float is
  false.
- `sabiruby`: a method written in Ruby given the wrong number of arguments states only the
  required count, as `argnum_error(mrb, m1+m2)` of `OP_ENTER` does: `def f(a, b = nil)` called
  with three is "wrong number of arguments (given 3, expected 1)", it was "expected 1..2"; with a
  rest parameter "expected 1", it was "expected 1+". Lambdas and a block given as a lambda the
  same. The natives' messages (from their own argument checks) are unchanged.
- `sabiruby-cli`: an exception nothing caught is written to standard error as the reference
  `mruby` writes it (`mrb_print_error`): `trace (most recent call last):`, the backtrace from
  the outermost frame as `\t[n] file:line:in method`, then the innermost entry, ": " and the
  message with the class in parentheses — the class alone when the message is empty, and
  `(unknown):0: ` with no header when the backtrace is empty. It was the message and the class
  alone. The VM's `Vm::describe_error` is unchanged.
- Known deviation, recorded in README.md: `Integer#infinite?` (and that of a wide Integer and a
  Rational) stays nil, where 4.1.0-rc2's `num_infinite_p` answers false and its Float answers nil
  for a finite value — the author's decision for an inconsistency within mruby.

Native argument specs, and what rc2-leftovers9 left
([`docs/worklog/2026-10-17-rc2-leftovers10.md`](docs/worklog/2026-10-17-rc2-leftovers10.md),
`tests/custom/rc2_leftovers10.rb`, `tests/custom/argnum_given_kw.rb`):

- `sabiruby`: every native method carries the argument spec 4.1.0-rc2 declares for it
  (`MRB_ARGS_*`, collected from the reference's VM by `tools/rc2_aspec.sh`; a native rc2 does
  not have declares what its own count check said). The VM checks the number of arguments
  against it before the native runs, on every call by name (SEND, `send`, `Method#call`, a
  native's `funcall`), as `check_argument_count` does, and the 358 checks in the bodies that
  said the same are gone. A call rc2 refuses by its spec is now refused here too, e.g.
  `nil.to_s(1)`.
- `sabiruby`: `Method#arity` and `UnboundMethod#arity` of a native answer from its spec as
  `mrb_proc_arity` does; of the 1098 methods both have, none differs (612 did). An
  `attr_reader` answers 0 (`MRB_PROC_NOARG`). The hand-written table of 37 is gone.
- `sabiruby`: `define_method` and `define_singleton_method` store a strict copy of the block
  (`define_method_m` sets `MRB_PROC_STRICT`): the method checks its arguments as a lambda does,
  its `arity` is a lambda's, and the block itself stays a proc.
- `sabiruby`: `Integer.new`, `Float.new`, `Symbol.new`, `NilClass.new`, `TrueClass.new`,
  `FalseClass.new`, `Binding.new` and `Binding.allocate` are undefined on the singleton class, as
  `mrb_undef_class_method` does: "undefined method 'new' for Class", and `respond_to?(:new)` is
  false. It was "... for Integer".
- `sabiruby`, public API: `object::Aspec` (the spec, built like the C macros:
  `Aspec::req(1) | Aspec::opt(1)`); `Vm::define_method_args`, `define_methods_args`,
  `define_private_method_args`, `define_private_methods_args`, `define_module_function_args`
  and `define_closure_args` take one. The existing `define_method`, `define_methods`,
  `define_closure` and the others are unchanged in signature and declare nothing
  (`Aspec::any()`: no check, arity -1, as before). `Method::Native` and `MethodRef::Native`
  carry the spec as a second field; `ClosureBody::arity` is replaced by `ClosureBody::aspec`;
  the hidden `Vm::native_arity` table is removed. `Vm::define_fn` declares the count of its
  Rust signature, so the count is checked by the VM (the message is the same).
- Known deviation, recorded in README.md: the "given" count of a wrong number of arguments when
  keywords are involved stays the number the method was given (`f1(1, k: 1)` to `def f1(a)` is
  "given 2, expected 1"); 4.1.0-rc2 says "given 1, expected 1" in four such calls — the
  author's decision for an inconsistency within mruby.
- Known deviations, recorded in README.md (the author's decisions of 2026-10-18):
  `define_singleton_method(name, a_proc)` defines the method from the Proc, as `define_method`
  does and CRuby does (4.1.0-rc2 and master declare it `MRB_ARGS_REQ(1)|BLOCK` and refuse the
  Proc its body reads; `tests/custom/define_singleton_method_proc.rb`); `[].push(1, k: 2)` stays
  `[1, {k: 2}]`, where 4.1.0-rc2 drops the keyword Hash (`tests/custom/push_keywords.rb`).

The dangerous differences the rc2 sweep found, as 4.1.0-rc2 answers
([`docs/plans/rc2-sweep-fixes-plan.md`](docs/plans/rc2-sweep-fixes-plan.md) Y1,
[`docs/worklog/2026-10-18-rc2-sweep-fixes.md`](docs/worklog/2026-10-18-rc2-sweep-fixes.md),
`tests/custom/bytesplice_two_args.rb`, `setmcnst_frozen.rb`, `def_no_target_class.rb`):

- `sabiruby`: `"abc".bytesplice(0, 1)` (two arguments, the first no Range) no longer stops the
  process with a Rust panic: it raises TypeError "Integer cannot be converted to String", or
  ArgumentError "wrong number of arumgnts" (rc2's spelling) when the second is a String.
- `sabiruby`: **a script can raise where it did not**: `A::Y = 2` on a frozen class or module
  raises FrozenError "can't modify frozen Class" (or Module), as `class A; Y = 2; end` already did.
  It used to assign.
- `sabiruby`: **a script can raise where it did not**: `def`, `alias` and `undef` in a block run
  by `instance_eval` or `instance_exec` on an Integer, a Float or a Symbol (and in a block or
  `instance_eval` string made there) raise TypeError "no class/module to add method". They used to
  define the method on the class the block was written in, and `undef` took the method from it.
  A constant assigned or a `class` opened in a block made inside such a block, or in such a
  string, now goes to Object, as on rc2.

The rest of the rc2 sweep's "A — follow rc2" table, each as 4.1.0-rc2 answers unless said
otherwise ([`docs/plans/rc2-sweep-fixes-plan.md`](docs/plans/rc2-sweep-fixes-plan.md) Y2,
[`docs/worklog/2026-10-18-rc2-sweep-fixes.md`](docs/worklog/2026-10-18-rc2-sweep-fixes.md),
`tests/custom/rc2_sweep_y2.rb`, `rc2_sweep_y2_regexp.rb`, `fill_empty_range.rb`). All `sabiruby`:

- `String#bytesplice` reads its arguments by their count as rc2 does (2 as Range and String, 3 as
  Integer, Integer, String or Range, String, Range, 5 as Integer, Integer, String, Integer,
  Integer); any other count, or a Range that does not fit, is ArgumentError "wrong number of
  arumgnts" (rc2's spelling), where it was "wrong number of arguments (given 0, expected 2..5)".
- `initialize` sent again: `Range#initialize` checks the ends ("bad value for range") before
  "'initialize' called twice"; `Array#initialize` writes elements 0 to size-1 over what the Array
  holds and keeps the rest (`[3, 1, 2].__send__(:initialize)` stays `[3, 1, 2]`), and a negative
  size makes nothing: **`Array.new(-1)` is `[]`** (it raised ArgumentError "negative array
  size"); `String#initialize` with no argument empties the String; `Class#initialize` and
  `Module#initialize` answer the class or module and run a block in it (they answered nil); a
  frozen Array, Hash, String or Set refuses `initialize` (FrozenError, or "already initialized
  set") before reading the arguments.
- Argument counts: `instance_methods(a, b)` and `Module.constants(a, b)` are ArgumentError (they
  ignored the second); `[].sample(nil, nil)` converts the first argument before counting
  (TypeError); `Data#initialize` checks the keywords before the count; `Object#method_missing`
  takes a Symbol or String name (TypeError otherwise, ArgumentError with none) and gives the rest
  as the NoMethodError's `args`.
- The frozen check stands where rc2's does: `[1].freeze.delete(9)` answers nil (it raised
  FrozenError); `"abc".freeze.chomp!(nil)` is TypeError, as is `"abc".chomp(nil)`, which
  answered `"abc"`; `{}.freeze.merge!({}, nil)` is FrozenError; `a.freeze[0, -1] = x` is
  FrozenError before "negative length".
- `Hash#merge!` and `update` merge the hashes before a bad argument and then raise;
  `Hash#initialize_copy` is `replace` (TypeError for anything but a Hash; it accepted anything).
- `Fiber#inspect` shows where the fiber's block begins (`#<Fiber:0x… file.rb:3 (created)>`);
  `Binding#inspect` is `#<Binding:0x…>` (it listed the binding's internals).
- `Array#join`: an empty nested Array keeps its separators (`[[], 1].join(", ")` is `", 1"`), so
  `Enumerator#inspect` of such arguments too; a nested Array that contains an Array it is inside
  is "recursive array join".
- The order arguments are converted in: `Regexp#match` converts the position before a nil
  subject answers nil; `caller(a, n)` converts `n` first; `(2**70) ** (2**70)` is TypeError "too
  big power"; a NaN or an infinity as the operand of `&`, `|`, `^` with a wide Integer, as the
  modulus of `pow` or as the divisor of `Integer#div`/`ceildiv` (and a wide Integer as that
  divisor) is RangeError — the author's decision for `div`; a finite Float stays as it was
  (rc2 would truncate it).
- `public`, `private`, `protected` and `module_function` take the names one at a time (an
  undefined name before a bad one is the NameError).
- `Math` functions, `Float#remainder` and `Complex.polar` take a wide Integer, a Rational and a
  Complex with no imaginary part (`Math.cos(2**70)`; it was TypeError); `Math` answers NaN for a
  NaN where it raised Math::DomainError.
- `Set#initialize` sent a second time is RuntimeError "already initialized set"; `Proc#initialize_copy`
  is ArgumentError "not a proc" for anything but a Proc; `Time#to_s`/`inspect` write the year
  without padding (`-1-01-01`, `5-01-01`) and `asctime` with at least four digits (`-0001`).
- `Array#sum` is mruby-enum-ext's `Enumerable#sum` (the block is called; `[].method(:sum).owner`
  is Enumerable); a native `Array#sum` that ignored the block is gone.
- `Rational(1, 2) - ""` converts the operand to Float (TypeError "String cannot be converted to
  Float"); a zero remainder of `Float#%` and `Integer#%` with a Float is `0.0`, not `-0.0`;
  `"".split("abc", -1)` is `[]`; the space split keeps a last empty field after trailing spaces
  unless the limit is 0 (`"a b  ".split(nil, -1)` is `["a", "b", ""]`); `"abc".gsub(k: 1)`
  without a block keeps the Hash as the Enumerator's argument.
- `"a" + "b"` (`OP_ADD` with two Strings) joins them without looking `+` up, so a redefined
  `String#+` is not called there (it is through `send`).
- `Module#const_defined?(name, false)` looks only in the receiver's own table (it found Object's
  constants); with `true` it walks the superclass chain only.
- A constant assigned in a block run by `instance_eval`, `instance_exec` or `class_eval` goes to
  the class the block was written in (it went to the receiver's singleton class or the class
  evaluated in).
- `Array#fill` with a Range the array cannot take answers **CRuby's** `[].fill(0, 1..3)` →
  `[nil, 0, 0, 0]`, a start before the array RangeError (the author's decision; rc2 reads memory
  it never set there, and SabiRuby answered `[]`). The record of this deviation is plan Y5.

Methods defined where 4.1.0-rc2 defines them (the rc2 sweep's `method-owner`,
[`docs/plans/rc2-sweep-fixes-plan.md`](docs/plans/rc2-sweep-fixes-plan.md) Y3,
[`docs/worklog/2026-10-18-rc2-sweep-fixes.md`](docs/worklog/2026-10-18-rc2-sweep-fixes.md),
`tests/custom/method_owner.rb`). `method(:x).owner`, `instance_methods(false)` and where `super`
goes follow; no receiver lost a method. All `sabiruby`:

- Integer and Float define `<`, `<=`, `>`, `>=` and `<=>` themselves (Float also `finite?` and
  `infinite?`); Numeric's `<`, `<=`, `>`, `>=`, `==` and `between?` are Comparable's (mrblib
  compar.rb): **`1.method(:between?).owner` is Comparable**, `Rational(1, 2) < 1` and
  `Complex(1, 0) <= 2` go through `<=>`, and `between?` takes its arguments as `min`, `max`.
  `Numeric#eql?` is one method for every number (it was Integer's, Float's and Rational's);
  `Numeric#-@` is Numeric's (it was Integer's and Float's).
- `Kernel#<=>` answers nil when `<=>` of the same two objects is already running further out, so
  `Numeric.new <=> Numeric.new` is nil (it ended in SystemStackError once Numeric had no `<=>`).
- Array's `take`, `drop` and `hash` and Hash's `to_a` and `hash` are Enumerable's (Ruby; the
  hash of an Array or a Hash is mrblib's `Enumerable#hash`). Array's, Hash's, String's, Range's
  and Proc's `dup`, `freeze`, `frozen?` and `clone`, String's and Symbol's `===` and Symbol's
  `hash` are Kernel's: `dup` of an Array or a String is filled by its `initialize_copy`, so a
  subclass's `initialize_copy` is called (one without `super` gives an empty copy), and
  `freeze` freezes a singleton class with the object.
- **`Exception#==` is gone**: two exceptions are `==` only when they are the same object
  (`ArgumentError.new("a") == ArgumentError.new("a")` is false; it compared class and message).
  `Exception#exception(e)` given the receiver itself answers the receiver.
- `Module#==` and `Proc#initialize` are BasicObject's, `Module#initialize_copy` is Kernel's
  (`Module.new.send(:initialize_copy, String)` is TypeError; it copied String's tables),
  `Kernel#__id__` is BasicObject's only; `Module#dup` (a clone, not frozen) and `NilClass#nil?`
  are their own.
- `Module.new` and `Range.new` are `Class#new` (there were singleton methods): `MyMod.new` of a
  `class MyMod < Module` is a MyMod, `Module.new(1)` is ArgumentError, a Range subclass's
  `initialize` is called, and `Module.allocate` is a module (using it panicked).

API: `InstanceKind` has a variant more, `Module`.

Messages worded as 4.1.0-rc2 words them (the rc2 sweep's B causes,
[`docs/plans/rc2-sweep-fixes-plan.md`](docs/plans/rc2-sweep-fixes-plan.md) Y4,
[`docs/worklog/2026-10-18-rc2-sweep-fixes.md`](docs/worklog/2026-10-18-rc2-sweep-fixes.md),
`tests/custom/rc2_sweep_y4.rb`), by copying the reference function each message comes from.
`sabiruby` unless said:

- A value a number method cannot take: "String cannot be converted to Float" (and "can't convert
  nil into Float") wherever rc2 converts with `mrb_as_float` (`Integer#+`, `-`, `%`, `divmod`,
  `fdiv`, `quo`, every Float operator, `Math`, `format("%f")`, `Rational()`, `Complex`), "can't
  convert nil into Integer" for `Integer#*`, `/` and the bit operations of a small Integer
  (`mrb_int_noconv`), "nil cannot be converted to Integer" where it is `mrb_as_int` (`div`, `**`).
  It was "can't convert String into Float" or "String can't be coerced into Float". A Complex
  with no imaginary part is now taken where `mrb_as_float` takes it (`1.5 % Complex(2, 0)`,
  `Math.sqrt(Complex(4, 0))`, `1.div(Complex(2, 0))`). `Rational(1, 2) / nil` says "NilClass cannot
  be converted to Rational"; `Rational(1, 2) / 0` "divided by 0".
- `include`, `prepend` and `extend` of a value that is no module say "wrong argument type Hash
  (expected Module)", also for a class (`include String` named the class itself); `include?` of
  a class is that TypeError (it was false). `is_a?`, `kind_of?`, `instance_of?`, `include?` and
  `Class.new` of a value that is no class or module say "0 is not class/module" ("class or module
  required" before). **`Class.new(Class)` and `class X < Class` raise TypeError** "can't make
  subclass of Class" (they made one).
- `mrb_check_type`'s "wrong argument type X": a Float that the reference keeps in the value word
  is written out (`1.5`, `Infinity`) and one it keeps on the heap is "Float" (a NaN, `1e100`), as
  rc2's word boxing decides; `Hash#default_proc=` words it so too. `define_method` names the class
  ("NilClass").
- "unknown keyword: k" without the colon (natives, `OP_KEYEND`), `Data#initialize` "missing
  keyword: x" for the first member missing, before an unknown keyword; `Binding`'s name errors
  say "nil is not a symbol nor a string" and "wrong local variable name :\"\" for binding";
  `Struct.new("abc")` says "identifier abc needs to be constant"; `(2**70) ** nil` says "nil
  cannot be convert to integer" (sic, the reference's spelling); `Rational(1, 2) ** (2**70)`
  RangeError "integer out of range".
- An `eval` SyntaxError names the line rc2 names: that of the compiler's first error, counted
  within the string (`eval("1 +", nil, "f.rb", 10)` says "line 1"), and 0 for an error of the code
  generator. `sabiruby-compiler`: such an `eval` writes nothing to stderr (the error is in the
  SyntaxError only).
- **`Integer#fdiv` by zero** (0, 0.0, -0.0, or anything `mrb_as_float` makes 0) raises
  ZeroDivisionError (it answered Infinity or NaN); `Float#fdiv` still divides.
  `Rational(2**70, Float::NAN)` and the like (a wide Integer beside a NaN or an infinity) raise
  RangeError "float NaN out of range".
- **`Class.allocate`** is a class with no superclass whose `new` and `allocate` raise TypeError
  "allocation failure of #<Class:0x…>" (`Class.allocate.new` panicked).
- **An uninitialized Range** (`Range.allocate`) raises ArgumentError "uninitialized range" from
  every method that reads it and wherever it is given as a Range (`[1, 2][r]`, `rand(r)`,
  `(1..2).cover?(r)`, `r.dup`); `Range#==` is false for a Range of another class.
- `Exception#exception(msg)` copies the receiver with `clone` (its singleton methods and its
  frozen state go along) and takes a message that is not a String with `to_s`
  (`e.exception(nil).message` is "").
- `FromRuby for f64` (`Vm::define_fn`) takes a Rational and a real Complex too, and words its
  TypeError as rc2's `mrb_get_args("f")` does.

API: `InstanceKind` has two variants more, `Class` and `Unset`. `Host` has a method more,
`compile_eval`, answering `Result<Vec<u8>, EvalError>` (the new `EvalError { line, message }`): what
`eval` calls. Its default calls `compile` and names the line the string starts at, so a host that
implements only `compile` works as before; `sabiruby-compiler`'s `Compiler` implements it with
its first diagnostic's line.

Known deviations recorded in README.md, no behaviour change (the rc2 sweep's C causes,
[`docs/plans/rc2-sweep-fixes-plan.md`](docs/plans/rc2-sweep-fixes-plan.md) Y5,
[`docs/worklog/2026-10-18-rc2-sweep-fixes.md`](docs/worklog/2026-10-18-rc2-sweep-fixes.md)):
counting splatted arguments before converting them (`tests/custom/argc_splat.rb`), a frozen
Set (`frozen_set.rb`), the union of an empty Set (`set_uninit.rb`), `Integer#div` by a Float
that is not whole (`int_div_float.rb`), wide Integers in `quo`, `remainder` and `Rational()`
(`bigint_float.rb`), copies of an exception (`exc_copy.rb`), the message of SystemStackError
after a GC (`stack_error_gc.rb`), keyword arguments to natives that take none, `push` among them,
whose reason is rewritten (4.1.0-rc2 drops the Hash only in methods that read `mrb_get_argc`
first; `push_keywords.rb`), and three that were recorded only in tests: `Array#fill` with a Range
past the array (`fill_empty_range.rb`), a `def` after a string `class_eval`
(`eval_class_eval_scope.rb`) and a top-level local in an `eval` string inside a method
(`eval_outer_scope.rb`).

What the re-run of the sweep showed inside the line, fixed and recorded
([`docs/plans/rc2-sweep-fixes-plan.md`](docs/plans/rc2-sweep-fixes-plan.md) Y6b,
[`docs/worklog/2026-10-18-rc2-sweep-fixes.md`](docs/worklog/2026-10-18-rc2-sweep-fixes.md),
`tests/custom/rc2_sweep_y6b.rb`, `rc2_sweep_y6b_regexp.rb`). Behaviour changes, each as
4.1.0-rc2 answers:

- `Method#parameters` (and `UnboundMethod#parameters`) of a native method is read from its
  argument spec as `mrb_proc_parameters` reads a C function's: every required, optional and post
  argument is `:opt`, then `:rest`, `:key`, `:keyrest` and `:block`, without names
  (`1.method(:+).parameters` is `[[:opt]]`, `"".method(:center)` `[[:opt], [:opt]]`); an
  attribute reader or writer answers `[]`. It was made up from the arity (`[[:req]]`,
  `[[:req], [:rest]]`). `Class#initialize` is declared `MRB_ARGS_OPT(1)` and `Module#initialize`
  `MRB_ARGS_NONE()`, without the block flag.
- `Vm::alias_method` (so `alias_method` and `alias`) with the same name twice returns at once and
  fires `method_added`, even for a method the class does not have or a frozen class.
- `Module#undef_method` answers nil.
- `Array#shift(n)` with a negative `n` raises ArgumentError "negative array shift" (an empty
  Array answers `[]`); it answered the whole Array.
- `Math.log(x, base)` converts the base before it looks at `x`'s domain.
- `String#match` and `match?` (and `Regexp#match`) with a position past the last character
  answer nil and false.
- The root fiber is `created` until a switch comes back to it (`Fiber.current.inspect`).
- `Complex#infinite?` is 1 for a Complex with an infinite part and nil otherwise, as CRuby
  answers (4.1.0-rc2 answers false for every Complex; recorded with `Integer#infinite?`).
  `Complex#finite?` follows it: false when a part is not finite (an infinity or a NaN), as CRuby
  answers (4.1.0-rc2 answers true for every Complex; recorded with the same entry).

Known deviations recorded in README.md, no behaviour change: a Time whose `initialize` fails
keeps its value (`time_init_failed.rb`), a wide Integer with a Float in the bit operations
(`bigint_bitop_float.rb`), `Integer#pow(n, m)` with a wide operand (`bigint_pow_mod.rb`),
`const_get("")` (`const_get_empty.rb`), and, with the frozen Set, `replace` with a value that is
no enumerable (`frozen_set.rb`).

Recorded in README.md for 0.8.0, no behaviour change (the author's decisions of 2026-10-19,
[`docs/plans/release-0.8-plan.md`](docs/plans/release-0.8-plan.md) R1): `private :m` in a class
with a prepended module changes the class's own `m`, where 4.1.0-rc2 replaces it with a private
copy of the module's (a known deviation, `vis_prepended_copy.rb`); `Float#remainder` of a large
receiver loses the remainder on both, `(2**70).remainder(1.5)` is 0.0 (a defect kept as rc2 has
it, CRuby answers 1.0; `float_remainder_precision.rb` holds the answer).

## 0.7.0 — 2026-09-27

Published: `sabiruby` 0.7.0, `sabiruby-compiler` 0.4.0, `sabiruby-serde` 0.3.0, `sabiruby-cli` 0.7.0. `sabiruby-macros` stays at 0.1.1 (no change since `v0.6.1`, and it does not depend on the VM). The compiler and serde move a minor because `sabiruby` is a public dependency of both (the reasoning of 0.6.0, `docs/design/compiler.md` "Publishing"). The plan was `docs/plans/release-0.7-plan.md`.

`sabiruby` — a task can wait inside more blocks, and a wait that cannot happen says where
([`docs/design/wait-anywhere.md`](docs/design/wait-anywhere.md),
[`docs/worklog/2026-09-26-wait-anywhere.md`](docs/worklog/2026-09-26-wait-anywhere.md);
`9eaf698`, `da68a3a`). Behaviour changes:

- `Task::Queue#pop`, `sleep`, `sleep(n)` and `Task.pass` work inside `instance_exec`,
  `instance_eval`, `class_exec`, `class_eval`, `module_eval`, `Method#call`, `bind_call`,
  `public_send`, the `initialize` `Class#new` calls, `eval("…")` and `instance_eval("…")`, as in
  mruby 4.1.0-rc2 (`9eaf698`), and inside `index { }`, `rindex { }`, `Array.new(n) { }`,
  `delete(x) { }`, a Hash's default proc, `Hash#default(k)`, `Class.new { }`,
  `Module.new { }`, `Struct.new { }`, `Data.define { }`, `catch { }` and `Regexp#match { }`,
  where mruby cannot (`da68a3a`, `33cc076`, `f3c7d9c`).
- `sort { }` stays a native boundary (the author's decision, 2026-09-26: waiting there cost a light
  block 8%). `sort` without a block on an Array of Integers sorts them in Rust directly.
- `sleep(n)`, `usleep` and `sleep_ms` in a task inside a native boundary raise `RuntimeError`;
  they returned at once without waiting (`9eaf698`).
- The error of a wait inside a native boundary names it: `can't wait inside Array#join's call to
  #to_s (Task::Queue#pop)`. It was `blocking pop cannot be called from within a C function
  boundary`, `can't sleep across C function boundary` or `can't switch task across C function
  boundary` (`9eaf698`).
- A `break` out of the blocks above leaves the method that took the block
  (`[1, 2].index { break :b }` is `:b`, was `0`; `Class.new { break 1 }` is `1`, was the class;
  `Foo.new { break 1 }` with an `initialize` that yields is `1`, was the object).
- Recursion through `instance_exec` and its relatives stops at 512 frames
  (`SystemStackError`), not at 96 nested loops.
- `Vm::catch_tags` (`#[doc(hidden)]`) is gone: `catch` runs its block from a frame of its own
  and `throw` finds that frame on the frame stack (`33cc076`).

Speed ([`docs/verification/bench.md`](docs/verification/bench.md), "Waiting inside blocks"): the 27
benchmarks together +0.9% and −0.7% in two rounds; a Hash's default proc −25.6%, a Hash miss −20.9%, `index { }` −13.2%, `instance_exec` −8.6%.

`sabiruby` — fixes gathered for 0.7.0 ([`docs/plans/release-0.7-plan.md`](docs/plans/release-0.7-plan.md),
[`docs/worklog/2026-09-26-release-0.7-a.md`](docs/worklog/2026-09-26-release-0.7-a.md)):

- **API:** `Vm::hash_get` and `Vm::hash_delete` return `VmResult<Option<Value>>` (were
  `Option<Value>`). A lookup can run Ruby — the key's `hash` and `eql?` — and what that raised was
  dropped. A host that wants the old answer writes `vm.hash_get(h, k).ok().flatten()`; one that
  passes the error on writes `vm.hash_get(h, k)?` (`3ad6fdd`).
- `Hash#[]`, `key?`/`has_key?`/`include?`/`member?`, `fetch`, `delete`, `dig`, `values_at`,
  `slice`, `except`, `==` and `eql?` raise what the key's `eql?` or `hash` raises; they answered
  nil/false, as if the key were missing (`3ad6fdd`). One difference from the reference is left: a Hash of up to 16 entries there never
  calls `hash` (its small form compares with `eql?` only), so a key whose `hash` raises answers
  nil there and raises here, as it does in both past 16 entries.
- A lookup whose `eql?` adds or removes entries of the Hash it is searching raises `RuntimeError`
  ("hash modified"), as mruby 4.1.0-rc2 does (`3ad6fdd`).
- A `method_missing` defined with `Vm::define_closure`, reached through `Vm::funcall` from a native
  called by Ruby, can no longer `Vm::fiber_yield` from under that native: it raises `FiberError`
  ("can't cross C function boundary"), as in mruby (`f01d5a9`).
- A RITE image with an irep record that has more locals than registers is refused at load
  (`VmError::Rite`), as mruby 4.1.0-rc2 does; it made the VM panic (`66099fd`).
- `OP_CALL`/`OP_BLKCALL` on a receiver that is not a Proc say `wrong type Integer (expected Proc)`;
  they said `wrong type (expected Proc)` (`66099fd`).
- A Hash or Set key that is the same object as a stored one matches it without `eql?` being
  asked, as `mrb_eql` answers identity first: a key whose `eql?` returns false (or raises) finds
  itself again (`c3f0e85`).
- A `Set` looks elements up as mruby-set does: an element's `hash` that raises counts as 0, and
  `eql?` is asked of the element already in the set, so `Set["k0"].add(x)` adds an `x` whose
  `eql?` raises; an `eql?` that does run and raises is raised (`c3f0e85`).
- `p` of a `Set` or a `Struct` prints its contents; it printed `Set[...]` / `#<struct S ...>`
  (`c3f0e85`).
- A block that is not a Proc handed to `Vm::call_block` and its relatives raises `TypeError`
  "not a block", as mruby's `check_block`; it said `wrong type (expected Proc)` (`a9a4740`).
- `Vm::call_block` and `exec_block` with a nil block raise `ArgumentError` "no block given", as
  mruby's `check_block`; they raised `LocalJumpError` "no block given (yield)" (`39be934`).
- `Array#index` and `#rindex` with neither an argument nor a block answer an Enumerator
  (`to_enum`), as mruby's `mrb_ary_index_m` (`39be934`).
- The hash of a Struct, or of any object whose class includes `Enumerable`, raised
  `NoMethodError` (`__method_recursive?`); `Kernel#__method_recursive?` and
  `Enumerable.__update_hash` are there now, as in mruby (`39be934`).
- A closure `method_missing` reached by a SEND sees `Vm::native_mid` as `:method_missing`; it
  saw the name of the native called before it (`91ca672`).
- Errors worded as mruby's where a block or a string was missing or wrong: `instance_eval` /
  `class_eval` with nothing give the argument count (`given 0, expected 1..3`), a non-String
  source to them or to `eval` is "Integer cannot be converted to String", `catch` without a block
  says "for NilClass", and `define_method`, `define_singleton_method` and `Proc.new` without a
  block say "no block given" (`da4684c`).

`sabiruby` — for hosts that run many scripts and replace them
([`docs/plans/release-0.7-plan.md`](docs/plans/release-0.7-plan.md) S4–S6,
[`docs/worklog/2026-09-26-release-0.7-b.md`](docs/worklog/2026-09-26-release-0.7-b.md)):

- `Exception#backtrace` names the natives on the way to the raise, placed at the nearest Ruby frame
  below them, as mruby names its C functions: `a.rb:2:in Integer`, `a.rb:4:in sort!`,
  `a.rb:5:in index`; a native with no frame that has debug information below it is `(unknown):0`.
  `caller` sees the same natives (`Vm::backtrace_record`, new). A native written by the host
  (`define_fn`, `define_closure`) is named too, so the first line of an exception it raised is now
  `data.rb:2:in unit` where it was `data.rb:2`. Where mruby rewrites a C function's frame into a
  block (`instance_exec`, `eval`, `Class.new { }`), SabiRuby still differs
  (`tests/custom/native_backtrace.rb`) (`1f933e9`).
- A task that is only waiting costs nothing per turn of the others: the scheduler's queues are
  ordered maps and the waiting tasks are indexed by deadline and by what they wait for. A push and
  a tick look at the one task they wake with 100 or 1000 tasks waiting (they looked at 406 and
  4006 elements). The order tasks run in is unchanged. `TaskState::queues` is now
  `[TaskQueue; 4]` (was `[Vec<ObjId>; 4]`) and `ObjId` is `Ord` (`1c3161e`).
- `Vm::unload(irep)` hands back the ireps of a program `Vm::load` read, once nothing can run them
  (no frame stands in them, no Proc is made of them); `UnloadError::StillInUse` otherwise. The
  numbers are not given out again, so an id a host kept can never run another program:
  `run_irep` and `task_spawn` refuse an unloaded one. Each unloaded irep leaves an empty entry of
  208 bytes behind; that is the accepted price (`docs/design/gc.md`, "Programs"). Programs read
  by `load_and_run`, `eval` and `binding` are not handed back (`docs/backlog.md`).

## 0.6.1 — 2026-09-22

Documentation only: no `.rs` file changed (`git diff v0.6.0 -- '*.rs'` is empty). The author
looked at crates.io after 0.6.0 went up and the five pages described an older repository, so
every `description` and every crate README was walked claim by claim against the tree
([`docs/worklog/2026-09-22-readmes.md`](docs/worklog/2026-09-22-readmes.md) has the table).
All five crates move a patch, because a crate's page on crates.io is only rebuilt by a release.

| crate | | | what was wrong |
|---|---|---|---|
| `sabiruby` | 0.6.0 | **0.6.1** | README headed "Status (2026-09-17, 0.5.0)"; nothing about the host bridge (`define_fn`, `HostStore`, `Data`), `next_line`/`backtrace_line` or `highlight()`; benchmark ratios two measurements old; a build time that no longer held |
| `sabiruby-serde` | 0.2.0 | **0.2.1** | neither the `description` nor the first line of the README said `declare`, which is what 0.2.0 was for |
| `sabiruby-compiler` | 0.3.0 | **0.3.1** | the `description` did not say `highlight()`; the golden-test counts said 17 fixtures and 61 test files where the repository has 18 and 112 |
| `sabiruby-cli` | 0.6.0 | **0.6.1** | "`-r` (require) is not implemented yet" — it has been since `37d4a31`; `mrbtest` was missing from the listing |
| `sabiruby-macros` | 0.1.0 | **0.1.1** | the one `Cargo.toml` line it shows said `sabiruby = { version = "0.5", … }`; `no-std` dropped from its keywords and categories (the proc macro itself is built for the host) |

Three things were changed on purpose rather than corrected, so that the pages go stale more
slowly: the version and the date are out of the `Status` heading (they are in this file), the
per-benchmark ratios name the results file they came from, and the seconds a clean build of the
C compiler takes are gone rather than re-measured, because that number is the machine's, not the
crate's. Each crate's README now links this file by absolute URL, since only `sabiruby` ships it.

## 0.6.0 — 2026-09-22

`sabiruby-serde` — a module for data *written* in Ruby, and the two things writing a game's
data stage on it turned up. The VM gains two read-only entry points, loses the name one of them
replaced, and changes no behaviour
([`docs/design/serde.md`](docs/design/serde.md) "Declarations",
[`docs/worklog/2026-09-20-declare.md`](docs/worklog/2026-09-20-declare.md),
[`docs/worklog/2026-09-21-serde-lines.md`](docs/worklog/2026-09-21-serde-lines.md),
[`docs/worklog/2026-09-22-release-0.6.md`](docs/worklog/2026-09-22-release-0.6.md)).

Four of the five crates are published; `sabiruby-macros` is not.

| crate | | | why |
|---|---|---|---|
| `sabiruby` | 0.5.2 | **0.6.0** | `Vm::current_line` removed (`5b4c0a5`), `next_line` and `backtrace_line` added (`cd013fa`, `d0bbde0`) |
| `sabiruby-serde` | 0.1.0 | **0.2.0** | `Options` is `#[non_exhaustive]` (`491ec0e`); `declare` (`2f1043f`, `d0bbde0`) |
| `sabiruby-compiler` | 0.2.3 | **0.3.0** | source unchanged; its optional `sabiruby` is a public dependency and now names 0.6 |
| `sabiruby-cli` | 0.5.0 | **0.6.0** | source unchanged; the release's name, and it names both of the above |
| `sabiruby-macros` | 0.1.0 | 0.1.0 | unchanged since `v0.5.1`, and it does not depend on the VM |

### Coming from 0.5.2

In the order a host meets them:

1. **`Vm::current_line` is gone.** Replace it with `Vm::next_line` — same answer, same type,
   nothing else to do. If what you wanted was the line an error names, the line a native was
   called *from*, that is the new `Vm::backtrace_line`, which is one line earlier.
   The deprecated alias `current_line` carried the rename for a day and was removed on
   2026-09-22 (`5b4c0a5`); 0.5.2 never had `next_line`, so there is no version of the crate
   where both names work.
2. **`sabiruby_serde::Options` can no longer be built with a struct literal.** It is
   `#[non_exhaustive]`, so `Options { symbol_keys: true }` does not compile outside the crate.
   Write `Options::symbol_keys()`, `Options::symbols()` or `Options::default()` and assign to
   the public fields. From here on a field added to `Options` is not a breaking change.
3. **`declare::expose` writes a map's keys as Symbols.** A published table now answers
   `recipe_of(:iron_plate)[:in][:iron_ore]` where it would have answered `[:in]["iron_ore"]`.
   Ruby that indexed an exposed map with a String has to be changed. `to_value` and
   `to_value_with` are unaffected unless you ask for it: the switch is
   `Options::symbol_map_keys`, off by default, and `Options::symbols()` is the pair.
4. **Version requirements.** `sabiruby = "0.6"`, `sabiruby-serde = "0.2"`,
   `sabiruby-compiler = "0.3"`. A caret requirement inside `0.x` reads the minor as the major,
   so `"0.5"` does not accept 0.6.0 and the four have to move together — a program that raises
   only one of them ends up with two `sabiruby` crates and types that are not each other's.
   For the same reason, a `[patch.crates-io]` whose patched crate still says `version = "0.5.x"`
   stops being used at all rather than failing loudly.

### `sabiruby-serde` 0.1.0 → 0.2.0

* **`sabiruby_serde::declare`** (`2f1043f`, `c38f0e2`) — `Declarations<T>` grows a method on the VM
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

### `sabiruby` 0.5.2 → 0.6.0

Two entry points, one rename and the removal of the name it replaced, no behaviour changed
(`cd013fa`, `d0bbde0`, `5b4c0a5`).

* **`Vm::backtrace_line() -> Option<u32>`** — the line the first frame of `Vm::backtrace`
  carries, which from inside a native (which pushes no frame of its own) is the line of the
  call. Distinct from `Vm::next_line`, which answers with the line of the instruction that
  will run **next**, since `ci.pc` is past the instruction being executed: that is what a
  debugger stopped at an instruction boundary wants to highlight, and it is one line late for
  anything asking where it was called from (`tests/native.rs`).

* **`Vm::current_line` is renamed `Vm::next_line`; the old name is gone** — a breaking change,
  which is why this release is **0.6.0** and not a patch on 0.5.2. Nothing about what the
  VM computes changed — the name did, because it invited the wrong reading: asked from inside
  a native it looks like it will say where the native was called from, and it says the line
  after. `next_line` says what it answers, and `backtrace_line` is the other question.

  What to write instead is "Coming from 0.5.2" above, item 1.

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
