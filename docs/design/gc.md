# Garbage collection

SabiRuby reclaims unreachable objects with a **stop-the-world, non-moving mark & sweep**
collector. What it reproduces from mruby is the observable part: unreachable objects
(cycles included) are collected, `GC.*` means what it means in mruby, and native code has a
clear contract. mruby's tricolor incremental marking, generational mode, debt-driven pacing
and heap pages are implementation choices of `src/gc.c` and are not copied (book, GC chapter,
"仕様と実装都合"; porting chapter, stage 8). The plan this follows is
[`../plans/gc-plan.md`](../plans/gc-plan.md); where the implementation departs from it, the reason is under
[Departures from the plan](#departures-from-the-plan).

## Method

* The heap is `Vec<HeapObject>` indexed by `ObjId`. Objects never move (an `ObjId` is an index).
* `Heap` keeps one flag byte per slot (`MARKED`, `FREE`) and a free list. `alloc` reuses the
  lowest free slot first, else pushes a new one. Allocation never collects; it only sets
  `gc_pending` when a threshold is crossed.
* Mark: an explicit grey stack (`Vec<ObjId>`), no recursion, so 100 000 nested arrays are
  fine on any host stack. Marking a slot that is `FREE` is a hard `assert!` (a live object
  refers to a freed one: a missing root).
* Sweep: every unmarked slot gets its payload dropped and becomes
  `HeapObject { class: ObjId(0), kind: Object, .. }` (inert if touched by mistake;
  `Heap::get` has a `debug_assert!(!is_free)`); trailing free slots are cut off the table.
* Not incremental, so there is **no write barrier**. Not generational.

## When it runs

* **Only at the head of the instruction loop** (`exec_frames`, right after the step-budget
  check): `if self.heap.gc_pending { self.gc_maybe(); }`. At that point every register and
  frame is in the `Vm`.
* **Never while a native is on the host stack.** `Vm::native_active` counts the natives
  running (`call_native`, used by SEND → native and `funcall` → native, and by `Method#call`
  of a native). `gc_maybe` collects only when it is 0; otherwise the collection stays due and
  runs at the first boundary after the natives returned.
* `key_hash` / `key_eql` count as natives too: the `HASH`/`HASHADD`/`HASHCAT` instructions
  and keyword-argument packing hold the Hash being built in a Rust local while a Ruby-level
  `hash`/`eql?` runs (and that Ruby code has instruction boundaries of its own).
* `GC.start` collects **at once** when it is the only native running (a SEND from bytecode:
  everything else is in registers). Called through `funcall` from another native it only makes
  the collection due. While `GC.disable` is in effect it does nothing (as in the reference).
* Automatic trigger: after a collection that left `live` objects, the next one is due after
  `max(live * (interval_ratio - 100) / 100, 4096)` allocations (`interval_ratio` 200 = the heap
  may double), or when the estimated bytes allocated since the last collection exceed
  `GC.malloc_threshold` (non-zero). The estimate is 64 bytes per object plus the string length,
  16 × array length and 40 × hash entries at allocation time.

## Roots

| # | root | notes |
|---|---|---|
| 1 | `Vm::stack` | registers of the running context up to the end of the top frame's window (`base + nregs`, or `base + n + 4` while ENTER has not packed the arguments yet), as mruby's `mark_context_stack` marks `ci->stack + nregs`. Above that are registers of returned frames, which hold nothing the program can reach; marking them kept garbage alive (`ObjectSpace.count_objects` after `GC.start` showed it, 2026-09-12). They are set to nil by the collection, as `mark_context_stack` does: when the frame returns, its caller's window covers them again, and a stale reference there to an object this collection freed would fail the mark-time assertion at the next one (the stress suite showed exactly that before the clearing) |
| 2 | `Vm::ci` | each frame's `proc_`, `target_class`, `env` |
| 3 | contexts | scanned: the running one, `ROOT`, and every context that is `Running`/`Resumed` or has `vmexec` (the chain waiting for a resumed fiber). A context's `stack`, `ci`, `fib`, `proc_` |
| 4 | `Vm::globals` | |
| 5 | `Vm::exc` | exception or `RBreak` being propagated |
| 6 | `Vm::core` (all classes), `top_self`, `call_proc` | |
| 7 | `inspect_guard`, `eq_guard`, `pending_kw`, `loop_exit` | `loop_exit` = value a fiber yielded to a native resumer |
| 8 | `Vm::gc_registered` | `Vm::gc_register` / `gc_unregister` (mruby `mrb_gc_register`) |
| 9 | mruby-task's four queues and the running task | a task the program dropped every reference to is still going to run, so the scheduler's queues own their tasks (`mrb_task_mark_all`) |
| – | `ireps` | not scanned: pool literals are Rust values (`Pool`), not objects |
| – | symbols | never collected |

Edges from an object: `class`, `ivars`, and per kind:

| `ObjKind` | edges |
|---|---|
| `Object`, `String`, `Exception` | none |
| `Array` | elements |
| `Hash` | keys, values, `default` |
| `Range` | `begin`, `end` |
| `Proc` | `upper`, `env`, `target_class` |
| `Env` | `values` (detached), `target_class`, the special variables the scope owns (`svar`, `$~` and `$_`, and `svar_fwd`, the env a returned nested-load frame sent them to — `docs/design/gems.md`, mruby-regexp); for an **attached** env, its window `stack[base..base+len]` of its context (mruby marks `e->stack[0..len]`); not the whole context or the Fiber |
| `Regexp` | none (the compiled pattern holds no Ruby value) |
| `MatchData` | `source` (the subject as it was), `regexp` |
| `Task` | its name, its result, the task it joins, the queue it waits on, and its context (mruby-task) |
| `Class` | `superclass`, `methods` (`Method::Ruby` only), `consts`, `cvars`, `attached`, `iclass_of`, `origin`, `origin_of`, `outer` |
| `Break` | `value` |
| `Fiber` | its context |

A context that nothing reached (its Fiber object is garbage) can never run again. Before it
is emptied, every environment of its frames that *was* reached (a block captured inside the
fiber and kept elsewhere) is detached: its window is copied into `values` (mruby
`mrb_env_detach_all` in the sweep of the Fiber). Then the context is replaced by an empty
`Terminated` one; its index in `Vm::contexts` is not reused.

## Contract for native code

* **A native may hold `Value`s and `ObjId`s in Rust locals for as long as it runs.** Nothing is
  collected while any native is on the host stack, including while it re-enters the VM
  (`funcall`, `call_block`: the `to_s` `Array#join` calls, a block a native calls through
  `funcall`). There is no arena and nothing to save/restore. The natives that hand their block
  to a frame of its own when a SEND called them (`instance_exec`, `Class#new` → `initialize`,
  `sort { }`, `Array.new(n) { }`, …; [`wait-anywhere.md`](wait-anywhere.md)) are not on the
  host stack while that block runs, so collection is not postponed there.
* The cost of that rule: collection is postponed during Ruby code that runs *under* a native.
  mrblib's `each`/`map`/`times`/`loop`/`upto` are Ruby, so ordinary iteration is not affected;
  a long-running `initialize` or a `sort_by` block that allocates heavily is.
* **A host that keeps an object across calls into the VM** (between `Vm::step` calls, in a
  Bevy resource, ...) must `gc_register` it and `gc_unregister` it when done. A value returned
  by `load_and_run`/`step`/`funcall` is safe until the next call into the VM.
* VM-internal helpers that call Ruby from an instruction (`GETIDX` → a Ruby `[]`, `to_proc`,
  `to_a`, `const_missing`, ...) must keep their values in registers, or count as a native
  (`native_active`) while the Ruby code runs, as `key_hash` does.
* **The free hook** (`Vm::set_on_free`, for `ObjKind::Data` objects: a value the host owns,
  named by a `(tag, handle)` pair) runs at the very end of `gc_collect`, once the sweep has
  rebuilt the free list and the counters are updated — not from inside the sweep. The sweep
  only records what it freed, in `Heap::freed_data`. The hook is given the two numbers and
  **not** a `&mut Vm`: it is called from the collector, so there is nothing in the VM it could
  safely read, and the type is what enforces that rather than a rule in prose. A hook that
  needs to run Ruby queues the work for the host's next call into the VM. Only what the
  collector reclaims reaches the hook; what is still live when the `Vm` is dropped does not.

## Decisions from review (2026-09-11)

* **The two mark-time `assert!`s stay in release builds** (`Heap::mark_id`, `Heap::mark_drain`:
  a live object refers to a freed slot). They run only during a collection, never on ordinary
  execution, and they are what stopped the missing roots in release stress runs. Stopping at the
  collection is easier to debug than a corrupted object failing somewhere else later. Access
  checks (`Heap::get`/`get_mut`) stay `debug_assert!`.
* **The loop-head test (~2.7% on `bm_so_mandelbrot`) is accepted.** Merging it with the
  step-budget test is only a candidate (`performance.md`); if tried, it is separate, measured work.
* **No collection in Ruby code running under a native is accepted**, as the plan decided.
  Condition to revisit: a real case where Ruby code called back by a native runs long and its
  allocations actually cause a memory problem. Then switch to an arena-style scheme for natives.
  Today's candidates (the implicit callbacks `wait-anywhere.md` lists, and blocks a native calls
  through `funcall`) are short; `sort { }` blocks and `initialize` from `Class#new` stopped being
  candidates when a SEND's call to them became a frame of its own (2026-09-26).

## `GC` module

| method | behaviour |
|---|---|
| `GC.start` | full collection (see above); nil |
| `GC.disable` / `GC.enable` | postpone / resume collection; return the previous disabled state. A collection that became due while disabled runs at the next boundary after `enable` |
| `GC.interval_ratio`, `=` | the growth factor above (default 200) |
| `GC.malloc_threshold`, `=` | byte axis of the trigger (default 16 MiB; 0 = off) |
| `GC.stat` | `live` (objects in use), `malloc_increase` (estimate since the last collection), `malloc_threshold`, `step_limit`, `symbol_count` are real values; `debt`, `state`, `generational`, `full`, `dynamic_symbol_count` are constants |
| `GC.step_ratio`, `step_limit`, `generational_mode` (and `=`) | values kept for the interface only; there are no steps and no generations |

## Intended differences

* `object_id` and the `0x...` in `inspect` come from the slot index, so the id of a collected
  object is given to a later one. mruby reuses addresses the same way.
* `GC.stat` fields of the incremental/generational machinery are constants (above).
* No generational mode, no incremental steps: one collection stops the program for its whole
  duration (tens of microseconds for a few thousand live objects; see `performance.md`).

## Verification

* **Stress mode**: `SABIRUBY_GC_STRESS=1` (read by the CLI, `sabiruby mrbtest` and
  `cargo test`; the library itself has `Vm::set_gc_stress`) makes every allocation make a
  collection due, i.e. a full collection at every instruction boundary after an allocation,
  from the loading of mrblib on (mruby `MRB_GC_STRESS`). The test suite, the fixtures and
  `tests/fixtures/gc.rb` give the same results as without it. A debug build adds the
  `is_free` assertion on every heap access.
* `tests/fixtures/gc.rb` (output recorded from the reference) covers what the suite does not:
  Ruby `hash`/`eql?` in a hash literal, a block captured in a suspended fiber, a collection in
  a chain of resumed fibers, dropped fibers, an external enumerator, `return` through `ensure`,
  deep nesting, anonymous and singleton classes. Removing any of these roots makes it fail
  under stress (checked by hand): `exc`, frame `env`, the attached-env window, the resume
  chain, the `key_hash` guard; so does skipping the detach of a reached env when its fiber
  is collected (the captured block then reads nil). The first two also break the test suite;
  the others only this fixture.
* `bench/src/gc_churn.rb`: 1 000 000 iterations allocating three objects each.

## Departures from the plan

* **Contexts are not all roots.** The plan listed every `Context` as root 3 and also said a
  context is emptied when its Fiber object is swept; with every context a root (and `fib` in
  it) no Fiber could ever be swept. Only the running context, `ROOT` and the resume chain are
  roots; a suspended fiber is reached through its Fiber object or through an attached
  environment, as in mruby.
* **`key_hash`/`key_eql` also hold the collector off.** The plan (§3) reasoned that `HASH`
  cannot collect because it is not an instruction boundary, but a Ruby `hash` method called
  from it runs a nested loop, whose boundaries are. The fixture shows the failure.
* **Envs and fibers follow mruby 4.1, not the plan's table.** An attached env marks its stack
  window, and a collected fiber detaches the reached envs of its frames first. (A first
  version marked the whole context from an attached env; Ruby saw the same values, but a
  captured block kept its whole fiber alive.)
* **`loop_exit` is a root** (not in the plan's list): it carries the value of a `Fiber.yield`
  back to a native resumer.
* **The plan's `debug_assert` "an attached env's context has not been emptied" is not added**:
  after an error, `reset_to_root` clears frames without detaching their environments, so an
  attached env can point at an emptied stack. Reading it gives nil, as before the collector.
* `SABIRUBY_GC_STRESS` is read by the test harnesses (`tests/*.rs`) as well as by the CLI, so
  `SABIRUBY_GC_STRESS=1 cargo test` works; the library only has the flag (no_std).

## Future work

* Incremental marking needs a write barrier; `Slot::set` is the single storing window where it
  would go (`performance.md`).
* Reuse context indices of collected fibers (today `contexts` only grows; the entries are empty).
* Allow collection inside Ruby code running under a native, when the condition in
  [Decisions from review](#decisions-from-review-2026-09-11) is met (would need an arena or a
  root-registration discipline for natives).
* Measure the collector on a larger live heap (Bevy scenes) and decide whether pauses need
  to be bounded (the book's scheduler-driven GC: "advance one unit, report the work done").
