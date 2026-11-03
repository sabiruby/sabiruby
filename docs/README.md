# SabiRuby documents

Laid out as every repository of the organization is
([`.github/CONTRIBUTING.md`](https://github.com/sabiruby/.github/blob/main/CONTRIBUTING.md)):
`design/` says how it is built, `verification/` how it is checked and measured, `plans/` what
was decided to do and in what order, `worklog/` what happened when. Design and verification are
in English; plans and the worklog are in Japanese.

What changed in each release, with the commit behind every claim, is
[`../CHANGELOG.md`](../CHANGELOG.md) at the root of the repository.

[numbers.md](numbers.md) lists the defaults the VM carries, where each is changed and where it came from (started with 0.7.0).

## design/ — how it is built

| file | what it covers |
|---|---|
| [compatibility.md](design/compatibility.md) | which mruby SabiRuby follows (4.1, `stable`), and the one exception: a clear defect of the reference, recorded as a deviation (author, 2026-10-16) |
| [compiler.md](design/compiler.md) | the reference compiler as a crate (`sabiruby-compiler`), the C shim, `eval`, the `Host` trait |
| [exceptions.md](design/exceptions.md) | unwinding with `Result` instead of `setjmp`/`longjmp`; `break`/`return` through natives |
| [fibers.md](design/fibers.md) | fibers without a second host stack; the native-boundary rule; fibers inside tasks |
| [wait-anywhere.md](design/wait-anywhere.md) | waiting inside a block: the natives that leave a frame instead of running a nested loop (the reference's `mrb_exec_irep`), the table of paths against the reference, why `sleep(n)` inside a boundary raises, what is still a boundary and the error that names it |
| [gc.md](design/gc.md) | stop-the-world mark & sweep, roots, the contract for natives, the scheduler-driven mode, the free hook for `Data` |
| [gems.md](design/gems.md) | every ported gem, what deviates and why, mruby-task in depth (host entry points, time limits, how far the fork may drift), gems as Cargo features |
| [macros.md](design/macros.md) | `sabiruby-macros`: a Rust struct and its `impl` block as a Ruby class — what the two macros generate, the Host Object method, how a host depends on it (the feature `macros`), what it does not cover |
| [inspect.md](design/inspect.md) | snapshots, traces and line numbers: what a debugger or a HUD can read |
| [performance.md](design/performance.md) | the value representation, the `Slot` window, known structural costs (short) |
| [optimizations.md](design/optimizations.md) | (Japanese) the speed-ups of 2026-09-15 one by one — symptom, cause, change, effect, what was dropped — how to measure on this machine, what is still slow |
| [utf8.md](design/utf8.md) | strings as characters (feature `utf8`) and as bytes |
| [playground.md](design/playground.md) | the browser playground: the wasm module's C ABI, the debugger, real-time `sleep` |
| [serde.md](design/serde.md) | `sabiruby-serde`: the data model between serde and `Value`, the error mapping, `Serde<T>` in a `define_fn` signature, declarations written in Ruby and collected into a Rust table (`declare`), and why `JSON` lives there |
| [rbs.md](design/rbs.md) | (Japanese) RBS at the host boundary: the three uses considered, the parsers there are (mruby/edge's, `ruby-rbs`, a hand-written one), the Rust type → RBS type table, why the direction to take is *generating* RBS from `#[ruby_methods]`, and the author's 2026-09-17 decision — no parser in the VM, ship `sig/` |

## verification/ — how it is checked and measured

| file | what it covers |
|---|---|
| [mrbtest.md](verification/mrbtest.md) | mruby's own test suite on SabiRuby, per file (the default, character-string build) |
| [mrbtest-bytes.md](verification/mrbtest-bytes.md) | the same for the byte-string build |
| [mrbtest-noregexp.md](verification/mrbtest-noregexp.md) | the same for the build without mruby-regexp (no feature `regexp`) |
| [mrbtest-notes.md](verification/mrbtest-notes.md) | why an assertion does not pass: every remaining failure with its reason |
| [edge-compare/](verification/edge-compare/README.md) | SabiRuby on mruby `891d1d9d`'s `test/t` counted by mruby/edge's driver, plus a column with SabiRuby's ports of the C test fixtures: the published report (`index.html`, SabiRuby only, linking to mruby/edge's own report), the side-by-side development record with mruby/edge (`compare.html`, not published), the three JSON reports and how to make them again (`tools/edge_compare.sh`) |
| [../compare/](compare/index.html) | (Japanese page) mruby/edge and SabiRuby side by side, point by point, with sources (2026-10-15, updated to SabiRuby 0.8.0 and mruby/edge `e30a5e0` on 2026-10-19); `index.html` is the entry page of the site. Published with `verification/edge-compare/index.html` (at https://sabiruby.github.io/sabiruby/mruby-compatibility/) at https://sabiruby.github.io/sabiruby/ by `.github/workflows/pages.yml` |
| [coverage.md](verification/coverage.md) | every class, module and method SabiRuby has, next to the reference's, with the differences both ways (generated by `tools/coverage.sh`) |
| [bench.md](verification/bench.md) | benchmarks by category against the reference, the baseline and every stage since |
| [size.md](verification/size.md) | code size on thumbv7em and x86_64, and what the feature `regexp` takes off it |
| [rc2-sweep/](verification/rc2-sweep/README.md) | every difference from 4.1.0-rc2 found by calling every method both VMs have with fixed arguments (977,521 calls) and by running 502 programs, grouped by cause with a repro, rc2's, SabiRuby's and CRuby's answers, the verdict under the compatibility policy, the size of a fix and whether the book is touched (2026-10-18; `tools/rc2_sweep/`). The tables are now the re-run after rc2-sweep-fixes (Y6, main `842d8ed`): every A and B cause fixed, every C cause recorded, what is left listed with a verdict and each survey row with the stage and commit that fixed or recorded it. Run again at the end of each stage of a fix and when following a new mruby version (the image is `--image`), then `compare.py diff` against the committed tables; about 3 minutes with Docker, so not on every commit |
| [upstream-pr-candidates.md](verification/upstream-pr-candidates.md) | what the port found in mruby-task that belongs upstream |
| [upstream-issue-task-dormant.md](verification/upstream-issue-task-dormant.md) | item 4 of that list worked out — the issue text, the PR description, what is already reported upstream, and what the gem's README does and does not promise. **A record: nothing was submitted** (author's decision, 2026-09-17) |
| [patches/](verification/patches/) | the patches those drafts propose, as `git format-patch` files. `mruby-task-dormant-weak.patch` holds the dormant queue weakly (mruby 4.1.0-rc) |

## backlog

[backlog.md](backlog.md) — what was decided to wait, one line each: why, when to look again, where the record is.

## plans/ — what was decided, in order

| file | status |
|---|---|
| [compiler-plan.md](plans/compiler-plan.md) | done (2026-09-12): the compiler crate |
| [gc-plan.md](plans/gc-plan.md) | done (2026-09-11) |
| [gems-plan.md](plans/gems-plan.md) | done (2026-09-13): orders 4–7 of the gem work |
| [utf8-plan.md](plans/utf8-plan.md) | done (2026-09-13) |
| [eval-require-plan.md](plans/eval-require-plan.md) | done (2026-09-13); written as a study, kept for the reasoning |
| [after-gems-plan.md](plans/after-gems-plan.md) | done (2026-09-13): rubevy on one VM with tasks, backtraces, `sleep`/`strftime` |
| [playground-plan.md](plans/playground-plan.md) | done (2026-09-12); the playground repository carries the visualizer plan |
| [leftovers-plan.md](plans/leftovers-plan.md) | all ten done (10 on 2026-09-17) — the small items each stage left behind |
| [perf3-plan.md](plans/perf3-plan.md) | 3a and 3b done, 3c/3d/3e dropped with their measurements (2026-09-16): the third round of speed — String, the instruction loop, calls, the 8-byte `Slot` experiment, what is left of `so_lists` |
| [host-scale-plan.md](plans/host-scale-plan.md) | (Japanese) to do (2026-09-20): what only the VM can fix, found while rubevy was made to carry thousands of scripted entities — parked tasks cost O(N) in the scheduler (every wake clones and walks the waiting queue; one turn of one task goes 9.0 → 16.8 µs from 300 to 3000 tasks), an irep is never given back, the host cannot count ireps or ready tasks without a `#[doc(hidden)]` field, instance variables cannot be reached by `Sym`, the two backtraces disagree about a native's name, and `check_no_std.sh` does not look at `sabiruby-serde`. Stages H1–H6, status inside; H2 stops at a report of designs |
| [serde-declare-lines-plan.md](plans/serde-declare-lines-plan.md) | (Japanese) to do (2026-09-21): two things rubevy_games' Factory found in `sabiruby-serde` while writing its data stage — `Options::symbol_keys` does not reach a map's keys, so a table declared with Symbols reads back with Strings, and `Declarations::take` does not remember which line a declaration was on, so a host's cross-declaration checks have no line to name. Stages D1–D3, status inside |
| [upstream-task-plan.md](plans/upstream-task-plan.md) | (Japanese) steps 1–4 done (2026-09-17), step 5 deliberately not: `upstream-pr-candidates.md` item 4 — a finished task is kept for the life of the VM — measured on the reference, patched and written up **as a record, not submitted** |
| [from-mrubyedge-plan.md](plans/from-mrubyedge-plan.md) | items 1 and 3 done (2026-09-16); what mruby/edge does that is worth having — serde, RBS at the boundary, a coverage list, gems as features, `RUBY_ENGINE` |
| [host-bridge-plan.md](plans/host-bridge-plan.md) | done through stage 6a (2026-09-15); 6b is rubevy's and 6c is both. Carries the findings of each stage |
| [wait-anywhere-plan.md](plans/wait-anywhere-plan.md) | done (2026-09-26), stages 1 and 2 (stage 3 undecided): a task can wait inside `instance_exec`/`instance_eval`/`Method#call` (as the reference mruby) and inside block-taking builtins DSLs use; a wait that cannot happen raises instead of passing silently; no slowdown on ordinary paths |
| [edge-compare-plan.md](plans/edge-compare-plan.md) | (Japanese) E1–E4 done (2026-10-15): mruby/edge's compatibility report made for SabiRuby on the same tests, and the page that puts the two side by side |
| [edge-diff-fix-plan.md](plans/edge-diff-fix-plan.md) | (Japanese) 2026-10-15: the differences from mruby `891d1d9d` the comparison found — operators reaching a redefinition, `$!`/bare `raise`, `defined?` paths, the cref and where a `def` lands, the Hash `eql?` case (D5, left to the author), four small ones — then the comparison measured again |
| [follow-master-plan.md](plans/follow-master-plan.md) | (Japanese) 2026-10-16: planned as following mruby master (Hash lookup order, the performance regressions, mrblib/tests/compiler on master); changed the same day to keeping mruby 4.1.0-rc2's behaviour (M3/M4 stopped, the master-only fixes of edge-diff-fix taken back out) |
| [rc2-leftovers-plan.md](plans/rc2-leftovers-plan.md) | (Japanese) 2026-10-16: the five differences from mruby 4.1.0-rc2 that were SabiRuby's own before edge-diff-fix (L1–L5) — where a constant is read from, `attr_*` and `method_defined?`, two messages |
| [rc2-leftovers2-plan.md](plans/rc2-leftovers2-plan.md) | (Japanese) 2026-10-16: four more differences from 4.1.0-rc2 found during L1–L5 (M1–M4) — `Foo::String` on a class, the TypeError of a receiver that is not a class, `Array#[]=` with a Float Range end and its conversion message — and the line for fixing or leaving each |
| [rc2-leftovers3-plan.md](plans/rc2-leftovers3-plan.md) | (Japanese) 2026-10-16: `Array#[]` with a Float Range end and its conversion message, checked against `String#[]` which shared the reading (N1); the two differences already decided, recorded under the compatibility policy — `a[0, nil] = 1`, and the Hash entry vacated by `eql?` (N2) |
| [rc2-leftovers4-plan.md](plans/rc2-leftovers4-plan.md) | (Japanese) 2026-10-16: the Hash entry vacated by `eql?` taken as mruby master answers it (P1, a recorded deviation), and the small differences from 4.1.0-rc2 left over — FrozenError messages, "string not matched", `delete_at` and `MatchData#[]` conversions, a dead `slice!` (P2) |
| [rc2-ivar-plan.md](plans/rc2-ivar-plan.md) | (Japanese) 2026-10-17: only the values 4.1.0-rc2's `obj_iv_p` accepts hold instance variables (Q1, the author's decision to follow rc2), and `Integer(2**70)` (Q2) |
| [rc2-leftovers5-plan.md](plans/rc2-leftovers5-plan.md) | (Japanese) 2026-10-17: what rc2-ivar found left — a multiple assignment that does not spread a non-Array or a Struct (R1), `remove_instance_variable`'s name check (R2), a Regexp's instance variable order (R3), `instance_variable_names` removed (R4) |
| [rc2-leftovers6-plan.md](plans/rc2-leftovers6-plan.md) | (Japanese) 2026-10-17: the Array methods read an argument with `mrb_array_p`, so a Struct or a Data is not an Array to them (S1); the natives `ext_array` and `ext_metaprog` override removed, `coverage.md` regenerated (S2) |
| [rc2-leftovers7-plan.md](plans/rc2-leftovers7-plan.md) | (Japanese) 2026-10-17: the small differences left, checked against the compatibility policy (the table: [rc2-leftovers7-check.md](plans/rc2-leftovers7-check.md)) — follow rc2 (T1), rc2's clear defects taken from master and recorded (T2), the author's three decisions (T3), the unused natives and `FromRuby for Vec<T>` (T4) |
| [rc2-leftovers8-plan.md](plans/rc2-leftovers8-plan.md) | (Japanese) 2026-10-17: the four left by rc2-leftovers7 — `module_function` in Class, `String#hex`/`oct`, the Float/Integer methods that take no argument (U1); `sort`'s order recorded, not followed (U2) |
| [rc2-leftovers9-plan.md](plans/rc2-leftovers9-plan.md) | (Japanese) 2026-10-17: the five left by rc2-leftovers8 — the number methods rc2 does not have, native `arity` (estimated, stopped), the argument-count message of `OP_ENTER`, the CLI's report of an uncaught exception (V1); `Integer#infinite?` recorded, not followed (V2) |
| [rc2-leftovers10-plan.md](plans/rc2-leftovers10-plan.md) | (Japanese) 2026-10-17: natives declare their argument spec as rc2 does (`Aspec`, checked before the native runs, `arity` from it) (W1); the "given" count with keywords recorded, not followed (W2); `define_method`'s block strict (W3); `Integer.new` and the other undefined singleton `new`s (W4) |
| [rc2-sweep-plan.md](plans/rc2-sweep-plan.md) | (Japanese) 2026-10-18, a survey only: every difference from rc2 at once (X1 the methods, X2 the programs), as a finite table for the author to draw the line of what to fix; result in `verification/rc2-sweep/` |
| [rc2-sweep-fixes-plan.md](plans/rc2-sweep-fixes-plan.md) | (Japanese) 2026-10-18, the author's line after the sweep: fix every A and B cause, record the C ones, the five open causes decided, method owners moved to rc2's; first the sweep tools made reusable (the image as an argument, a diff against the last run) |
| [regexp-engine-plan.md](plans/regexp-engine-plan.md) | proposed (2026-10-19): mruby-regexp's engine ported from the reference (literal, Pike VM and backtracking) in place of regex-automata — lookaround, backreferences, `\g`, atomic groups as rc2 answers |
| [posix-gems-plan.md](plans/posix-gems-plan.md) | proposed (2026-10-19): the seven POSIX gems (io, socket, errno, dir, env, signal, process) as a separate, optional crate, outside the no_std VM |
| [checkpoint-plan.md](plans/checkpoint-plan.md) | (Japanese) in 0.9.0 (2026-11-03, the author's word to take the whole branch): the feature `checkpoint`, off in the VM crate and on in the `sabiruby` command — resume a running program's state (objects, frames, registers, Tasks and Fibers) written by C mruby with mruby-checkpoint (format v1), from the instruction after where it stopped; one way (C mruby → SabiRuby) |
| [release-0.9-plan.md](plans/release-0.9-plan.md) | (Japanese) R1-R5 done (2026-11-03): 0.9.0 published — the checkpoint branch merged, `checkpoint` on by default in the CLI, the CLI's output as it is written; R6 done (2026-11-03): rubevy 0.4.0 on main (its crates.io publish and tag left to the author), the playground and the games on 0.9.0, both Pages deployed |
| [release-0.8-plan.md](plans/release-0.8-plan.md) | done (2026-10-19; published as 0.8.0): 0.8.0 after the rc2 sweep — versions, tidying, downstream check, bench against 0.7.0, publishing after the author's word |
| [release-0.7-plan.md](plans/release-0.7-plan.md) | done (2026-09-27; published as 0.7.0): what 0.7.0 takes besides waiting inside blocks — the Hash `eql?` exception, `method_missing` via `funcall`, rc2's two checks, irep reclaim (H2), native names in `Exception#backtrace` (H5), waiting-task cost (H1), tidying |

## worklog/ — what happened, when

One file per piece of work, `YYYY-MM-DD-<slug>.md`, written while the work happened. The five
of 2026-09-15 are the records of `host-bridge-plan.md`'s stages 2, 2b, 2c, 4, 5, 3b and 2d
([stage3b-host-entry-points](worklog/2026-09-15-stage3b-host-entry-points.md): the entry points
that let rubevy stop reaching into `Vm`'s fields;
[stage2d-perf](worklog/2026-09-15-stage2d-perf.md): what a small Hash lookup is made of, the O(n)
hiding in `hash_sync`, and cutting `vm_optimization_bench` into five), and
[stage6a-macros](worklog/2026-09-15-stage6a-macros.md) is stage 6a: why the host's stores sit in
the `Vm`, how a method that is given the `&mut Vm` borrows its receiver, and what the macros read
out of a signature.
[2026-09-16-perf3](worklog/2026-09-16-perf3.md) is the third round of speed: what `ds_string`,
`call_args` and `bm_so_lists` are actually made of when split by operation, the four changes that were
kept and the four that were measured and dropped, why `codegen-units = 16` and not "code layout" is
half of this machine's ±15% swing (`96221fb` made one unit the default; see
[2026-09-17-release-prep](worklog/2026-09-17-release-prep.md)), and the estimate that says an 8-byte
`Slot` is not the next thing to do.
[getidx-dispatch](worklog/2026-09-15-getidx-dispatch.md) is the same move for the three index
opcodes: the reference's fast paths for Array, Hash and String, the six slots that say when one
of them may answer, why `p` in mruby-print made a test look wrong, and what the shape of an
opcode's arm costs in the instruction loop.
[serde](worklog/2026-09-16-serde.md) is `from-mrubyedge-plan.md`'s item 1: why serde needs an
error type of its own, why `Serde<T>` can implement `IntoRuby` or `IntoRubyRet` but not both,
and the one line of CRuby's JSON output that caught `serde_json::Map` sorting its keys.
[rbs-study](worklog/2026-09-16-rbs-study.md) is item 2, a study rather than a change: what the
one build of `ruby-rbs` said (libclang), that mruby/edge's `.rbs` is not RBS at all, and the
asymmetry between checking a signature and generating one when all the macro has is a spelling.

[leftovers](worklog/2026-09-16-leftovers.md) is `leftovers-plan.md`'s items 1–4, 8 and 9: why an
empty keyword Hash is something mruby carries rather than something it drops, the one place
`send` and `method_missing` do not lay out a frame the same way, the reference's own wording for
an object that has no Rust value behind it, the block a `#[ruby_methods]` method can now take,
the three entry points rubevy was reaching through `funcall`, and the eight methods and hooks
the coverage list said were missing.

[leftovers-perf](worklog/2026-09-16-leftovers-perf.md) is the same plan's items 5 and 6: that
`printf` and `putc` come from mruby-io and not from a gem called mruby-print, what the reference
means by "the first character" of a string (the build's answer, not the string's), the benchmark
set growing to twenty-seven files, why copying an array is not worth removing when the answer is
the whole array, and how a micro measurement reads only once the same round's empty loop has been
subtracted from it.

[regexp-feature](worklog/2026-09-16-regexp-feature.md) is the same plan's item 4: what a build
without mruby-regexp must answer where the reference has no gem to ask, the `sub`/`gsub` that
had been dead code all along, and the assertion that only passes with the feature off.
[2026-09-17-release-prep](worklog/2026-09-17-release-prep.md) is the release profile: what
`codegen-units = 1` is worth when the same tree is built twice and measured against itself, why
one unit makes the binary 17% smaller and the release build twice as slow in wall clock while
using a third less CPU, the baseline that was thrown away because this session's own polling
loops were the noise, and why `lto` and `panic = "abort"` were left unmeasured.

[2026-09-17-release-0.5](worklog/2026-09-17-release-0.5.md) is the 0.5.0 preparation: why the
`macros` feature needs no change to what the macros generate, why `sabiruby-compiler` goes to
0.2.2 for its dependency alone and not for its content, why a dev-dependency must not carry a
version, what a breaking change looks like when it appears only in a signature diff, and which
of the publish dry-runs can pass before anything is on crates.io.

[2026-09-17-visibility](worklog/2026-09-17-visibility.md) is `leftovers-plan.md`'s item 10: the
three places mruby sets `MRB_METHOD_PRIVATE_FL`, why its "`initialize` is always private" rule
does not reach the built-in tables (and so `Struct#initialize` is public there), the one frame in
a context that starts out private and what that makes of a top-level `def`, and the two things
that turned up because of it — `Module#define_method` writes its own visibility, and
`respond_to?` never looks at visibility at all.

[2026-09-17-upstream-record](worklog/2026-09-17-upstream-record.md) is the same defect taken back to
the reference, as a record rather than a submission: what the leak costs there in dormant tasks,
live objects and RSS, the 65 500-spawn ceiling where it stops being a leak and becomes a
`RuntimeError`, what the gem's README promises instead, why the obvious patch is a use-after-free
in C (an mruby data type has no mark hook) and what has to go with it, and the merged PR that
added `Task#close` because of this very leak.

[2026-09-17-task-end-nil](worklog/2026-09-17-task-end-nil.md) is the two defects rubevy's garden
demo found in mruby-task's scheduler: why a task that ends with a nil result used to cost a whole
turn of the host loop (and why ten scripts restarted at once therefore froze every other script
for sixty frames), why the reference's own dormant queue keeps every finished task for the life
of the VM and what the smallest departure from it is, and the case for reading `task_run_once`'s
nil as nothing more than a value.

[stage6c-method-missing](worklog/2026-09-15-stage6c-method-missing.md) is the VM's half of stage
6c: reading `prepare_missing` in the reference, why packing the arguments into one Array and
shifting them along one register are the same thing to `OP_ENTER`, and what a `method_missing`
that runs in the caller's frame can do that one in a nested run loop cannot.

[2026-09-18-highlight](worklog/2026-09-18-highlight.md) is `highlight()`, the category byte per
source byte an editor colours Ruby with (stage H0 of rubevy_games' `editor-highlight-plan.md`):
what family-mruby's `picoruby-syntax-highlight` is made of and which of its three parts was
deliberately left behind, why dropping its AST pass paints fewer method names and no wrong one,
how the name of a method is read off the token before it when there is no tree to ask, what
Prism 1.9.0 actually answers for interpolation, a regular expression, a `key:` label, a
Japanese comment and a source that does not parse (measured before any expectation was written
down), and the 64345 bytes of the garden's Ruby that say family-mruby's 32 KiB cap belongs to
an ESP32's heap and not here — so there is no cap.

[2026-09-18-release-0.5.2](worklog/2026-09-18-release-0.5.2.md) is the publishing of that
release: why only two of the five crates went out (`git diff --stat v0.5.1..HEAD` names
`compiler/` and one line of the root `Cargo.toml`, nothing else), why the compiler could go
before the VM this time, the commands and what they printed, and who is unaffected because
they depend on the repository and not on crates.io (rubevy, rubevy_games, the playground).

[2026-09-20-declare](worklog/2026-09-20-declare.md) is `sabiruby_serde::declare`, rubevy's
`generalize-plan.md` stage R7 on this side of the fence (Japanese): why a table of
declarations goes in the type's `HostStore` and not in `Vm::set_host_state` or behind a lock
this crate has none of, what makes a slab meant for `Data` objects a fair place for a value no
`Data` object names and what that costs, why `take` borrows the table for good instead of
removing it, why what a script reads back is a second table rather than the one being filled,
and the four shapes that were tried and dropped.

[2026-09-21-serde-lines](worklog/2026-09-21-serde-lines.md) is
`serde-declare-lines-plan.md`'s D1–D3, the two things rubevy_games' Factory found in that crate
while writing its data stage (Japanese): the two measurements taken before anything was written
— that the interner never gives a symbol back (1000 names from Rust and 2000 from Ruby survive a
collection intact), which is why a map's keys get a switch of their own rather than riding on
`symbol_keys`, and that `Vm::next_line` answers with the line of the
*next* instruction, so a native that asks it where it was called from is told the line after;
why that is not a bug to fix but a second question (the playground's stepper wants exactly that
answer) and what the three ways of doing without a new entry point would each have cost; and why
"a key written as a string" is tested on the value that came out rather than on the Rust type.
Its last sections are the rename that followed — `current_line` → `next_line`, which is why
everything above them says `current_line` — and the day the alias it left behind was removed.

[2026-09-22-release-0.6](worklog/2026-09-22-release-0.6.md) is the 0.6.0 preparation
(Japanese): which four of the five crates go out and why `sabiruby-macros` does not, why
`sabiruby-compiler` jumps to 0.3.0 rather than 0.2.4 although not a byte of it moved (with the
feature `host` on, `sabiruby` is a *public* dependency, so raising its requirement breaks the
compiler's own users too), every place a version requirement had to move with it, the order the
`--workspace` dry-run chose and which three crates cannot be dry-run until the VM is on
crates.io, and the three repositories that use it — built from copies, which is where the
warning in `2026-09-21-serde-lines.md` §7.6 was confirmed: a `[patch.crates-io]` whose crate no
longer satisfies the requirement is not an error but two VMs in one graph.

[2026-09-22-readmes](worklog/2026-09-22-readmes.md) is 0.6.1, the release that changed no `.rs`
(Japanese): what each of the five crates.io pages claimed and what the tree actually says,
claim by claim — a `Status` heading two releases old, a CLI switch that had been implemented
for nine days, golden-test counts of 17 and 61 where there are 18 and 112, benchmark ratios one
measurement behind, and two crates whose one-line `description` did not name the thing their
last release was for; the three ways a README was made to go stale more slowly (no version or
date in a heading, a measured number carrying the file it came from, and a machine's seconds
dropped rather than re-measured); and why `2026-09-22-release-0.6.md` §10-1 is wrong —
`CHANGELOG.md` has shipped inside the `sabiruby` tarball since `861b133`, and what crates.io
does not render is not what `include` decides.

[2026-09-23-rc2](worklog/2026-09-23-rc2.md) moves the reference from mruby 4.1.0-rc to 4.1.0-rc2
(Japanese): the recipe of the UTF-8 reference image written down (`tools/utf8-image/`), every
`.mrb` byte-identical under rc2's `mrbc` except `hash.mrb`, and why rc2's one new assertion
(an `eql?` that deletes entries mid-lookup must raise) does not pass here — the reference's small
hash calls `eql?` on every entry, SabiRuby compares hash codes first — with the options left open.

[2026-09-26-wait-anywhere](worklog/2026-09-26-wait-anywhere.md) is `wait-anywhere-plan.md`
(Japanese): every place a native calls back into Ruby, sorted into blocks and implicit callbacks;
`instance_exec` and its relatives, `Method#call`, `public_send`, `Class#new` and the string `eval`s
leaving a frame where the SEND's value goes instead of running a nested loop (the reference's
`mrb_exec_irep`); `sleep(n)` inside a boundary raising instead of returning at once; and the error
that names the boundary, worked out from the frame below it only when it is raised; then the
measurements: the A/A that set the line, the Ruby loops that were 2.5–3 times slower and the native
loop frames that replaced them, `Cci::KeepSelf`, and the one path left over the line (`sort { }`, +8%).

[2026-09-26-release-0.7-a](worklog/2026-09-26-release-0.7-a.md) is stage A of
`release-0.7-plan.md` (Japanese): Hash lookups that dropped what `eql?`/`hash` raised
(`Vm::hash_get`/`hash_delete` now return `VmResult`, and an `eql?` that changes the Hash raises
"hash modified" as rc2 does), which of the rc2 worklog's three options was taken and why, the
20-entry case where the reference's hash table never meets a key that claims a String's hash; a
closure `method_missing` reached through `funcall` that could yield a fiber from under a native;
rc2's two loader/VM checks with images forged as the reference's bintest forges them; and the
tidying (the no_std check now covers `sabiruby-serde`, version numbers kept out of `docs/design`).

[2026-09-26-release-0.7-b](worklog/2026-09-26-release-0.7-b.md) is stage B of `release-0.7-plan.md`
(Japanese): the natives in `Exception#backtrace` as the reference's C frames are, found where they
left a trace (the native that raised, the frame a native's block runs in, a native loop frame), and
what is still different; the scheduler's queues as ordered maps so that a waiting task costs
nothing per turn, with the walked elements counted before and after; and handing a program's
ireps back (`Vm::unload`) where nothing can still run them.

[2026-09-26-release-0.7-c](worklog/2026-09-26-release-0.7-c.md) is stage C of `release-0.7-plan.md`
(Japanese): `Enumerable#hash`, which needed two natives the core mrblib calls, and the nil-block
error that `check_block` gives; merging stage B into stage A (no conflict in the VM; what the merge
had to be checked for), the `native_mid` of a closure `method_missing` reached by a SEND, the
decision that unloaded irep numbers are never reused, and every check on the merged tree.

[2026-09-27-release-0.7-bench](worklog/2026-09-27-release-0.7-bench.md) measures the 0.7.0
candidate (Japanese): the A/A line, seven task-heavy micro-benchmarks (`bench/micro-tasks/`), the
`BTreeMap` queues that made a few tasks 10–31% slower and the sorted `VecDeque` that replaced them,
what is still over the line, the slowdown of paths without tasks that begins at the merge of stage B
(cause not found), and rubevy's two instruments against the candidate.

[2026-10-15-edge-compare](worklog/2026-10-15-edge-compare.md) is `edge-compare-plan.md`
(Japanese): mruby/edge's compatibility driver read and run on SabiRuby unchanged (the three
natives it needs, in the command and not in the VM), the reference `mrbc` of `891d1d9d` built as
an image, mruby/edge's own report run on the same PC (in a container, for libclang) and found
identical to its published page, the page that puts the two side by side, and every assert
SabiRuby does not pass, sorted into C fixtures, what already failed with rc2's tests and what
is new in `891d1d9d`.

[2026-10-15-edge-diff-fix](worklog/2026-10-15-edge-diff-fix.md) is `edge-diff-fix-plan.md`
(Japanese): each difference checked on the reference `891d1d9d` and traced to the commit that
made it — `bop_redefined` for the arithmetic opcodes and the one compare that guards them, `$!`
set by the new compiler and read by a bare `raise`, the two new `defined?` helpers, the keyword
register of a 15-parameter method, the port of the reference's cref model (`MRB_PROC_CREF`,
`MRB_PROC_GIVEN`, the class a `def` adds to, the constant walk), the visibility `attr_*` and
`define_method` take — why the Hash `eql?` tests need the reference's lookup order and the three
ways to answer that, the A/B numbers, and the comparison measured again.

[2026-10-16-follow-master](worklog/2026-10-16-follow-master.md) is `follow-master-plan.md`
(Japanese): the base moved to mruby master and then back to 4.1.0-rc2 on the author's decision; every
fix of edge-diff-fix checked on 4.1.0-rc2 and `891d1d9d` with the master commit that changed it, the
ones taken back out and the ones kept; the Hash and the Set searching in the reference's order
(`src/hashtable.rs`: the entry array, AR and HT, the index buckets, `H_CHECK_MODIFIED`, khash) and what
it cost; the performance regressions' three hypotheses; the comparison measured again with a column
that has the C fixtures.

[2026-10-16-rc2-leftovers](worklog/2026-10-16-rc2-leftovers.md) is `rc2-leftovers-plan.md`
(Japanese): a constant read from the running proc as `mrb_vm_const_get` reads it (and the block
inside a module's method that sees the including class's ancestors), `attr_*` as
`mod_attr_define`, `method_defined?` as `mrb_obj_respond_to`, a class in a NameError spelled as
`Module#to_s`, `Array#[]=`'s three messages; the first shape that cost 10% on a constant in a loop,
the frame flag that took it back, and the benchmarks that move by a few percent without a single
constant in them.

[2026-10-16-rc2-leftovers2](worklog/2026-10-16-rc2-leftovers2.md) is `rc2-leftovers2-plan.md`
(Japanese): `X::Y` and `const_get` stopping before Object as `const_get(..., skip = FALSE)` does
(and the grep that found no script relying on the old answer), `mod_const_check`'s and
`check_if_class_or_module`'s messages, `mrb_as_int` for `Array#[]=`'s Range ends and indexes, the
reference's `aget_index` that converts the first argument for the length, and what was left as it
is.

[2026-10-16-rc2-leftovers3](worklog/2026-10-16-rc2-leftovers3.md) is `rc2-leftovers3-plan.md`
(Japanese): `Array#[]` read as `mrb_ary_aget` (a Float index is a C cast, the length converted
first) and split from `String#[]`, which reads as `str_convert_range` (`mrb_as_int` for both);
`mrb_float_to_integer`'s RangeErrors; `a[0, nil] = 1` recorded as a deviation; the Hash entry
vacated by `eql?`, where master raises and CRuby reads no match, left to the author.

[2026-10-17-rc2-ivar](worklog/2026-10-17-rc2-ivar.md) is `rc2-ivar-plan.md` (Japanese): which
values hold instance variables, type by type against 4.1.0-rc2 (`obj_iv_p` and every function that
asks it), how SabiRuby's kinds map onto the reference's types (a Set is a Hash, a Random and a
Rational are plain objects here), the grep of rubevy, the games and the playground, the A/B of
GETIV/SETIV; `Integer()` and `Float()` read as `mrb_ensure_integer_type` / `mrb_ensure_float_type`.

[2026-10-17-rc2-leftovers5](worklog/2026-10-17-rc2-leftovers5.md) is `rc2-leftovers5-plan.md`
(Japanese): OP_AREF, OP_APOST and OP_ENTER read with `mrb_array_p`, so a Struct is one value to a
multiple assignment and to a block's parameters (`HeapObject::tt_struct`); the name check of
`remove_instance_variable`; why a Regexp lists `@flags` first (`iv_tbl` sorted by symbol); the grep
before removing `instance_variable_names`.

[2026-10-17-rc2-leftovers6](worklog/2026-10-17-rc2-leftovers6.md) is `rc2-leftovers6-plan.md`
(Japanese): every place an Array method, `Kernel`, `Hash[]`, `puts` and the splat ask "is this an
Array", against 4.1.0-rc2's `mrb_array_p`, `mrb_ensure_array_type`, `mrb_check_array_type` (no
`to_ary`) and `mrb_check_type`, with the table of rc2's answers; the overridden natives of
`array.rs` and `object.rs` removed.

[2026-10-17-rc2-leftovers7](worklog/2026-10-17-rc2-leftovers7.md) is `rc2-leftovers7-plan.md`
(Japanese): `Vm::expect_int` made `mrb_as_int` and the 25 conversions that differed in value, each
against its rc2 function; the Float index and `rand` limit that rc2's C cast leaves undefined
(master e2148c1b1); top-level `private`, which was missing; the frozen singleton class whose
`extend` went through; the 83 unused natives and how a method-table snapshot in four feature sets
showed them unused.

[2026-10-17-rc2-leftovers8](worklog/2026-10-17-rc2-leftovers8.md) is `rc2-leftovers8-plan.md`
(Japanese): `module_function` undefined in Class; `String#hex`/`oct` on the scanner of `to_i`,
and the 3000 random strings that found where the old one differed (an underscore after the sign);
the 21 Float and Integer methods of `MRB_ARGS_NONE` that took an argument; `sort`'s order of equal
elements against rc2's insertion and heap sort, recorded.

[2026-10-17-rc2-leftovers9](worklog/2026-10-17-rc2-leftovers9.md) is `rc2-leftovers9-plan.md`
(Japanese): the seven number methods rc2 does not have, found by what can be called rather than
where it is defined, and the table of the other classes; rc2's arity of a C function from its
`MRB_ARGS_*` and the 612 natives that differ (stopped); `argnum_error`'s m1+m2 and the "given"
count left to the author; the CLI's `trace (most recent call last):` checked on 21 programs;
`Integer#infinite?` recorded.

[2026-10-17-rc2-leftovers10](worklog/2026-10-17-rc2-leftovers10.md) is `rc2-leftovers10-plan.md`
(Japanese): the argument specs collected from rc2's own VM (a 50-line gem and `tools/rc2_aspec.sh`)
rather than read from its sources; the 948 definitions matched to them through numbered
placeholders and the owner each class reaches; 358 `argc!` gone; arity 612 → 0 differences of
1098, wrong-count errors 989 of 989 the same; why `funcall` checks too; `define_method`'s strict
copy; the undefined singleton `new`s.

[2026-10-18-rc2-sweep](worklog/2026-10-18-rc2-sweep.md) is `rc2-sweep-plan.md` (Japanese): the
method sweep's call table and its size, what was left out, the time and memory limits; the rules that
group the differing calls by cause, each checked on a one-line program; why rc2 counts after converting
when the arguments come through a splat (`argc-splat`); the programs and mruby's tests assertion by
assertion; where the classifying stopped.

[2026-10-18-rc2-sweep-fixes](worklog/2026-10-18-rc2-sweep-fixes.md) is `rc2-sweep-fixes-plan.md`
(Japanese). Y0: the sweep's reference image as an argument, `compare.py diff` against the committed
tables, the tool's numbers in `numbers.md`; the per-call limit that counted the reference's reading
of a large chunk; two runs on an unchanged main and what moves between them (the current time, a
wall-clock program, rc2's addresses and uninitialised memory in the example columns). Y6: the re-run
after Y1-Y5 against the survey, the C causes made k through `RECORDED`, a verdict for every row left
(A 7, C not recorded 4, ? 1), `(1..)` run again, `stdout_diff_lines` for lines put right behind
another.

[2026-10-19-release-0.8](worklog/2026-10-19-release-0.8.md) is stages R1 and R2 of
`release-0.8-plan.md` (Japanese): the versions (sabiruby 0.8.0, compiler 0.5.0, serde 0.4.0, cli
0.8.0, macros stays), the CHANGELOG cut with its summary and the date left for publishing, the two
records of the author's decisions (`vis-prepended-copy`, `float-remainder-precision`), the numbers
in README checked, the dry-runs in dependency order, rubevy, the garden page and the playground
built against 0.8 on copies (nothing breaks; the second VM a stale lock brings), and the commands
for publishing.

[2026-10-19-release-0.8-bench](worklog/2026-10-19-release-0.8-bench.md) is stage R3 of
`release-0.8-plan.md` (Japanese): `v0.7.0` against the 0.8.0 candidate `e7a3381` on the three sets
(`bench/` −8.5%, `bench/micro-wait/` −7.5%, `bench/micro-tasks/` −6.4%, with the A/A line), instruction
counts the same in all 55, `vmo_objects` the one on the slow side and within its A/A, and the ratio to
the reference taken again the way `96221fb`'s was (2.28x, median 2.70x; one run thrown away).

[2026-10-19-edge-compare-0.8](worklog/2026-10-19-edge-compare-0.8.md) brings `compare/mrubyedge.html`
to 0.8.0 (Japanese): mruby/edge nine commits on (`e30a5e0`, 2.1.2; its report run again, the same
counts) and SabiRuby's `tools/edge_compare.sh` on the committed tree (the same counts); the wasm sizes
taken under the conditions of 2026-09-12, checked by rebuilding 1.1.12 and 0.2.0 within 0.2% (the
numbers of 2026-09-13 do not come back); the speed ratio written as a number only (the author,
2026-10-19); the two VMs timed on 12 of the 27 benchmarks, kept here only and not on the page (the
author: the runner for mruby/edge was ours).

[2026-11-03-release-0.9](worklog/2026-11-03-release-0.9.md) is stages R1-R3 of `release-0.9-plan.md`
(Japanese): `feature/checkpoint` merged (one conflict in this index), the versions (sabiruby 0.9.0,
compiler 0.6.0 and serde 0.5.0 with no change of their own, cli 0.9.0, macros stays), `checkpoint` on
by default in the CLI and what that does to the workspace's test count (282 on, 276 off), the checks
(custom, GC stress, no_std, wasm, CI's doc steps, mrbtest), rubevy and the playground built against 0.9
on copies, the dry runs in dependency order, and the commands for publishing.

## Where things were (before 2026-09-15)

Every document above sat directly under `docs/`; references in older commits, in the book's notes
and in the worklog use those paths (`docs/gems.md` is now `docs/design/gems.md`, and so on).
