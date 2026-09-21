# Looking inside the VM: snapshots, traces and line numbers

`src/inspect.rs` is what a debugger or a visualiser reads: a **snapshot** of the VM at an
instruction boundary, and a **trace** of the events the book explains (environments detaching,
a raise walking the catch tables, fibers switching, the GC collecting). The browser playground
draws its "VM の状態" panes from exactly these two ([`playground.md`](playground.md)); nothing
here is specific to the browser, and the CLI or a test can use the same API.

Two rules shape the whole module:

* **It never calls Ruby.** Rendering a value must not run `inspect`, because running Ruby would
  allocate, could collect, could raise, and would move the program being inspected. `Vm::render`
  formats values in Rust only (`src/inspect.rs`), so a snapshot is free of side effects on the
  program. The price is that `#<Foo>` for an object with a custom `inspect` shows the default
  form, not the program's own.
* **It does not touch the instruction loop.** `exec_frames` has no new branch: recording happens
  only where the events themselves happen (`frame_env`, `pop_frame`, `handle_raise`,
  `unwind_return`, `switch_context`, `gc_collect`, the GC sweep). With recording off, the cost is
  one `Option` test at those places, which the benchmarks do not see (below).

## Line numbers (the DBG section)

`src/rite.rs` reads the debug section that `mrbc -g` writes, in the same order as the reference
`read_debug_record` (`src/load.c`): the file name table, then per irep the record size, the file
count, and for each file `start_pos`, the file name index, the entry count and the line type.
All three line types are decoded — `mrb_debug_line_ary` (a line per pc), `flat_map`
(`{start_pos, line}` pairs) and `packed_map` (variable-length pc and line **differences**, what
4.1.0-rc actually emits). The differences are added with `wrapping_add`, as the reference does:
a line number may go backwards, and the encoder relies on the wrap.

* `Irep { lines: Vec<(u32, u32)>, filename: Option<Vec<u8>> }`, `Irep::line_of(pc)` — the line of
  the largest `start_pos` not greater than `pc`, i.e. `mrb_debug_get_line`.
* `VmIrep` keeps both, so `Vm::next_line()` (named `current_line` when this was written) and
  `FrameView::line` work at run time.
* `vm::dump` now prints the line column, so a listing matches `mrbc --verbose`:
  `    3 004 GETUPVAR	R2	2	0`. Without debug info the column is blank.
* A failure to decode is ignored (best effort), like the LVAR section: a binary without usable
  debug info still loads and runs.

`tests/lines.rs` compares `line_of` with the reference listing: `tools/custom.sh` records
`mrbc -g --verbose` output as `tests/custom/*.dump`, and the test parses its `%5d %03d ` columns
and requires every line number to agree, including a case where the line goes backwards.

## The trace

`Vm::set_trace(true)` turns on `pub trace: Option<Vec<TraceEvent>>`; `Vm::take_trace()` takes the
buffer (leaving it empty). Off is the default.

| event | where it is recorded | what it shows |
|---|---|---|
| `EnvCreate { env, ctx, frame, base, len, mid }` | `frame_env` | a block or a lambda made an environment on the frame |
| `EnvDetach { env, len, reason }` | `pop_frame` (`FrameReturn`), the GC sweep of a finished fiber (`ContextSwept`) | the registers were copied into the heap: the closure outlives the frame |
| `Raise { exc, class, frame, irep, pc, line }` | `handle_raise` entry | where the exception was raised |
| `CatchLook { frame, irep, pc, line, matched }` | each turn of `handle_raise` | the catch table of one frame and whether an entry covered `pc` (`None` means the frame is discarded) |
| `FrameUnwound { frame, mid, by }` | `handle_raise`, `unwind_return` | a frame thrown away by `Raise`, `Break` or `Return` |
| `FiberSwitch { from, to, kind }` | `switch_context` | `Resume`, `Yield`, `Transfer`, `Terminate`, `Reset` |
| `GcCollect { before_live, after_live, swept, allocated_since }` | end of `gc_collect` | one collection, with `before_live = after_live + swept` |

The exception events are the mruby mechanism the book describes: no `longjmp`, but a catch table
per irep, consulted from the innermost frame outwards ([`exceptions.md`](exceptions.md)).

## The snapshot

`Vm::snapshot(regs_frames)` builds a `Snapshot` of plain data (`ContextView`, `FrameView`,
`RegView`, `EnvView`, `ProcView`, `HeapView`, `ValueView`). `regs_frames` limits how many frames
carry their registers, counted from the innermost, so a deep recursion does not produce a huge
structure.

* The running context's stack and callinfo live in `Vm`, not in `contexts[cur]` (`switch_context`
  swaps them), so the snapshot reads them from the right side and reports every context the same
  way.
* Registers are named from the irep's `lv`: `R1 = x` when the program was compiled with `-g`.
  `R0` is `self`, printed as `main` for the top-level object.
* `ValueView { text, class, id }`. The `id` is the heap index, so the same object can be
  recognised in two panes (a register and an environment slot).
* `EnvView` lists both the environments still attached to a frame and those that moved to the
  heap, with their values.
* `HeapView` is the GC's own counters: `len`, `live`, `free`, `allocated_since_gc`,
  `alloc_threshold`, `gc_count`, `live_after_gc`, `stress` ([`gc.md`](gc.md)).

`Vm::op_histogram() -> Vec<(&'static str, u64)>` names the entries of `op_counts` (instructions
never executed are left out).

## Cost

`tools/bench.sh`, trace off, against the 2026-09-11 baseline in [`../verification/bench.md`](../verification/bench.md):
`bm_fib` 6093.6 → 6165.9 ms (+1.2%), `bm_so_lists` 3767.1 → 3781.1 ms (+0.4%),
`bm_so_mandelbrot` 1697.8 → 1611.6 ms (−5.1%). Within the noise of the machine and inside the
±3% the plan asked for; the loop itself is unchanged.

With tracing on, the cost is one push per event, and the events are rare compared with
instructions — except under GC stress, where every allocation collects and every collection
records.

## Tests

`tests/inspect.rs`: the environment lifecycle of a closure, `Raise` → `CatchLook(matched)` →
`FrameUnwound`, fiber switches from the enumerator fixture, collections under stress
(`before = after + swept`), trace off by default and `take_trace` emptying the buffer, a snapshot
of frames, named registers and the heap, values rendered without running Ruby (the instruction
counter must not move), and the opcode histogram. `tests/lines.rs` checks the line numbers.
