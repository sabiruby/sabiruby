# Why an assertion does not pass

Hand-written companion of the generated [`mrbtest.md`](mrbtest.md) (the default build, strings as
characters) and [`mrbtest-bytes.md`](mrbtest-bytes.md) (without the feature `utf8`). Every
assertion of mruby's `test/t` and of the ported gems that SabiRuby does not pass is listed here
with its reason, and with what the reference `mruby` (4.1.0-rc; 4.1.0-rc2 from 2026-09-23, default gembox, run through the
kit's `reference_runner.rb`) does on the same file. Update this file whenever the table
changes; `tests/mrbtest/notes.tsv` holds the one-line version that fills the `note` column.

Categories:

* **C fixture** — the test calls a native helper that only exists in mruby's `mrbtest`
  binary (`mrbgems/mruby-test/` or a gem's `test/*.c`). Porting the helper is possible but
  it tests mruby internals (REnv slots, `mrb_vformat`, `mrb_sys_fail`), not Ruby semantics.
  The reference `mruby` command fails these too.
* **gem** — needs a gem that is not ported (the POSIX ones).
* **engine** — mruby-regexp's pattern engine is `regex-automata`, not the reference's NFA
  (`docs/design/gems.md`, "Deviations kept"): a pattern using a construct a finite automaton has none of
  is refused with `RegexpError`, and the reference's assertions on it are counted as intended
  differences.
* **deviation** — a difference SabiRuby keeps on purpose (see README).
* **build** — depends on how the reference binary was built.
* **undecided** — a difference nobody has decided to keep or remove yet.

| file | not passing | category | reason |
|---|---|---|---|
| array | 1 crash | C fixture | `Array shared from an emptied heap array keeps a buffer` needs the `AryShared` class of `mruby-test/ary_shared.c`. The reference `mruby` crashes here too (62/63). |
| env | 8 crash | C fixture | Every test calls `__env_svar?`, `__env_len`, `__env_cfunc_proc`, ... from `mruby-test/env.c`, probing REnv slot internals. The reference `mruby` crashes on all 8 too. |
| float | 1 KO (3 assertions) | deviation | `a NaN is the object it is and no other`: SabiRuby Floats are immediates, so two NaNs made separately are `equal?` and have the same `object_id`. mruby with Word Boxing allocates each NaN on the heap. |
| literals | 1 skip | build | `Literals Numerical without Float` skips because Float is defined (the reference skips it too, 11/12). |
| syntax | 1 KO (2 assertions) | deviation | `pattern matching - a key that moves the subject`: a hash pattern whose key's `hash`/`eql?` mutates the subject is not detected (mruby raises RuntimeError from the hash iteration guard). The reference `mruby` command has 3 KO here of its own: tests reading `__FILE__`/`__LINE__` see the concatenated runner script. |
| sysfail | 1 crash | C fixture | `TestSysFail` of `mruby-test/sysfail.c` (`mrb_sys_fail`). Reference `mruby` crashes too. |
| vformat | 1 crash | C fixture | `TestVFormat` of `mruby-test/vformat.c` (`mrb_vformat`). Reference `mruby` crashes too. |
| regexperror | 0 assertions | — | The file's one test is commented out on the reference (`# TODO broken ATM`), so it reports 0 there too. |
| codegen | 1 KO (2 assertions) | gem | `register window of calls (#3783)` expects `/static/` to raise NoMethodError, which holds only where the build has no `Regexp` class. mruby-regexp is not in any gembox, so the reference's `mrbtest` never links it; the reference `mruby` command, which does, answers as SabiRuby does. |
| range | 1 crash | reference too | `Range#last`: `assert_nil (1..).last` in the core test, but mruby-range-ext's Ruby `last` raises RangeError for an endless range. The reference `mruby` (default gembox) crashes on the same assertion (21/22); the two only agree in a build without the gem. |
| gem_array | 1 KO (4 assertions) | deviation | `Array#uniq, Array#- and Array#include? with a NaN`: the reference treats every NaN made as its own object (identity), SabiRuby's Floats are immediates. |
| gem_enum | 1 KO (2 assertions) | deviation | `Array#count with a NaN`: same NaN identity. |
| gem_enum_chain | 1 KO | reference too | `Enumerator::Chain#size`: `[1,2,3].chain(3..4).size` is 5 once mruby-range-ext gives `Range#size`, and the test expects nil. The reference `mruby` (default gembox) fails the same assertion (6/7). |
| hash | 1 KO (the 4 `assert_raise` of 9 assertions; 4.1.0-rc2) | undecided | `Hash lookup with entries deleted by an eql? callback` (new in 4.1.0-rc2, GHSA-2778-fvwg-5m8w) expects `RuntimeError: hash modified` when the key's `eql?` deletes entries mid-lookup. Its key answers `hash` 0, and the reference's small ("array") hash calls `eql?` on every entry whatever the codes are, so the delete happens. SabiRuby compares the cached hash codes first (`HashData::first_candidate`, `src/object.rs`; as CRuby does), no entry hashes to 0, `eql?` never runs and the lookup answers nil (`[]`, `key?`, `[]=` and the 20-entry form alike). Even with a key whose code does match, the lookup finishes (bounded, no out-of-range read) instead of raising: `Vm::hash_get`/`hash_delete` return `Option` and drop the error. `docs/worklog/2026-09-23-rc2.md` has the options. |
| gem_set | 1 KO | deviation | `Set#include? with an element that changes the Set`: the reference raises RuntimeError from the khash rebuild guard (GHSA-4jw6-mq65-g3c8); the Hash-shaped Set here has no such state and finishes the lookup. |
| gem_binding_binding | 1 KO, 2 crash | C fixture | `binding_in_c` and the `__binding_env_*` helpers are `mruby-binding/test/binding.c`; the reference `mruby` command fails them too. |
| gem_proc_binding | 1 crash | C fixture | `proc_in_c` of `mruby-proc-binding/test/binding.c`. |
| gem_pack | 1 KO | deviation | `unpack a NaN that signals`: two unpacked NaNs are the same immediate here, so `equal?` is true. Same NaN identity as `float`. |
| gem_string | 3 skip (9 as bytes) | build | Each build skips what only the other answers, as the reference does: the UTF-8 build skips the three tests written for a byte-string one, and the byte-string build skips `swapcase`/`casecmp?` Unicode and the six `scrub` tests (`UNICODECASE` false). |
| gem_fiber2 | (all pass) | — | Needs the six natives of `mruby-fiber/test/fibertest.c`; SabiRuby provides them in `src/mrbtest.rs`. The reference `mruby` command lacks them and crashes on all 4. |
| gem_array | (C helper) | — | `__unshift_from_c` of `mruby-array-ext/test/array.c` is provided by `src/mrbtest.rs`. |
| gem_sprintf | 1 skip (3 as bytes) | build | `what the string sprintf builds claims` asks for `String#encoding`, which comes with mruby-encoding (not ported, and absent from the reference build too). The two `%c` tests skip themselves in the byte-string build, where `__ENCODING__` is `"ASCII-8BIT"`. |
| gem_proc | 2 crash | C fixture | `ProcExtTest.mrb_proc_new_cfunc_with_env` / `mrb_cfunc_env_get` test the C closure API of `mruby-proc-ext/test/proc.c`; there is no C closure here. The reference `mruby` crashes too. |
| gem_regexp | 1 crash | engine | `Regexp#to_s` folds a leading option group by trial-compiling what it encloses, and the pattern there is `(?=a)`. |
| gem_regexp_syntax | 25 KO, 55 crash | engine | The crashes are lookbehind (18), backreference (11), lookahead (9), `\k` (8), possessive (3), atomic, absent, conditional and one nesting depth. The KO are `\Z`, `^` after a trailing newline, an empty iteration's capture, `/i` over `\w` and over a class the reference closes once, the step and stack limits, and the parser's wording for constructs neither engine compiles. |
| gem_regexp_call | 12 crash, 2 KO | engine | The file is about `\g<…>`, the subexpression call. |
| gem_regexp_utf8 | 5 KO, 6 crash | engine | The crashes are backreference, lookaround and the absent operator. The KO are a byte-read subject (`String#b`) searched with a pattern compiled for characters: the reference decides per search what a byte means, this engine at compile time. |
| gem_string_index | 1 crash | engine | A pattern with a lookbehind. |
| gem_string_regexp | 2 KO, 6 crash | engine | The crashes are lookahead, atomic and possessive; the KO are `^` after a trailing newline and a byte-read subject split by byte. |
| gem_unicode_case | 3 KO, 1 crash | engine | `/i` is Rust's Unicode simple folding, applied to each member of a class rather than to the class its set operations build; the crash is a backreference. |
| gem_unicode_ctype | 4 KO, 1 crash | engine | A nested negated class and a `&&` of two unions compile here where the reference refuses them, `/i` over a negated POSIX bracket folds the other way round, and a byte-read subject is read as characters; the crash is a lookbehind. |
| gem_ascii_case | 1 KO, 1 skip | engine | Byte-string build only (the gem's `spec.build_settings`): `/i` carries Rust's Unicode table in either build, where the reference has none without `MRB_UTF8_STRING`. |
| gem_backtracking_stack | 0 assertions | — | The file carries the helper the other gem test files call; `tools/mrbtest.sh` extracts it into `prelude.rb` and loads it after `assert.rb` for every file, the reference's driver getting it by linking all the files into one program. |
| gem_gc_task | 2 KO | deviation | `GC.scheduler_driven` is there and the scheduler collects from its idle points, but `GC.generational_mode` is always false (the collector marks and sweeps in one go, `docs/design/gc.md`) and both assertions turn on it being on. |
| gem_proc_set_stack | 0 assertions | — | Both tests ask `TaskTest.respond_to?` first and skip themselves: they probe how mruby sizes a task's stack allocation, which a growable vector has no equivalent of. |

4.1.0-rc2 (2026-09-23): the reference's `test/t/hash.rb` gained one assertion (`hash` above) and
nothing else in the suite changed, so the default build is 2344 of 2508, the byte-string one 2267 of
2453 and the one without mruby-regexp 1979 of 2012; every file's `ok` is what it was.

Summary (2026-09-13, after mruby-regexp, mruby-task, the source-location/backtrace work and
mruby-sleep/mruby-strftime): 2507 assertions in the default build, 2344 pass (2452 and 2267
without the feature `utf8`). mruby-sleep (6) and mruby-strftime (17) pass whole.
mruby-task's own files add 72, of which 70 pass (`gem_task` 43/43, `gem_queue` 23/23,
`gem_gc_task` 4/6). The eight assertions that used to skip for want of debug information now run:
`Proc`/`Method`/`UnboundMethod`/`Binding#source_location`, `Proc#inspect`, the two `exception`
tests that read `Exception#backtrace`, and `MRUBY_REVISION`.
Not passing: 100 crashes and 50 KO, of which mruby-regexp's engine accounts for 82 crashes and
39 KO; the rest are the 17 crashes (16 C fixtures, 1 core-test-vs-gem conflict the reference
shares) and 11 KO (NaN identity ×5, the pattern-matching guard, the Set rebuild guard,
`binding_in_c`, `Enumerator::Chain#size`, `codegen`'s two assertions that hold only without a
`Regexp` class, and `gc_task`'s two) of the files above. 19 skips, 0 warnings (one was a bug, see below).
The gem's own test files add 501 assertions (`gem_regexp*`, `gem_match_data`,
`gem_string_regexp`, `gem_string_index`, `gem_symbol_regexp`, `gem_backref_scope`, the
`unicode_*`/`ascii_*` pair for the build that owns it); `gem_backref_scope` (64, `$~` scoping),
`gem_match_data` (53) and `gem_symbol_regexp` (11) pass whole.
The four assertions that skipped for want of mruby-bigint (`array`, `gc`, `integer`,
`literals`) run now; the four gems' own test files add 244 (bigint 29, rational 134,
complex 8 + 81, cmath 21), all passing, and mruby-pack 50 (one KO, the NaN above).

The five `... does not retain ... in the GC arena` assertions of `gc` (`OP_GETIDX`, `OP_GETIDX0` twice each,
`OP_SETIDX`) failed until the collector (`docs/design/gc.md`): they compare `GC.stat[:live]` around 20000 operations after a
`GC.start` and expect a rise under 100. SabiRuby has no arena; the objects are simply collected, so they pass.

## Warnings

`ensure - context - yield and return` reported "no assertion" until 2026-09-11: `return`
from a block inside a lambda, through a method with `ensure`, returned from the wrong frame
(the lambda's captured environment was used as the target instead of the environment of the
frame running the lambda). Fixed in `Vm::op_return_blk` following mruby's `top_proc`; the
file passes 5/5 now. If a warning reappears, the assertions inside the block did not run.
