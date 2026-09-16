# Ported gems

How a gem of the reference tree becomes part of SabiRuby, and what each ported one taught.

## Recipe

1. **Natives**: the gem's `src/*.c` methods go into `src/builtins/ext_<gem>.rs` (or
   `fiber.rs`), registered after the core natives in `builtins::init` so they replace core
   natives of the same name, as the gem's `mrb_..._gem_init` does in the reference.
2. **Ruby part**: `tools/mrbtest.sh` concatenates the gem's `mrblib/*.rb`, compiles it with the
   reference `mrbc` into `src/mrblib/<gem>.mrb`, and `Vm::with_mrblib` loads it after the core
   mrblib **in the order of `mrbgems/default.gembox`** (`lib.rs`, `vm.rs`). The order matters
   when two gems define the same method: mruby-enumerator's `Enumerable#zip` replaces
   mruby-enum-ext's because it is initialised later.
3. **Tests**: add the gem to `GEMS` in `tools/mrbtest.sh`; its `test/*.rb` become
   `tests/mrbtest/gem_<file>.mrb` and run like the core files. C test helpers (`test/*.c`)
   are re-implemented in `src/mrbtest.rs`.
4. **Core natives that only exist because of a gem must go.** SabiRuby's core stage had
   `Array#zip`, `Kernel#to_enum` (a NotImplementedError stub), `Range#count`, `Hash#count`
   as natives; in the reference those are gem mrblib Ruby methods (or `Enumerable`'s). A
   native on the class shadows the Ruby definition inherited through `Enumerable`, so the
   gem's method never runs. Rule: before writing a native, check whether the reference
   defines the method in C at all.

## What each gem needed

* **mruby-fiber** — see `docs/design/fibers.md`.
* **mruby-enumerator** — pure Ruby; needed the `send`/`__send__` in-place dispatch,
  `initialize_copy` on `dup`/`clone`, and the removal of the `to_enum` stub.
* **mruby-array-ext** — 34 natives (`ext_array.rs`). The set operations (`-`, `|`, `&`,
  `difference`, `union`, `intersection`, `intersect?`, `__uniq`) compare with `eql?`, not
  `==` (the reference uses a khash set keyed by `hash`/`eql?`; here a linear walk with
  `eql?`). `__combination_init/next` and `__product_generate/next` keep their state in a
  plain Array instead of an RData. `insert` and `__fill_exec` refuse sizes above
  `ARY_MAX_SIZE` (2^28 here) with the reference's "array size too big".
* **mruby-enum-ext** — `__minmax` and `__count` on Array. `Kernel#<=>` (`mrb_obj_cmp`:
  0 for the same object or `==`, else nil) was missing and is needed by `min_by`/`max_by`.
  `OP_EQ` must answer true for the same object *before* sending `==` (`mrb_obj_eq`);
  SabiRuby only did that for immediates, which broke `count(obj)` on an object whose `==`
  is always false.
* **mruby-hash-ext** — `values_at`, `slice`, `slice!`, `except`, `key`, `__merge`,
  `Hash.[]`, plus the core `__compact` the gem's Ruby `compact!` calls.
* **mruby-range-ext** — `cover?` (range-in-range rules), `size` (Float-aware, with the
  reference's epsilon), `__empty_range?`. Its Ruby `first`/`last`/`min`/`max` replace the
  core natives. Note: with this gem loaded, `(1..).last` raises RangeError, and the core
  test `Range#last` (`assert_nil (1..).last`) fails on the reference `mruby` as well.
* **mruby-string-ext** — 56 natives (`ext_string.rs`). What a position means depends on how
  the string is read (`docs/design/utf8.md`); what follows holds in both builds. The
  `tr`/`delete`/`squeeze`/`count` pattern parser is ported byte for byte, including two
  quirks of the reference: the bitmap used by `delete`/`squeeze`/`count` treats a range
  `a-c` as *exclusive* of `c` (`tr_compile_pattern` loops `i < ch[1]`), while `tr` itself
  is inclusive; and replacement bytes are read as signed `char`, so a byte ≥ 0x80 in the
  replacement deletes. `String#succ` follows `str_succ_bang`: the rightmost alphanumeric
  steps and carries across non-alphanumerics, but a letter never carries into a digit nor a
  digit into a letter (`"1-z".succ == "1-aa"`, `"1.9".succ == "2.0"`); above ASCII the runs of
  letters and digits come from `str_alnum.h`, ported as `src/builtins/str_alnum.rs`, so `"ת".succ`
  is `"אא"`. `Integer#chr` accepts `ASCII-8BIT`/`BINARY`, and `UTF-8` only in a build that has
  the encoding (the feature `utf8`), as the reference accepts it only under `MRB_UTF8_STRING`.

## Deviations kept

* NaN identity: every NaN is one immediate here, the reference allocates one object per NaN
  (`[nan].uniq`, `[nan].count(nan)`, `[nan] - [nan]`, two `"…".unpack1("E")` of a NaN).
* `Exception#backtrace` is kept as the frames rather than as text (`Vm::keep_backtrace`, the
  hidden `@__bt`): a program that uses exceptions for control raises far more often than it reads
  the backtrace, so the strings are built only when `backtrace` is called. The text is the format
  `caller` uses and matches the reference's byte for byte on the same script; a re-raise keeps the
  first record, as `mrb_keep_backtrace` does. `MRUBY_REVISION` is this repository's commit
  (`build.rs`), `HEAD` where the build had no git — the reference's own default.
* `RUBY_ENGINE` is `"mruby"` and `RUBY_ENGINE_VERSION`/`MRUBY_VERSION` are the reference's
  version (decided 2026-09-16): the reference's own tests branch on `RUBY_ENGINE == "mruby"` to
  tell mruby from CRuby (`test/assert.rb`, `mruby-fiber/test/fiber2.rb`), and any script written
  for mruby may do the same, so answering `"sabiruby"` would send them all down the CRuby path.
  A script that wants to know it is on SabiRuby has `MRUBY_PLATFORM == "rust-sabiruby"` and
  `SABIRUBY_VERSION` (this crate's version; the reference defines no such constant, so
  `defined?(SABIRUBY_VERSION)` is the one-line test). mruby/edge answers the same question with
  `RUBY_ENGINE == "mruby/edge"`; the price it pays is the branch above.
* The differences a character-indexed String brings with it are their own list, in
  [`utf8.md`](utf8.md) ("Deviations kept").
* `` Kernel#` `` is private, as it is in the reference (mruby-io writes it `module_function def
  \``), but the public `` Kernel.` `` that is the other half of that pair is not defined. A
  `self` that *is* Kernel would find it before any instance method written over it, which is
  what mruby's own `test/t/syntax.rb` ("External command execution.") does; it passes there
  because mruby-io's body runs the command, and the body here is the `NotImplementedError` the
  core `mrblib/kernel.rb` gives (there is no shell in a no_std VM). Visibility otherwise matches
  method for method (`verification/coverage.md`, "On both sides, with a different visibility").
* **mruby-regexp**: the engine is `regex-automata`, not the reference's NFA, so what a finite
  automaton has none of is refused at compile time with `RegexpError` naming the construct. The
  reference's own test files (14 of them, 501 assertions) use them, and those assertions are
  counted as intended differences rather than fixed. In the default build, by construct:

  | refused construct | assertions | written instead |
  |---|---:|---|
  | lookbehind `(?<=…)` `(?<!…)` | 22 | — |
  | lookahead `(?=…)` `(?!…)` | 16 | `(?!)` alone (a pattern that never matches) is `\b\B` |
  | backreference `\1`…`\9` | 15 | — |
  | named backreference `\k<…>` | 10 | — |
  | subexpression call `\g<…>` | 8 | — |
  | possessive quantifier `*+` `++` `?+` | 4 | — |
  | atomic group `(?>…)` | 2 | — |
  | absent operator `(?~…)` | 2 | — |
  | conditional `(?(…)…)` | 1 | — |
  | nesting past `regex-syntax`'s depth limit | 1 | — |

  `\p{…}` is refused on both sides, so it is not in that table: the reference's engine is built
  without the Unicode property tables (`character property is not supported: /\p{L}/`) and this
  one refuses it in the same place, in this engine's wording (`character property is not supported
  by this engine: /\p{L}/`). A bare `\p`, and `\pL`, are the letter in both, as they are in CRuby.

  What the two engines answer differently where both compile the pattern (39 assertions, most of
  them in `regexp_syntax.rb`): `\Z` is `\z` here, so it does not match before a trailing newline
  (writing it takes a lookahead); `^` matches after a trailing newline, where Ruby opens no line
  there (that takes a lookbehind); a repetition's last, empty iteration keeps no capture, the
  automaton having no backtracking state to keep it in; `/i` is Rust's Unicode simple folding,
  applied to each member of a class rather than to the class its set operations build, and to a
  POSIX bracket before its negation; a nested negated class and a `&&` of two unions compile here
  where the reference refuses them; the reference's step and stack limits (`MRB_REGEXP_STEP_LIMIT`,
  `MRB_REGEXP_STACK_LIMIT`) have no equivalent, an automaton being linear in the subject, so the
  constants are there and nothing raises against them; and a byte-read subject (`String#b`) is
  searched with the pattern's own automaton, which was built for characters unless the *pattern*
  was byte-read too — the reference decides that per search, so `"\xC3\xA9x".b.scan(/./)` answers
  three bytes there and two characters here. A few of the parser's complaints keep the
  reference's wording (`empty range in char class`, `too big number for repeat range`,
  `too many capture groups are specified`, the unmatched-parenthesis pair); the rest are
  `regex-syntax`'s own text.
* Wide integers (mruby-bigint): five answers of the reference are slips of its own bigint code,
  not decisions, and SabiRuby keeps the meaning the same at both widths
  (`tests/custom/bigint_reference_bugs.rb` holds them with CRuby's answers):
  `~x` is `-x-1` (`mrb_bint_rev` takes one off the *magnitude*, answering `-(x-1)` for `x > 0`);
  `x >> n` floors for a negative `x` (`mpz_div_2exp` shifts the magnitude, truncating toward
  zero); `x.div(f)` divides by a Float (`mrb_bint_div` multiplies by it); `x % f` floors like
  `Integer#%` (`mrb_bint_mod` takes `fmod`, which truncates); and `x.dup` is the number (the
  reference's `mrb_obj_dup` copies an RInteger as an empty object and answers 0 — visible there
  for `(2**62).dup`, which is wide in its build and immediate here). The bit operations work over
  one limb more than the longer operand, which the reference does not, so a result needing the
  extra limb (`-1 ^ 0xffffffff`) is not truncated.
* `Integer#quo` on a wide integer answers the exact Rational (`(2**64).quo(2)` is
  `(9223372036854775808/1)`); the reference reads the wide receiver as a machine integer
  there and answers `(0/1)`.
* `Integer#div` and `Integer#divmod` with a Float or a Rational divisor divide by the value
  (`2.div(1.5)` is 1, `2.div(Rational(1, 2))` is 4, as in CRuby); the reference converts the
  divisor to an Integer first (`mrb_as_int`), which answers 2 and raises ZeroDivisionError.
* A Rational and a Complex are plain objects with hidden instance variables here, so
  `ObjectSpace.count_objects` counts them as `T_OBJECT`, not `T_RATIONAL`/`T_COMPLEX`.
  `Rational#eql?` is defined natively (the reference gets it from `mrb_eql`'s "same type,
  then `==`" rule, which has no equivalent here).
* `ObjectSpace.count_objects` has a `T_BIGINT` entry, counted after `T_BREAK` as in the
  reference's type enum.
* **mruby-sprintf** — `Kernel#sprintf`/`format` (`ext_sprintf.rs`): the reference's state machine
  for flags, `n$`, `<name>`/`{name}` and `*`, with its error messages; integers follow
  `mrb_int_to_cstr`/`mrb_uint_to_cstr` including the `..f` two's-complement form for negative
  `%x`/`%o`/`%b`; floats are rendered with `core::fmt` (correct rounding) and reshaped to C's
  `%f`/`%e`/`%g` (exponent of at least two digits, `%g` trailing-zero removal, `#` keeps them).
  `String#%` is the gem's Ruby part. The core-stage `String#%` stub was removed.
* **mruby-metaprog** (`ext_metaprog.rs`) — instance/class variable name checks (`'@0' is not
  allowed as an instance variable name`), `methods`/`instance_methods` families with the
  `regular`/`inherit` argument and the reference's listing rule (first table wins, `undef`
  hides), `singleton_methods(recur)`, `local_variables` from the caller's irep names,
  `included_modules`, `constants(inherit)`, `Module.constants`, `Module.nesting`,
  `remove_method` (+ `method_removed` hook), `undefined_instance_methods`, `public_send` with
  `method_missing` fallback. Two structural fixes came with it: the natives SabiRuby's core stage
  had defined on **Object** are moved to **Kernel** at init (the reference defines them in
  `mrb_init_kernel`; owners, listings and `Kernel.instance_method(:inspect)` depend on it), and
  `Vm::funcall` now dispatches a user-defined `method_missing` like a SEND does.
* **mruby-proc-ext** (`ext_proc.rs`) — `Proc#inspect` (`#<Proc:0x... -:-> (lambda)`),
  `lambda?`, `parameters` (from the ENTER operand and the irep's local names),
  `source_location` (nil), `Kernel#proc`; `curry`, `<<`, `>>`, `===`, `yield` are Ruby.
  `proc { break }.call` raising LocalJumpError needed the orphan rule for natives: when a native
  returns, a non-strict block made by the calling frame and passed to it is marked orphan
  (`Vm::orphan_block_of_native`; the reference does it in `cipop` of the C frame).
* **mruby-method** (`ext_method.rs`) — `Method`/`UnboundMethod` as plain objects with the
  reference's ivars `_owner`, `_recv`, `_name`, `_proc`, `_klass` (plus `_missing` for
  `respond_to_missing?` methods). A native method has no Proc object here: it is looked up again
  by owner and name when called or compared (two natives are `==` when they are the same
  function). Native arity comes from a small table keyed by function (`Vm::native_arity`),
  `-1` otherwise, because SabiRuby natives carry no `MRB_ARGS_*` spec.

* **mruby-compar-ext**, **mruby-toplevel-ext**, **mruby-enum-lazy** — pure Ruby, loaded as is. The
  core stage had a native `clamp` on Comparable and on Integer; in the reference the only `clamp` is
  this gem's Ruby method, so both natives went (rule 4: the native took two arguments and shadowed the
  one-argument range form).
* **mruby-enum-chain** — pure Ruby. Its `rewind` test defines a singleton method on a Range literal,
  which uncovered a deviation: SabiRuby marked every Range **frozen** and used that flag as the
  reference's `RANGE_INITIALIZED` flag (`'initialize' called twice`). In the reference `(1..2).frozen?`
  is false, and since the singleton class of a frozen object is frozen too (`sc->frozen = o->frozen`),
  `define_method` on the range's singleton class raised FrozenError here. Now an uninitialised Range is
  a plain object until `initialize`/`initialize_copy` gives it its ends, and no Range is frozen.
  `Enumerator::Chain#size` with a Range fails on the reference as well (mruby-range-ext defines
  `Range#size`, the test expects nil).

* **mruby-object-ext** (`ext_object.rs`) — `NilClass#to_a/to_h/to_i/to_f`, `Kernel#itself`,
  `BasicObject#instance_exec`; `tap`, `then`/`yield_self` are its Ruby (the core-stage natives
  `tap`/`then` went). `instance_exec` on an Integer uncovered a rule of `mrb_singleton_class_ptr`:
  an Integer/Float/Symbol has no singleton class, the frame then has no target class of its own
  and `class B` inside the block goes to the block's lexical class (`Vm::call_block_with_self_kw`
  passes no override). Keywords given to `instance_exec` stay keywords for the block (the
  pending-kdict rule of `send`).
* **mruby-symbol-ext** (`ext_symbol.rs`) — `length`/`size` (bytes in this build), `slice`/`[]`,
  which hand the name to `String#slice` as the reference does; `Comparable`, `capitalize`,
  `casecmp`, `empty?`, `intern` are its Ruby (the core-stage native `empty?` went).
* **mruby-kernel-ext** (`ext_kernel.rs`) — `Integer()`/`Float()` are the reference's scanners
  ported byte for byte (`mrb_str_len_to_integer`, `mrb_str_len_to_dbl`, `mrb_read_float`; the
  float's value is parsed by `core` from the exact span, so rounding is right). `caller` is the
  reference's arithmetic over `Vm::backtrace`, new here: one `file:line:in method` entry per Ruby
  frame with debug info, the native itself first, located at the frame that called it (the
  reference locates a C frame at the nearest Ruby frame below it the same way). `__method__`
  reads the frame: its `mid`, else the environment's (a block answers the method it was written
  in). Two things came with it: **an alias is a proc of its own** (`ProcData::mid`, the
  reference's `MRB_PROC_ALIAS` with `body.mid`) so a frame of `alias m3 m1` has `mid` `:m1`,
  which `__method__`, `__callee__` and `super` see; and `raise`/`fail` is one function. The
  core-stage `Integer()`, `Float()`, `String()`, `Array()`, `__method__` moved here.
* **mruby-class-ext** (`ext_class.rs`) — `Module#<`, `<=`, `<=>`, `>`, `>=` with the reference's
  three answers (true, false, nil when unrelated; a TypeError for a non-module), `class_exec`/
  `module_exec` (keywords kept), `name` (frozen), `singleton_class?`; `Class#attached_object`,
  `subclasses` (a heap walk over `Heap::ids`). The core-stage `Module#<`, `<=`, `name` moved here.
* **mruby-numeric-ext** (`ext_numeric.rs`) — `remainder`, `pow(b, m)` with the reference's
  overflow rule, `digits`, `size`, `bit_length`, `odd?`, `even?`, `gcd`, `lcm`, `modulo`,
  `Integer.sqrt`, `Float#remainder`/`modulo`, `Float::EPSILON`.. constants. `zero?`, `nonzero?`,
  `positive?`, `negative?`, `integer?`, `allbits?`, `ceildiv` are its Ruby: the core-stage natives
  of those went, and `even?`, `odd?`, `size`, `bit_length`, `gcd` moved here. The core `Float#div`
  (`flo_idiv`) was missing and is in `numeric.rs` now.
* **mruby-objectspace** (`ext_objectspace.rs`) — `count_objects` (TOTAL = every slot, FREE = the
  swept ones, then the live `T_*` counts in the reference's type order), `each_object`. Its test
  counts hashes across a `GC.start`, which exposed **the stack root**: SabiRuby marked the whole
  register vector, so registers left over from returned frames kept garbage alive. Now the
  running frame's window is the limit (`base + nregs`, or the arguments still to be packed),
  as `mark_context_stack` marks `ci->stack + nregs`. The test's C helper `__gc_root_survivors`
  (`mrb_gc_register` counting) is in `mrbtest.rs`; it found that `Heap::is_free` said "live" for
  a slot the sweep had truncated away.
* **mruby-catch** (`ext_catch.rs`) — `catch` records its tag and the depth of the block's frame
  in `Vm::catch_tags`; `throw` finds the innermost entry with the same object and returns a
  `Break` to that frame, the mechanism a `return` from a nested block already uses, so `ensure`
  bodies run and `rescue Exception` does not see it (the reference raises an `RBreak` aimed at
  the frame of its bytecode `catch`). An unmatched tag raises the gem's `UncaughtThrowError`.

* **mruby-math** (`ext_math.rs`) — the `Math` module over `libm` (already a dependency), the
  reference's domain checks as `Math::DomainError`; module functions. Output is identical to
  the reference for the values tried (`frexp`, `ldexp`, `cbrt`, `log(x, base)`, ...).
* **mruby-random** (`ext_random.rs`) — PCG-XSH-RR with the reference's seeding, so
  `Random.new(123)` gives the reference's sequence (checked: `rand(1000)`, `rand`, `bytes`,
  `shuffle`, `sample` with the same seeds). The state lives in two hidden instance variables
  (`__state`, `__seed`) rather than an `MRB_TT_ISTRUCT` payload. The default generator is
  seeded with a constant (the VM has no clock; the reference uses `time(NULL)`), so its
  sequence repeats across runs until `srand`; `srand` without a seed mixes the host's
  `gc_clock` when there is one.

* **mruby-struct** (`ext_struct.rs`) — a struct instance is the reference's `MRB_TT_STRUCT`, an
  array-shaped object whose class is the struct class: here an `ObjKind::Array` with that class,
  so `mrb_ary_set`/`mrb_ary_replace` are the array operations on it (VM paths keyed on the array
  kind, such as a splat, see the members, as the reference's `to_a` gives them anyway). The member
  accessors are one native each way that reads the name it was called by (`Vm::native_mid`, set
  at every native dispatch) instead of the reference's per-member C procs carrying an index. The
  constructor logic (`struct_init_body`, the overridden-`initialize` bridge `__struct_init_fwd`,
  `keyword_init`) is the reference's; "keywords given" is the pending-kdict rule. A struct's
  recursive `inspect` uncovered a shortcut in `Vm::inspect`: the recursion mark was chosen by
  storage kind (`[...]` for anything array-shaped); it now goes by class, so a Struct or Set
  answers through its own `inspect`.
* **mruby-data** (`ext_data.rs`) — the same shape, frozen once built; `Data.define`, keyword
  construction (`missing keyword`/`unknown keyword` are ArgumentErrors), `with`, the bridge
  `__init_with_kw` for an overridden `initialize`.
* **mruby-set** (`ext_set.rs`) — a Set is a Hash-shaped object (`ObjKind::Hash`, element => true)
  with the class `Set` (`instance_kind`), so membership follows the Hash's `hash`/`eql?` rule and
  elements come out in insertion order (the reference's khash walks its buckets). Deviations: an
  unfrozen String element is stored as a frozen copy (the Hash key rule); the reference's
  "uninitialized Set" state and its rebuild-during-`eql?` RuntimeError (GHSA-4jw6-mq65-g3c8,
  one failing assertion) do not exist here, the lookup simply finishes. `Set#hash` is the
  reference's xor fold over 32-bit element hashes.

* **mruby-time** (`ext_time.rs`) — `sec`/`nsec`/zone in hidden instance variables (`__sec`,
  `__nsec`, `__utc`); the calendar (`gmtime`, `timegm`) is computed in the VM (days-from-civil
  and back). There is no `localtime`: the local zone is UTC with the offset `+0000` (no zone
  database in a no_std VM), so `Time.local` and `Time.utc` differ only in `utc?`, `zone` and the
  `to_s` suffix; `utc_offset` is 0 and `dst?` false. `Time.now` reads the host's `Vm::wall_clock`
  (the CLI sets it from `SystemTime`); without one it is the epoch. Checked against the reference:
  negative times, leap years, day overflow (`Time.gm(2024, 2, 30)`), the float/usec rounding of
  `Time.at`, `-` between Times.

* **mruby-bigint** (`src/bigint.rs`, the branches in `src/builtins/numeric.rs`) — the only
  gem that changes what a core type *is*: an Integer that leaves the `i64` range stops being an
  immediate and becomes a heap object, so every arithmetic, comparison, conversion and hash path
  of Integer has a second width to answer for. What it needed:

  * **Representation** — `ObjKind::BigInt(BigInt)`, a sign and the absolute value in 32-bit
    limbs (the reference's `mpz_t`: `sn`, `p[0..sz]`), with the class `Integer`, so `class_of`,
    `is_a?` and `inspect` need no special case. The reference's embedded-limb optimization
    (`RBIGINT_EMBED_SIZE_MAX`) is not copied. **Normalization is the invariant the rest relies
    on**: a result that fits in an `i64` is always turned back into `Value::Int`
    (`Vm::bint_value`, the reference's `bint_norm`), so one number never has two shapes and
    `==`, `eql?`, `hash` and Hash keys stay simple.
  * **The loader** — pool type 7 (`IREP_TT_BIGINT`) is `len, base, digits[len]`, and
    `load.c`'s `pool_data_len = len + 2` counts the length byte itself. `src/rite.rs` read
    `len + 2` bytes *after* the length byte, one too many, and the next pool entry's type byte
    was then read as a digit (`bad pool type 56`, the `'8'` of a literal). A negative base means
    a negative number (`mrb_bint_new_str`). This is why `test/t/literals.rb` skipped.
  * **The arithmetic** (`src/bigint.rs`) — schoolbook multiplication and Knuth's algorithm D for
    division, not the reference's Karatsuba/Barrett/Montgomery: `tools/bench.sh` shows the
    integer benchmarks unchanged (the VM's `checked_*` fast path is untouched and only its
    overflow exit reaches here), and no test is slow. `tests/bigint.rs` checks the layer against
    `i128` for everything that fits and against known values (`3**1000`, `Integer.sqrt(10**40)`,
    base 2..36 round trips) for what does not.
  * **The core branches** — `+ - * / div % divmod ** pow -@ ~ & | ^ << >> <=> == eql? hash to_s
    inspect to_f succ pred chr floor ceil round truncate quo fdiv size bit_length digits gcd lcm
    even? odd? remainder pow(b,m) Integer.sqrt`, `Float#to_i`/`floor`/`ceil`/`round`/`divmod`
    (a Float beyond the `i64` range now converts instead of raising), `String#to_i`/`hex`/`oct`,
    `Integer()`, `sprintf` (`%d`/`%x`/`%o`/`%b` and the `..f` form for a negative wide value),
    `rand(big)`, `Time.at(big)`, and `Vm::expect_int`, which answers the reference's RangeError
    `integer out of range` wherever a wide integer reaches something that needs a machine
    integer (an Array index, a shift width, an exponent).
  * **What stopped raising** — `RangeError: integer overflow` is gone from `+`, `-`, `*`, `**`,
    `<<`, `MRB_INT_MIN / -1` and `-@`; those now promote, in `op_arith`'s overflow exit as well
    as in the methods.
  * **`Integer#hash`** — a wide integer hashes its limbs and sign (`mrb_bint_hash`), so
    `{2**64 => 1}[2**64]` finds the entry although the two objects are different.

  Checked against the reference image with this script (`docker run --rm -v "$PWD:/w" -w /w
  kishima/mruby:4.1.0-rc mruby probe.rb` against `sabiruby probe.rb`; everything agrees except
  the six lines listed under "Deviations kept"):

  ```ruby
  def t(l); print l, " => "; begin; p yield; rescue => e; p e; end; end
  t("2**64"){ 2**64 };                t("square"){ (2**64) * (2**64) }
  t("div"){ -(2**64) / 7 };           t("divmod"){ (2**64).divmod(-7) }
  t("to_s(2)"){ (2**64).to_s(2).size }; t("to_s(36)"){ (2**64).to_s(36) }
  t("pow"){ 3.pow(1000).to_s.size };  t("powm"){ (2**64).pow(2, 7) }
  t("shift"){ (1 << 64) >> 63 };      t("and"){ (2**64) & (2**63) }
  t("float =="){ 2**64 == 18446744073709551616.0 }
  t("to_f.to_i"){ (2**64).to_f.to_i == 2**64 }
  t("hash key"){ ({2**64 => 1})[2**64] }
  t("%d"){ "%d" % 2**64 };            t("%b"){ "%b" % -(2**64) }
  t("sqrt"){ Integer.sqrt(10**40) };  t("digits"){ (10**40).digits.size }
  t("frozen?"){ (2**64).frozen? };    t("size"){ (2**64).size }
  t("to_i"){ "18446744073709551616".to_i }
  t("Integer()"){ Integer("18446744073709551616") }
  t("1e30.to_i"){ 1e30.to_i };        t("MIN/-1"){ (-9223372036854775807-1) / -1 }
  t("index"){ [1,2,3][2**64] };       t("chr"){ (2**64).chr }
  t("Time.at"){ Time.at(2**64) };     t("exp"){ 2 ** (2**64) }
  ```

* **mruby-rational** (`ext_rational.rs`) and **mruby-complex** (`ext_complex.rs`) — the two
  gems that finish the numeric tower. They land in the same places of `numeric.rs` as
  mruby-bigint and were ported together, because the Rational tests need Complex and the
  Complex tests need Rational (`add_test_dependency`).

  * **Representation** — a Rational keeps `__num`/`__den` and a Complex `__real`/`__imag` in
    hidden instance variables of a plain object (the Random/Time style), not a new
    `ObjKind`. A Rational's halves are Integers of any width, always reduced with the sign on
    the numerator; a Complex's parts are whatever member of the tower they were given
    (Integer, Rational or Float — the reference's `COMP_VALUE` form, which is the only form
    here). Both objects are frozen. `Vm::core` gained `rational` and `complex` so a native can
    ask "is this one of ours" with one comparison, and `Vm::s` the four ivar names.
  * **Exact arithmetic** — the reference works in `mrb_int` and reaches for a wide integer
    when that overflows; here every Rational operation goes through [`BigInt`] and comes back
    normalized, which is the same value with less code. A Complex works part by part through
    the tower's own dispatch (`mrb_num_add` is a `funcall` here), so an exact pair stays exact:
    `(1+2i) * (3-1i)` is `(5+5i)` with Integer parts and `1/Complex(1,2)` is
    `((1/5)-(2/5)*i)`.
  * **The core branches** — `numeric.c`'s `MRB_USE_RATIONAL`/`MRB_USE_COMPLEX` arms, in
    `numeric.rs`'s `tower()`: `Integer` `+ - * /` with a Rational or a Complex hands the pair
    to the gem, `Float` does the same for a Complex only (a Rational converts to a Float
    there), `==` asks the wider operand, `<=>` compares a Rational through Float and asks a
    Complex for its own answer, and `Integer#quo` of two Integers is now a Rational.
    `Vm::expect_int` gained the `mrb_ensure_int_type` arms (a Rational truncates, a Complex
    converts when its imaginary part is zero), which is why `2 ** Rational(2, 1)` is 4.
  * **What the receiver taught** — SabiRuby had `<`, `<=`, `>`, `>=` as natives on **Numeric**;
    in the reference they are Comparable's, and Integer and Float have their own. With a
    Rational receiver the native compared nothing it knew and raised. `cmp` now dispatches
    `<=>` for a receiver that is neither an Integer nor a Float, which is what Comparable
    does — and that is also what makes mrblib's `Numeric#abs` (`self < 0`) work for a Rational.
* **mruby-cmath** (`ext_cmath.rs`) — `CMath`, Math over the complex plane. The reference
  leans on C99 `<complex.h>`; the functions are written out here from their definitions with
  C's principal branches (`casin z = -i ln(iz + sqrt(1 - z^2))`, and so on), on `libm`. The
  gem is **not in `default.gembox`**, so the reference image has no `CMath` and cannot answer
  for it: the checks are its own test file (21 assertions, all passing, including the
  round trips `sin(asin z)`, `cosh(acosh z)`, …) and `CMath.log(-8, -2)`, whose expected
  value the test spells out.

* **mruby-pack** (`ext_pack.rs`) — `Array#pack`, `String#unpack`, `String#unpack1`. The
  template parser and all the directives (`a A Z b B h H c C s S l L q Q j J n N v V U w m M u
  f d e E g G x X @`, the modifiers `_ ! < >`, `*` and counts) are the reference's, byte for
  byte, including its own choices: `i`/`I` and `j`/`J` are decided by the size of C's `int`
  and `intptr_t` (4 and 8 here, so `j` is `q`), a modifier after anything but `sSiIlLqQ` is an
  ArgumentError, `#` is a comment to the end of the line, `p`/`P`/`%` are refused, and an
  unsigned 64-bit value that does not fit an Integer is `RangeError: cannot unpack to Integer`
  — the reference tests `MRB_INT_MAX` there even in a build that has mruby-bigint, so a wide
  integer never comes out of `unpack("Q")`, and `[2**63].pack("Q")` is a RangeError from the
  argument conversion. Base64, quoted-printable, uuencode, BER and UTF-8 are written out here
  (no dependency added). Checked directive by directive against the reference image: two
  scripts of 60 and 40 lines (every directive, every modifier, the counts, the error cases)
  answer identically.

* **mruby-eval** (`ext_eval.rs`), **mruby-binding** (`ext_binding.rs`) and
  **mruby-proc-binding** — `Kernel#eval`, the string forms of `instance_eval` and
  `class_eval`/`module_eval`, `Kernel#binding`, `Binding` and `Proc#binding`. These are the
  first gems that need something the VM crate does not have: a compiler.

  * **The host** (`src/host.rs`) — the VM asks a `Host` (`Vm::set_host`) to turn a string
    into a RITE binary, handing it the file name, the line and **the local variable names of
    the enclosing scopes**. `sabiruby-compiler` implements it (its feature `host`), so the
    dependency points compiler → VM and the VM stays pure Rust and `no_std`; the `sabiruby`
    command, the test runner and `tests/custom.rs` install it. Without one, `eval` raises
    NotImplementedError.
  * **The one patch to the vendored compiler** (`compiler/vendor/VENDOR.md`,
    `SABIRUBY_EVAL_SCOPES`, 121 lines) — the reference's compiler reads the caller's `RProc`
    chain to know those names (`MRC_TARGET_MRUBY`); this build has no VM to read, so the same
    two places (`mrc_pm_options_init` for Prism, `search_upvar` for the code generator, plus
    numbered parameters) read the name table instead. `sabiruby_mrc_compile` — the `mrbc`
    entry the golden tests cover — is not touched.
  * **What makes the string see the caller** — the Proc the string becomes has the caller
    frame's *environment* as its own and the caller's Proc as its `upper`, so the
    `GETUPVAR`/`SETUPVAR` the compiler emits at depth 0 read and write the caller's registers
    (`create_proc_from_string`). Depth accounting follows from that: from a block inside the
    string the caller is one step further out, which is the reference's `lv - 1`.
  * **Binding** — the reference's four instance variables (`proc`, `env`, `recv`, `pc`) and
    its **local variable space**: the Proc a Binding holds is not the caller's but a Proc with
    no locals wrapped around it, with an environment of its own. `local_variable_set` of a new
    name grows that space (`mrb_proc_merge_lvar`: the irep's table and the environment
    together), and `dup` wraps a fresh space over the shared one — which is what makes a copy
    see the variables set so far and not the ones set afterwards.
  * **`Binding#eval` and the variables a string leaves behind** — the reference parses the
    string once to find the names its top level defines and merges them into the binding
    before compiling (`binding_eval_prepare`). Here the string is *compiled* once instead: the
    local variable table of the result is exactly what did not resolve to an enclosing scope.
    Those names are merged and the string is compiled again, so `b.eval("x = 2")` leaves `x`
    in the binding and the next `b.eval("x")` finds it.
  * **`Class#new` sends `allocate`** — in the reference `Class#new` is bytecode
    (`new_iseq` of `src/class.c`) that sends `allocate` and then `initialize`, so a class that
    redefines `allocate` decides what `new` makes. SabiRuby's native allocated directly; it
    now dispatches (skipping the dispatch when `allocate` is still the built-in one, so the
    benchmarks do not move). mruby-binding's test found this.
  * **`require`/`load`** (`src/mrblib/require.rb`, `src/builtins/ext_require.rs`,
    `docs/plans/eval-require-plan.md` §5) — mruby has none of it, so the shape is picoruby-require's
    (MIT), with three differences. There is no `extern`: the gems are all linked from the start,
    so their names are in `$LOADED_FEATURES` from the first line and `require 'fiber'` answers
    false. There is no `File`: a path is built as a string in Ruby (`"#{dir}/#{name}.rb"`, no
    `expand_path`) and handed to the two natives `__file_exist?` and `__load_file`, which are the
    host's `file_exists`/`read_file` — so a build with no host has no `require` either and the VM
    stays `no_std`. And there is no Sandbox: `__exec_file` runs the file in a top-level frame of
    its own (`Vm::run_irep`), a `RITE` file as it stands (a version this VM does not read is a
    LoadError) and Ruby source through the host's compiler. `$LOAD_PATH` comes from the host
    (`Vm::set_load_path`; the `sabiruby` command uses the program's directory and the working
    directory, and `-r` loads a library first). The feature is recorded *before* the file runs,
    so a cycle stops instead of repeating, and taken back out if the file raises — CRuby does
    both, picoruby-require records afterwards and loops. `tests/require.rs` holds the cases,
    every expectation measured with CRuby 3.2 on the same files.

* **mruby-regexp** (`src/regexp/mod.rs`, `src/builtins/ext_regexp.rs`) — `Regexp`, `MatchData`
  and the String and Symbol methods whose regexp form the gem answers. The reference carries its
  own NFA (`re_compile.c`, `re_exec.c`, `re_utf8.c`, 7,263 lines, plus 3,611 lines of tables);
  that part is **not** ported. The engine here is Rust's `regex-automata` (author's decision of
  2026-09-13, `docs/plans/gems-plan.md` 3.6), so what a pattern *means* is a finite automaton's: linear
  in the subject, and without backreference, lookaround, atomic group or subexpression call. The
  Ruby surface above it is ported from `regexp.c` as usual, and the two halves meet at a
  translation layer.

  * **The pattern is shared, not pointed at** (2026-09-14) — a Regexp holds its compiled pattern
    as `Arc<Pattern>` (`ObjKind::Regexp`). The port first kept the reference's shape
    (`DATA_GET_PTR`, then search with the pointer) as a `*const Pattern` read back with `unsafe`
    at 14 places, because a search goes on using `&mut Vm` (MatchData, `$~`, and in `gsub` a
    block that runs any Ruby) while it needs the pattern, and a `&Pattern` borrowed from the heap
    cannot be held across that. It was sound only through two rules nothing checked: a Regexp is
    never initialized twice, and the collector does not run under a native. `pattern_of` now
    clones the `Arc` (one reference count per call), the pattern lives as long as a search holds
    it whatever happens to the object, and the VM crate is back to one `unsafe` (the opcode
    byte to `Op`).

  * **The translation layer** (`src/regexp/mod.rs`) — Ruby's pattern syntax written as
    `regex-syntax`'s, about 500 lines against the 7,263 not ported. What the two spell
    differently is rewritten: the ASCII shorthands (`\d`, `\w`, `\s`, `\h` and their negations,
    each wrapped in `(?-i:…)` so `/i` never folds them out of ASCII), the octal and `\xNN`
    escapes (a run of them is decoded together, so `\303\244` is the one character those bytes
    spell), `\uXXXX` and `\u{…}`, `(?'name'…)`, the comment group `(?#…)`, `{,m}`, a `{`
    that opens no repeat, and Ruby's `m` flag, which is the automaton's `s` (the automaton's `m`
    is `^`/`$` as line anchors, which Ruby has on whatever the flags). Free spacing (`/x`) is
    applied here rather than by the parser, because the parser's own drops the spaces a class
    holds. A pattern that names a group numbers no other (Onigmo's `DONT_CAPTURE_GROUP`), so a
    plain `(` is written `(?:` there; the names are kept beside the pattern rather than in it,
    since Ruby lets two groups share a name and names one the automaton would refuse. A digit
    escape is a backreference or an octal escape by the group count, as CRuby reads it.
  * **What reads a byte as a byte** — a pattern byte that starts no character stands for the
    byte, which only a span with Unicode turned off can compare, so such a span is written
    `(?-u:…)` (a whole class where one of its members is such a byte). A byte-read pattern
    (`Regexp.new(s.b)`) compiles with `unicode(false)` throughout, and so does the whole
    byte-string build, where `\u{…}` becomes the UTF-8 spelling of the codepoint the way the
    character written out is spelled.
  * **`$~`** — the one name a match publishes; `$&`, `` $` ``, `$'`, `$+` and `$1` onward are
    readings of it the compiler derives. In 4.1.0-rc it lives in the scope that owns it
    (`svar_owner` of `src/vm.c`), not in the globals table: a method's match stays out of its
    caller's `$~`, a block shares its defining method's, a native writes through to the Ruby
    frame below, and a fiber keeps its own. SabiRuby holds it in the owning scope's environment
    (`EnvData::svar`, a container made on the first non-nil write, with `$_`'s slot beside it),
    resolves the owner by the same walk — including the redirect that keeps a fiber's matches to
    itself and the forward a nested `mrb_load_string` frame leaves behind when it returns — and
    intercepts `$~` in `OP_GETGV`/`OP_SETGV`, the gem registering it as a virtual global.
    `test/backref_scope.rb` (64 assertions) pins all of it and passes.
  * **The surface** — `Regexp` (including `union`, `last_match`, `named_captures`, the
    `to_s` fold of a leading option group), `MatchData`, and the String methods the gem takes
    over from core by aliasing the originals away (`__split`, `__aref`, `__index`, …), which is
    why `ext_regexp::init` runs *after* `load_mrblib`: a gem initialises after core's Ruby part,
    and mrblib's `sub`/`gsub` would otherwise win. What says a `Regexp` was initialized at all is
    the pattern slot (`ObjKind::Regexp(Option<…>)`), the reference's `DATA_PTR`: it is opened
    before the compile, so an object whose compile raised still answers `source` and `inspect`
    while refusing to match, and a `dup` whose `initialize_copy` never called `super` answers
    neither.
  * **The engine's own tests** are `tests/regexp_engine.rs`; everything else is the reference's
    (`tools/mrbtest.sh`, the `gem_regexp*` files). The pair `ascii_case`/`ascii_ctype` and
    `unicode_case`/`unicode_ctype` assert opposite things about the same patterns, so each build
    runs one pair, as the gem's `spec.build_settings` says.

* **mruby-sleep** (in `ext_task.rs`, beside the task-aware `sleep`) — `Kernel#sleep(sec)` and
  `#usleep(usec)`. Both this gem and mruby-task define these names; the reference lets
  mruby-task's win where both are linked (its README says so and tells you to drop mruby-sleep),
  so there is one function here with mruby-task's arity and messages. What mruby-sleep adds is
  what happens **outside** a task: the reference blocks in `usleep(3)`, and a `no_std` VM has
  nothing to block on, so the host lends one (`Vm::sleep_hook`, the same shape as `wall_clock`;
  the `sabiruby` command passes `std::thread::sleep`). Without a hook the call returns at once,
  which is what the test runner does — the reference's own suite spends a second on
  `sleep(1)` and this one does not. The answer is the seconds (or microseconds) actually waited
  where the host has a clock, and what was asked for where it has none.

* **mruby-strftime** (`ext_strftime.rs`) — `Time#strftime`. The reference hands the format to the
  platform's `strftime(3)`; there is no C library here, so the conversions are written out
  against the broken-down time `ext_time.rs` already computes: `%Y %C %y %m %B %b %h %d %e %j %H
  %I %M %S %p %A %a %w %u %Z %z %F %T %D %R %c %x %X %n %t %s %%`, with glibc's `-` (no padding),
  `_` (spaces) and `0` (zeros) flags and an optional field width. A conversion the C library does
  not know is copied through as written, as glibc does. `%Z` is `UTC` and `%z` is `+0000`, which
  is what this `Time` decided (there is no time zone database). A NUL in the format is a byte like
  any other and comes out where it stood — the reference reaches the same answer by cutting the
  format at each NUL, calling `strftime(3)` per piece and putting the NULs back.

  There is no reference image to compare with (the gem is in no gembox), so each conversion was
  checked against CRuby, whose C-locale answers are glibc's:

  ```ruby
  t = Time.gm(2026, 9, 13, 8, 11, 32)
  %w(%Y %C %y %m %B %b %h %d %e %j %H %I %M %S %p %A %a %w %u %Z %z %F %T %D %R %c %x %X
     %n %t %% %-d %_d %05Y %q).each { |f| print f, "\t", t.strftime(f).inspect, "\n" }
  t2 = Time.gm(2023, 1, 5, 0, 0, 0)
  %w(%I %p %e %j %u %w %C %s).each { |f| print f, "\t", t2.strftime(f).inspect, "\n" }
  ```

  `sabiruby` and `ruby` print the same 43 lines.

* **mruby-task** (`src/builtins/ext_task.rs`, the scheduler state in `Vm::task`) — `Task`,
  `Task::Queue`, `Task::Error` and the task-aware `sleep` family. Not in any gembox; it is here
  because it is the shape a host loop needs (`Vm::task_run_once`).

  * **A task is a context, as a Fiber is.** The scheduler hands one the CPU by resuming its
    context exactly the way `Fiber#resume` does (`Vm::fiber_switch` with `vmexec`), and `sleep`,
    `Task.pass`, `#join` and a blocking `Queue#pop` give it back the way `Fiber.yield` does. That
    is also the reference's design (`mrb_task` embeds an `mrb_context` and `execute_task` swaps
    `mrb->c`), so almost nothing new had to be built: the four priority queues, the statuses and
    the wait reasons sit on top of the machinery the fibers already needed.
  * **The tick is instructions, not milliseconds.** The reference's tick comes from a timer
    interrupt (`mrb_tick` from `SIGALRM` or a multimedia timer, one HAL per platform). A `no_std`
    VM has no timer and no thread to run one on, so the tick is counted in instructions
    (`TaskState::tick_every`, 10,000 by default) and `Task.tick` reports that count in the
    reference's tick unit. A timeslice is therefore a fixed amount of *work* rather than of time
    — which is what a deterministic test suite wants — and a host with a clock can drive the tick
    itself by setting `tick_every` to 0 and calling the scheduler when it pleases.
  * **The idle jumps the clock.** Where nothing is ready and something is sleeping, the reference
    idles the CPU until the timer fires; here the scheduler moves the tick straight to the
    earliest deadline. Where nothing is ready and nothing has a deadline (every task suspended or
    joining), the reference idles forever — `Task.run` returns instead, since nothing else in
    this VM could ever make a task ready.
  * **Preemption is one test at the instruction boundary.** `exec_frames` checks the switch flag
    where the reference's `RETURN_IF_TASK_STOPPED` does, with the same three refusals: not on the
    root context, not while an exception is in flight (the catch handler must consume it first,
    or a rescued exception would be swallowed into the task's result), and not across a native
    frame (`Vm::fiber_check_native`, which is `task_across_c_boundary`).
  * **An unhandled exception is the task's result**, so the scheduler carries on and `Task#value`
    answers the exception object (`mrb->task.exception_as_result`).
  * **A task that is closed or terminated while suspended takes its environments with it.** Its
    frames still hold the stack the escaped blocks read, so `Vm::detach_context_envs` copies the
    values out before the context goes — the same thing the sweep does for a context nothing
    reached (`mrb_env_detach_all`). Without it a closure written inside a task read freed memory
    after `Task#close`, which is what three of the gem's tests are about.
  * **`Task::Queue`** is the gem's own Ruby part (`mrblib/queue.rb`, loaded as
    `src/mrblib/task.mrb`) over five natives; the items are an ivar Array rather than a `DATA_PTR`
    struct. A blocking `pop` parks the task with reason `QUEUE` and answers the `WAIT_RETRY`
    sentinel, which the Ruby loop retries once a push or a close made it ready again.
  * **`GC.scheduler_driven`** turns the collector over to the scheduler's idle points, and
    `GC.debt_limit` is kept as a value. Two of `gc_task.rb`'s six assertions ask for
    `GC.generational_mode` to be on, which this collector never is (`docs/design/gc.md`).
  * The C test helpers (`test/tasktest.c`) are in `src/mrbtest.rs`: the scheduler hook probes,
    `run_once`, `reinit_context`, `run_sync`, and `block_then_raise`, whose busy-wait is replaced
    by saying outright that the timeslice expired — the state the test is about.
  * **The root context is not a task**, as it is not in the reference: `Task.current` answers a
    "main" wrapper there that the scheduler never runs. Nothing runs a task until the root asks —
    `Task.run` runs them all to the end, `Task.pass` from the root runs one ready task for one
    timeslice (`task_run_one_iteration`) — and `Task#join` from the root raises `RuntimeError`,
    `join can only be called from running task`, the reference's words. Wait for a result with
    `Task.run` and read it with `Task#value`.
  * **A call that parks still answers its own value**, because the switch is deferred exactly as
    the reference defers it: the native raises a flag (`switching_ = TRUE`) and returns, the SEND
    stores what it returned, and the context goes at the next instruction boundary. So
    `Task.pass` is nil, `sleep(sec)` in a task is the seconds it asked for (the reference's
    `ms / 1000`), and `Task#join` is the joined task's result *as it stood when join was called* —
    the result where the task had already finished, nil where the wait was real. The gem's own
    tests call none of these for their value, so this was found by reading `task.c` rather than by
    a failing assertion.
  * **A fiber runs inside a task here, where the reference says not to mix them.** Its README is
    explicit ("no compatibility, do not mix in one application"), and its reasons are real: its
    `MRB2TASK` is pointer arithmetic on the running context, so `Task.current` or `sleep` inside a
    fiber reads an unrelated address as a task; and a timeslice that expires inside a fiber leaves
    the run loop *in the fiber's context*, orphaning it while the task resumes as if `resume` had
    returned. Neither can happen here: the task is found by asking whether the running context is
    the running task's (`current_task`, which answers nothing inside a fiber), and the switch is
    deferred across a fiber exactly as it is across a native frame, since a fiber is entered
    through one. So `Fiber` and `Enumerator#next` work inside a task (`tests/task.rs`), with two
    consequences to know: a task **is not preempted while it is inside a fiber** (the work in
    there is not divided into timeslices — yield often if it is long), and inside a fiber the task
    is not visible, so `Task.current` answers the "main" wrapper, `sleep` does not park the task
    and `Task.pass` does nothing. Do the task's own business outside the fiber.
  * **The scheduler is not re-entered from inside a task.** `Task.run` answers nil where the
    caller is a task, as it does where the reference's own loop is already running
    (`loop_running`): a host that drives the scheduler a step at a time leaves that flag clear, so
    a `Task.run` in the program would otherwise take the head of the ready queue — the running
    task itself — and resume the context it is standing in. A program written for `Task.run` is
    therefore unchanged when a host drives it instead; the call simply does nothing.
  * **What a host drives it with** (`Vm::task_*`, checked by `tests/task.rs`): `task_spawn` makes
    a task out of a compiled program rather than out of a Ruby block (`mrb_create_task` takes an
    `RProc`), `task_run_once` is `mrb_task_run_once`, and `task_run_budget` is one turn of a frame
    loop — ready tasks, one timeslice each, until a budget of instructions is spent. A host with a
    clock of its own says so (`task_external_clock`) and moves it with `task_advance_ticks`; the
    instruction count then only ends timeslices, which is what keeps one task from eating a whole
    frame, and a turn that finds nothing ready simply ends rather than jumping the clock (which is
    what `Task.run` does, since nothing else could move it there). `task_value` and
    `task_finished` read a task back, and `task_instructions` and `task_location` are what a host
    shows of a script: what it has spent (the difference between two frames is what it spent on
    that frame) and where it stands in its own source — file and line, while it is parked as well
    as while it runs, which is how a game can show the line a script is waiting on.
    `task_frames` is the same for every frame a task stands
    in, innermost first, so a host showing one file of several can find the innermost frame in
    *that* file — a script parked inside a library method stands in the library, and what its
    author wants to see is the line of their own that is waiting.
    `task_running` answers the task the scheduler currently has on the CPU, which is what a
    native called from Ruby needs before it can use any of the above: the host that spawned the
    task holds its `ObjId`, a method the script calls does not, and what a native usually wants
    is something the host hung on that very task with `Vm::ivar_set` (rubevy gives each script
    the entity it drives that way, and `is_exception` is how the host tells a task that raised
    from one that answered — `Vm::task_value` returns both).
    `task_queue_new` and `task_queue_push` are how a host
    answers a script that asked it for something: hand the script a `Task::Queue`, do the work
    outside (a frame later, a thread, an event loop), push the result, and the `pop` the script
    parked on returns it. `task_queue_len` and `task_queue_try_pop` are the reading side, for a
    queue a script pushes to and the host drains: `try_pop` answers `None` on an empty queue
    rather than parking, because the host is not a task and has nothing to park — an asynchronous host call without a second scheduler, and without the
    script's code looking asynchronous. `task_next_wakeup_ticks` with `task_pending` are what a
    host waits on: how long until the earliest sleeper is due, and whether anything is left that
    could run. A host that has a clock and something to wait on (an event loop, a frame) can then
    make `sleep` cost real time — which is what the browser playground's 実時間 does
    (`docs/design/playground.md`). A `Vm` is `Send + Sync` so that an engine can keep one in
    its own world, which is what the `Host` trait's bound is for (`tests/send_sync.rs`).
  * **Time limits** (2026-09-14, `Vm::task_set_clock`, `Timeslice`, `RunLimits`,
    `Task::Overrun`; `tests/task.rs`) — best effort, on a clock the host gives. Two holes of the
    instruction-counted timeslice were measured with two tasks and a budget of 200,000
    instructions a turn: a task inside a native waiting for a block cannot be switched out, so
    `a.sort { |x, y| x <=> y }` over 300,000 elements took one turn of 13 million instructions
    (326 ms) and `Array.new(1) { loop { } }` never gave the turn back; and one instruction can be
    any amount of work, so `'x' * 50_000_000` reversed in a loop took seconds a turn. The changes:
    * The host may give a monotonic clock in nanoseconds (`task_set_clock`). Nothing reads one
      otherwise; the VM stays `no_std`.
    * `Timeslice::{Instructions, Time { nanos }, Both { nanos }}` says how a slice ends. The
      default stays `Instructions` — deterministic, the same on every machine. `Time` is what the
      reference means (its tick is a timer interrupt), and bounds work the instruction count does
      not see. The instruction tick stays as *when* the clock is read, so the policy changes
      without touching the instruction loop.
    * `task_run_limits(RunLimits { instructions, time_ns, overrun_ns, overrun_instructions })`
      is one turn under limits; `task_run_budget(n)` is it with `instructions` alone. A time
      budget cuts the running slice short at the next look at the clock. A task past a hard
      limit (`overrun_*`) that cannot be switched out gets `Task::Overrun`, which unwinds the
      native as any exception does. It is an `Exception`, not a `StandardError` — as `Interrupt`
      is — so `rescue => e` does not swallow it; the switch stays due, so a task that rescues
      `Exception` and walks back into the same call is still switched out at its next boundary
      (which may be the rescue clause itself: the clause then runs on the task's next turn). A
      hard limit also ends the turn. With those limits the two measured cases come back at the
      limit: `sort` and `Array.new { loop }` end their turn at 50 ms with `Task::Overrun`.
    * **Where the clock is read, and what it costs.** At every tick (10,000 instructions),
      piggybacking on the countdown the instruction loop already does, so an instruction costs
      nothing more; and after every 32 natives (`task_set_native_sample`), only while a limit is
      kept on the clock, because a native can take longer than ten thousand instructions. With no
      clock the benchmarks of `docs/verification/bench.md` stayed within noise of the build before (fib −1.3%,
      so_lists +1.7%, mandelbrot +1.1%, gc_churn −2.8%, vm_optimization_bench +0.02%, best of
      five). With a clock and limits, a loop of eight million `push`/`pop` natives cost +1.2% at
      32, +5.8% at 8 and +34% at 1; fib, which calls no natives, did not move.
    * **What stays out of reach.** A single native is not interrupted: the 20 MB `reverse` (163 ms
      apiece) is noticed after it returns, and with the default sample only after 32 of them. The
      native count bounds how late; making it 1 makes the lateness one call and costs a third on
      native-heavy code. Charging heavy natives by the work they do, or a heap limit, would be the
      next step if that matters.
  * **A turn of the host loop ends when nothing is runnable — not when a task answers nil**
    (2026-09-17, `docs/worklog/2026-09-17-task-end-nil.md`). `task_run_once` answers three
    different things through one value: the result of a task that finished, `true` where one ran
    or the clock jumped to a sleeper, and `nil` where nothing was ready. A task whose block is
    worth nil therefore answers exactly what an empty scheduler answers, and `task_run_limits`,
    which used to stop on a nil, gave up its whole turn each time one ended. Ten tasks finishing
    in the same frame — rubevy's garden replacing ten creatures at once, each reflex task's
    `Queue#pop` raising into an empty `rescue` — cost ten frames in which *nothing else ran*,
    which at 30 fps is a third of a second of frozen VM per burst, and several seconds where the
    frame rate had already dropped. The loop now asks the scheduler instead
    (`ext_task::task_step`, which answers `Ran` / `Idled` / `Stuck`) and stops only on `Stuck`:
    nothing ready and nothing that can be made ready, which under a host-owned clock is still the
    same moment as before — moving the clock is the host's to do. `Vm::task_run_once` keeps its
    value and its meaning; its rustdoc now says that a nil is not the end of the work, and the
    loop to write against it is `while vm.task_pending()`.
  * **The dormant queue is weak** (2026-09-17, same worklog). A finished task goes to the dormant
    queue and nothing but `Task#close` ever took it out again, while all four queues were GC
    roots: every task a program had ever run stayed live with its result, its name and its queue,
    for the life of the VM. That is what the reference does too — `mrb_task_mark_all` walks all
    four queues, and `task_create_common` additionally `mrb_gc_register`s every task object, so
    there a finished task is pinned twice over and only `mrb_close_task` frees it. On a
    microcontroller running a fixed set of tasks that is invisible; a game that restarts a script
    every few seconds leaks one Task per restart. So this is a deliberate departure: the dormant
    queue is held **weakly**. It is left out of the root set, and after the mark phase — before
    the sweep — a collection drops from it every task nothing else reached (`Vm::gc_collect`).
    Nothing observable changes, because a task dropped this way is one no Ruby code and no
    registered host handle can still name: `Task.list`, `Task.get`, `Task.stat`, `Task#status`
    and `Task#value` answer for every task a program can reach, which is what the reference's own
    tests check (1000 tasks spawned and finished: 612 live objects before and 612 after a
    collection, against 2612 before; `tests/task.rs`). A host that means to read a task back
    after it finished registers it, as `Vm::task_spawn`'s documentation already said.

## How far mruby-task may drift (decided 2026-09-13)

mruby-task is the one gem this VM effectively maintains a fork of: it is in no gembox, it is
young, and its own tests leave whole methods unchecked (nothing calls `Task#join`, and nothing
reads what `Task.pass` or `sleep` return). The ports here are therefore free to improve it — the
author's decision — but "free" needs a boundary, because the reference's 72 assertions
(`gem_task` 43, `gem_queue` 23, `gem_gc_task` 6) are the only outside check that this scheduler is
right. They are kept for that, not out of deference. The rule is **adding is free, breaking
costs**:

1. **Additions are free.** The host entry points (`Vm::task_*`), the external clock, the
   instruction-counted tick, running a fiber inside a task, time limits and `Task::Overrun`
   (raised only under limits a host sets): none of them change what a program written for the
   reference does. Record them in this file and move on.
2. **What the reference's tests cover stays as it is.** Status names, wait reasons, priorities,
   the exception-as-result rule, `Queue`'s answers, the error messages. If one of them looks
   wrong, the move is an issue or a patch upstream (`docs/verification/upstream-pr-candidates.md`), not a
   silent divergence — which is also what the book claims the criterion is.
3. **Where the reference decided nothing, decide here.** No test covers it and no program can
   depend on it, so pick what a Ruby programmer would expect, and write down why. `Task#join`'s
   answer after a real wait and what `Task.current` says inside a fiber are of this kind.
4. **The reference's test files are never edited.** A deliberate difference is counted as one
   (`tests/mrbtest/notes.tsv`), never hidden by changing the assertion. Otherwise the number
   stops measuring anything.

The practical test for a change: *does a program written against the reference behave
differently?* No → free. Yes → rule 2 or 3, and it goes in "Deviations kept" with a reason.

## Gems as Cargo features

A gem can be left out of the build. The rule, as of 2026-09-16, is that **a gem gets a feature
of its own when it drags a crate in behind it** — not merely because it is optional in the
reference's gembox. Exactly one does: mruby-regexp, whose engine is `regex-automata`.

The crate's features, for reference (`Cargo.toml`, and
[`CHANGELOG.md`](../../CHANGELOG.md) for when each arrived):

| feature | default | what it is |
|---|---|---|
| `std` | yes | `std::error::Error` for `VmError`, and nothing else. The library is `no_std` + `alloc` without it |
| `utf8` | yes | a String is a sequence of characters, as `MRB_UTF8_STRING` makes it ([`utf8.md`](utf8.md)) |
| `regexp` | yes | mruby-regexp and `regex-automata`, this section |
| `macros` | **no** | re-exports `#[derive(RubyClass)]` and `#[ruby_methods]` from `sabiruby-macros` at the crate root ([`macros.md`](macros.md), "How a host depends on it") |

`macros` is the one that is not a gem: it adds no Ruby-visible class or method, only a way of
writing the host's own. It is off by default because a proc macro is compiled for the host
machine, and nothing of it reaches the target — `tools/check_no_std.sh` passes with it on.

`regexp` (on by default, `Cargo.toml`). With `default-features = false, features = ["utf8"]`
the crate has no `regex-automata`/`regex-syntax` dependency at all (`cargo tree` shows four
crates instead of six) and the VM is under half the size (`docs/verification/size.md`). What
the build loses, and why each is where the reference puts it:

* `Regexp` and `MatchData` are not defined — not even as empty classes. Naming either is a
  NameError, which is what a reference built without the gem answers, and it is what makes a
  literal `/re/` raise: the compiler emits `Regexp.new(…)` for one (`codegen.c`), so the
  literal fails at the constant. mruby's own test driver links no regexp gem, and its
  `test/t/codegen.rb` asserts exactly that — the one assertion that passes *only* without the
  feature (`tests/mrbtest/baseline-noregexp.txt`).
* `RegexpError` **stays**. It is 15.2.27, defined by mruby's core beside the rest
  (`src/vm.rs`), even though only the gem raises it.
* `String#match`, `match?`, `=~`, `scan` are gone (NoMethodError). In the reference these
  three exist only in `mrbgems/mruby-regexp/src/regexp.c`, so a build without the gem has no
  such methods either.
* `String#sub`, `gsub`, `sub!`, `gsub!`, `split`, `index`, `partition`, `start_with?`, `[]`,
  `[]=` and the rest of the widened set keep their String-pattern meaning, because mruby's
  core mrblib defines them in Ruby and mruby-regexp only *replaces* them. A String pattern
  therefore behaves the same in both builds; a non-String argument raises TypeError from
  `expect_str`, where a build with the gem would have tried to match. Two of them —
  `sub` and `gsub` — are registered by `builtins::string::post_mrblib` after mrblib rather
  than left to mrblib's Ruby versions, which mix character and byte positions and so are
  wrong for a multi-byte string in a `utf8` build; mruby-regexp's own C is the reference's
  fix for the same bug, and this is the same move for a build without it.
* `Symbol#match`, `match?`, `=~` are gone, for the same reason as String's.
* `$~` becomes an ordinary global variable instead of the per-scope one
  (`mrb_gv_define_virtual`): `Vm::s.backref` is `None`, so `OP_GETGV`/`OP_SETGV` read and
  write the globals table. A reference without the gem defines no virtual global either.
* `require "regexp"` raises LoadError. `$LOADED_FEATURES` is a literal in
  `src/mrblib/require.rb`, so `Vm::load_mrblib` takes the name back off it when the feature
  is off — the answer a build that never linked the gem gives.

What stays regardless: `ObjKind::Regexp` and `ObjKind::MatchData` are `#[cfg]`-ed out of the
heap's value enum, `src/regexp/` and `src/builtins/ext_regexp.rs` are not compiled, and
`src/mrblib/regexp.mrb` is not even `include_bytes!`-ed.

Tests: `tools/mrbtest.sh --no-regexp` runs mruby's suite on the feature-off build, leaving
out mruby-regexp's own test files (and only those) and comparing against
`tests/mrbtest/baseline-noregexp.txt`; `tests/custom/*.rb` may carry a `# regexp-only:` header
the way it carries `# utf8-only:`. Every other file passes the same count as in the default
build.

## Compiling the tests

`tools/mrbtest.sh` copies a gem's `test/<file>.rb` as `gem_<file>.rb`; a name an earlier gem
already used is qualified as `gem_<gem>_<file>.rb` (`numeric.rb` of string-ext and numeric-ext,
`range.rb` of range-ext and string-ext, whose second copy had silently replaced the first
until 2026-09-12).

`tools/mrbtest.sh` compiles the test files with `mrbc -g`, which keeps the LVAR section:
`local_variables` and `Proc#parameters` need the names, and the reference driver compiles the
tests from source so it has them too. Reading LVAR uncovered a bug of the loader: the symbol
count of the section is 32-bit (`write_lv_sym_table`), not 16-bit.


## Remaining gems (none, as of 2026-09-13)

The plan is finished: `docs/plans/gems-plan.md` (orders 4 to 7, UTF-8 strings and `require`/`load`) is
marked done, and nothing of it is left. The seven POSIX gems below were never part of it — the VM
is `no_std`, so a host offers what it can through the `Host` trait instead. What comes after the
gems is `docs/plans/after-gems-plan.md`.

The reference `mruby` command is built from `default.gembox` = stdlib, stdlib-ext,
stdlib-io, math, metaprog (33 gems). Ported: fiber, enumerator, array-ext,
enum-ext, hash-ext, range-ext, string-ext, sprintf, metaprog, proc-ext, method,
compar-ext, toplevel-ext, enum-chain, enum-lazy, object-ext, symbol-ext, kernel-ext,
class-ext, numeric-ext, catch, objectspace, math, random, struct, data, set, time, bigint,
rational, complex, pack, eval, binding, proc-binding, regexp (36), plus mruby-cmath,
mruby-task, mruby-sleep and mruby-strftime from outside the gembox. Only the POSIX gems are left.
Sizes are lines of the reference C / mrblib Ruby / test.

What all of that adds up to, class by class and method by method next to the reference's own
list, is [`../verification/coverage.md`](../verification/coverage.md) (generated by
`tools/coverage.sh`; the class-to-gem column there is the one hand-kept copy of the list below).

| order | gem | C / Ruby / test | depends on | notes |
|---|---|---|---|---|
| – | mruby-io, mruby-socket, mruby-errno, mruby-dir, mruby-env, mruby-signal, mruby-process | 3868+1429+334+530+223+108+1320 | POSIX | not planned: the VM is no_std; a host `Host` trait may offer `puts`-level output only. mruby-error and mruby-exit are C API helpers, not needed |

Order: 1 (pure Ruby, done 2026-09-12) → 2 (small natives, done 2026-09-12) → 3 (data structures and host
clocks, done 2026-09-12) → 4 (eval, with the compiler hook) → 5 (numeric tower, pack) → UTF-8 strings
(`docs/plans/utf8-plan.md`, a build-configuration milestone required for Japanese text) →
6 (regexp, on top of UTF-8, done 2026-09-13 — the engine is Rust's, only the surface is ported)
→ 7 (task).
Each gem: natives in `src/builtins/ext_<gem>.rs`, mrblib into `src/mrblib/<gem>.mrb`,
tests into `tools/mrbtest.sh` `GEMS`, reasons for what does not pass into
`docs/verification/mrbtest-notes.md`, and the `Vm::with_mrblib` load order stays the gembox order.

### Core gems outside default.gembox

Candidates, with the reference sizes (C / Ruby / test):

| gem | size | worth it? |
|---|---|---|
| mruby-sleep | 186 / 0 / 29 | **done** (2026-09-13): `Kernel#sleep`/`usleep` outside a task, through `Vm::sleep_hook` |
| mruby-strftime | 118 / 0 / 152 | **done** (2026-09-13): `Time#strftime`, written out rather than handed to `strftime(3)` |
| mruby-string-bitops | 581 / 0 / 210 | maybe: `String#&`, `|`, `^`, `~` on bytes; small, self-contained |
| mruby-os-memsize | 283 / 0 / 63 | maybe: `ObjectSpace.memsize_of`; needs per-object sizes from our heap, answers will differ from the reference (deviation) |
| mruby-encoding | 109 / 0 / 921 | maybe: the default build now reads strings as characters (`docs/design/utf8.md`), so `Encoding`, `String#encoding` and `force_encoding` would have something to say; `String#b` is already here |
| mruby-benchmark | 0 / 130 / 283 | no: pure Ruby but depends on io and process |
| mruby-error, mruby-exit | 143, 82 | no: C API helpers (`mrb_protect`), `exit` is a host decision |
| mruby-test-inline-struct, mruby-test, mruby-bin-* | – | build/test infrastructure, not runtime |

Third-party gems are out of scope until the core list is done; the ones worth a look then
are pure-Ruby or small-C libraries used by PicoRuby (`picoruby-json`, `picoruby-yaml`,
`picoruby-base64`, `picoruby-crc`, `picoruby-markdown`) since their mrblib compiles as is.
