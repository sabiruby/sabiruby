# Waiting inside a block

A task waits — `Task::Queue#pop`, `sleep`, `sleep(n)`, `Task.pass` — by going back to the
scheduler with its frames left where they are. It cannot do that while a native is on the host
stack between it and the scheduler: a native that calls a block or a Ruby method back runs a
nested instruction loop (`Vm::call_proc_with`, a frame marked `Cci::Skip`), and the native's own
Rust frames cannot be kept while the task is away. That frame is the *native boundary*
([`fibers.md`](fibers.md), "Native boundaries"; mruby's `ci->cci > CINFO_NONE`).

A host that runs scripts as tasks — a DSL whose steps wait for the host's answer — meets the
boundary inside the blocks the DSL takes. This is what SabiRuby does about it
([`../plans/wait-anywhere-plan.md`](../plans/wait-anywhere-plan.md), stages 1 and 2; the record
of the work is [`../worklog/2026-09-26-wait-anywhere.md`](../worklog/2026-09-26-wait-anywhere.md)).

## Leaving a frame instead of running a loop

mruby 4.1.0-rc2 keeps `instance_exec`, `instance_eval`, `class_exec`, `class_eval`,
`module_eval`, `Method#call`, `bind_call`, `send` and the string `eval`s off the boundary: called
from the VM (`ci->cci == CINFO_NONE`), they do not run the block themselves but rewrite the
calling frame into the block's and return to the VM (`exec_irep` / `mrb_exec_irep`, src/vm.c
1601-1663; `mrb_object_exec` 1665; `eval_under` 1832; `send_method` 1688;
mruby-method `mcall` → `mrb_exec_irep`, method.c:278; mruby-eval `eval_irep`, eval.c:153).
`Class#new` and `catch` are methods written in bytecode there (`new_iseq`, src/class.c:4596;
`catch_iseq`, mruby-catch/src/catch.c:20), so their `initialize` and their block are ordinary
frames. Iterators with a block written in C (`Array#index`, `sort`, `Array.new`) are
boundaries there.

A SabiRuby native has no frame of its own — a SEND calls it straight from the instruction loop
and writes what it returns into the call's register. So the same move is: **push a frame whose
R0 is that register, and return R0.** The SEND writes the value R0 already holds, the loop reads
its next instruction from the frame on top — the one just pushed — and that frame, an ordinary
`Cci::None` one, returns into the caller's register like any method. Nothing of the native is
left on the host stack.

| piece | what it does |
|---|---|
| `Vm::in_frame` | the running native was called by a SEND of the running frame (`direct_send`; mruby `cci == CINFO_NONE`). Cleared for the length of every run loop (`run_loop_ctx`), so a native reached any other way — a nested loop, `funcall`, the inline natives of the index opcodes — sees false |
| `Vm::exec_proc` | `mrb_exec_irep`: in a frame when `in_frame`, nested (`call_proc_with`) otherwise. The frame's target class and `mid` are chosen as the nested call chooses them |
| `exec_block`, `exec_block_with_self`, `exec_method_proc`, `run_eval` | `call_block`, `call_block_with_self_kw`, `call_method_proc` and the `eval` runner on top of it |
| `Vm::send_in_frame` | `send_method`: a Ruby method or a Ruby `method_missing` gets a frame; a native is called with `in_frame` kept, so it can do the same in turn |
| `Cci::KeepSelf` | a frame that answers its R0 — its receiver — whatever it returns, unless a `break` ends it (`BreakTag::BlockBreak` carries that through an `ensure`). `Class#new` pushes `initialize` so, and `Class.new { }`, `Module.new { }`, `Struct.new { }` and `Data.define { }` their block: the tail of the reference's `new_iseq`, without a frame of its own |
| `Vm::push_return_frame` | a frame of one instruction, `OP_RETURN R0`, holding a value; what is pushed above it answers into its R1 and is dropped. Only for a native `initialize` given a block (`Array.new(n) { }` through `Class#new`) |
| `Vm::push_loop_frame` | a native that calls its block again and again — `index { }`, `rindex { }`, `Array.new(n) { }`, `catch { }` — leaves the loop to a frame that runs one `OP_DEBUG`: each time it runs, the native's step function (`builtins::array::loop_step`) reads the loop's state from the frame's registers and either calls the block in a frame above (its value lands in the register `LOOP_RESULT`) or answers. Step for step it does what the native's own loop does: the same elements, the array read again where the native reads it again. Not called by a SEND, the same frame runs in a nested loop; `throw` finds `catch`'s either way |
| `OP_GETIDX` | a Hash that misses and has a default proc: the proc's frame is pushed where the instruction's value goes |

**The rule for a native that uses these:** it must be the native the SEND called, and it must
return what they return, as it is, with nothing after. A native that another native calls as a
Rust function to go on with its answer would break it — `Hash#values_at` calling `hash_aref`
did, until `hash_aref` got a separate entry for such callers (`hash_value`, which runs a default
proc nested). Outside these conditions each of them falls back to the nested call, which is
always correct and only keeps the boundary.

What it costs where nothing waits: the tail of every native call is unchanged; a native that
takes a block reads `direct_send` once more. Measured against the version before
([`../verification/bench.md`](../verification/bench.md), "Waiting inside blocks"): the 27 benchmarks
together move +0.9% and −0.7% in two rounds, inside the spread of the old binary against itself.
The paths that no longer run a nested loop are 5–25% faster. `sort { }` in a loop frame was 8%
slower with a light block — one more trip through the instruction loop per comparison — and it
keeps its nested loop instead (below).

## What changed, path by path

The paths are those of `tests/wait/probe.rb` (the reference's were measured with the book's
`data/code/vm_task_wait_in_blocks.rb`; the book's `docs/notes/mruby-task.md`). *waits*: the
task went back to the scheduler (another task's marks fall between the start and the end of the
wait). *raises*: `RuntimeError` for `pop`, `sleep`, `Task.pass`. `tests/wait_anywhere.rs` holds
the SabiRuby column after the change, for six ways of waiting and a host's own queue.

| path | reference 4.1.0-rc2 | SabiRuby before (`3170b63`) | SabiRuby after |
|---|---|---|---|
| plain, `yield`, `Proc#call`, `each`, `times`, `map`, `loop`, `tap` | waits | waits | waits |
| `send`, `__send__`, `method_missing` | waits | waits | waits |
| `send` falling to a Ruby `method_missing` | raises; `sleep(n)` stops every task | waits | waits |
| `public_send` | waits | raises; `sleep(n)` returns at once | waits |
| `instance_exec`, `instance_eval`, `class_exec`, `class_eval`, `module_eval` | waits | raises; `sleep(n)` returns at once | waits |
| `Method#call` (with and without a block), `bind_call` | waits | raises; `sleep(n)` returns at once | waits |
| `define_method`, `rescue`, `ensure` | waits | waits | waits |
| `Foo.new` → `initialize`, and the block it yields to | waits (source: `new_iseq`) | raises; `sleep(n)` returns at once | waits |
| `eval("…")`, `instance_eval("…")` | waits (source: `eval_irep`) | raises; `sleep(n)` returns at once | waits |
| `Array#index { }`, `Array.new(n) { }` | raises; `sleep(n)` stops every task | raises; `sleep(n)` returns at once | waits |
| `sort { }` | raises; `sleep(n)` stops every task | raises; `sleep(n)` returns at once | raises (named) |
| `Class.new { }`, `Module.new { }` | raises; `sleep(n)` stops every task | raises; `sleep(n)` returns at once | waits |
| `rindex { }`, `delete(x) { }`, `Hash.new { }[k]`, `Hash#default(k)`, `Struct.new { }`, `Data.define { }`, `Regexp#match { }` | not measured | raises; `sleep(n)` returns at once | waits |
| `catch { }` | waits (source: `catch_iseq`) | raises; `sleep(n)` returns at once | waits |
| `ObjectSpace.each_object { }` | raises; `sleep(n)` stops every task | raises; `sleep(n)` returns at once | raises (named) |
| `sub`/`gsub`/`scan` with a block | not measured | raises; `sleep(n)` returns at once | raises (named) |
| a callback the native makes itself (`join` → `to_s`, `include?` → `==`) | not measured | raises; `sleep(n)` returns at once | raises (named) |

`select!`, `reject!`, `keep_if`, `delete_if`, `sort_by`, `count`, `to_h`, `Array#fetch`,
`Hash#fetch`/`delete`/`merge`/`update`, `each_char`, `each_byte` and `String#upto` have natives
in SabiRuby that mrblib's Ruby definitions replace, so they waited before and wait now.

A `break` out of one of these blocks now leaves the method that took the block, as `break`
does: `[1, 2, 3].index { break :b }` is `:b` (it was `0` — the nested loop used `:b` as the
block's answer), `Array.new(3) { |i| break :e if i == 1; i }` is `:e`, `Class.new { break :c }`
is `:c`, and `Foo.new { break 9 }` with an `initialize` that yields is `9`. Recursion through
`instance_exec` and its relatives is limited by `CALL_LEVEL_MAX` frames (512) instead of
`NATIVE_DEPTH_MAX` nested loops (96).

## `sleep(n)` inside a boundary raises

`sleep(n)`, `usleep` and `sleep_ms` in a task that is inside a native boundary raise
`RuntimeError`, as `sleep` with no argument and `pop` do. The reference sleeps on the wall clock
instead (`sleep_us_impl`, mruby-task/src/task.c:767-789, "fall back to blocking sleep without
context switch"), which stops every task for that long and, since it clears `switching_` after,
leaves the task unpreemptable for the rest of its run. SabiRuby used to return at once without
waiting. Both of those look like a wait to a program that runs one task — the reference's takes
the time, SabiRuby's went on to the next line — and only a second task shows that nothing else
ran, or that no time passed. An error at the line that cannot wait is the one outcome a test with
one task sees. Outside a task (the root context) `sleep(n)` still waits with the host's
`sleep_hook`, as mruby-sleep does.

## What is still a boundary

Each of these raises `can't wait inside <the native>'s call to <what it called> (<the wait>)`,
for example `can't wait inside Array#join's call to #to_s (Task::Queue#pop)`. The reference says
`blocking pop cannot be called from within a C function boundary` (and two more variants). The
name is worked out when the error is raised, from the frames: the innermost `Cci::Skip` frame is
what was called back (a method by its `mid`, otherwise "a block"), and the frame below it is
still at the call that reached the native — its `pc` points past that instruction and the
receiver is still in its register (`Vm::boundary_name`). Nothing is recorded on the way in.

- `sort { }` and `sort! { }` — **the author's decision, 2026-09-26.** In a native loop frame a
  light block (`sort { |x, y| x <=> y }` over 50 elements) was 8.1% slower than the nested loop,
  over the line the change was held to, and no other way of writing the loop came under it (the
  same loop in Ruby was 3 times slower); speed was chosen over waiting here. The reference keeps
  it too. `sort_by { }` is mrblib's Ruby and waits.
- `ObjectSpace.each_object { }` — the reference keeps it too; the heap is being walked.
- `sub`, `gsub` and `scan` with a block (mruby-regexp; `sub` and `gsub` with a block in a build
  without it too), which run their block from the middle of a match loop in Rust.
- A block a native calls through `funcall` or `call_block` from anywhere but its own SEND: a
  host function (`Vm::define_closure`, `define_fn`) calling Ruby back, `Vm::class_new_instance`,
  the instance `new` of a `Struct` class (its `initialize`), `Proc.new` (its `initialize`), a
  `Hash` whose `default` is redefined.
- The callbacks a native makes on its own (the plan's stage 3, not done): `to_s` and `inspect`
  (`join`, `p`), `==`, `eql?` and `hash` (`include?`, Hash keys), `<=>` (`sort` without a block,
  `max`), `to_ary`, `to_str`, `to_proc`, `coerce`, `respond_to_missing?`, `const_missing`,
  `inherited`, `method_added` and the other hooks, `allocate`, `sum`'s `+`, `dig`'s `[]`,
  `initialize_copy`. Waiting there would need the native's Rust frames to be resumable — a state
  machine per native, or `core::future` — which is the plan's stage 3.

The timer switch (a task preempted at the end of its timeslice) is deferred inside all of these
as it was: it happens at the first instruction boundary outside them. A task that never leaves
one is what `RunLimits`'s overrun limits are for.
