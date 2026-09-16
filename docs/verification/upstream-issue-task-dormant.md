# mruby/mruby: a finished task is kept for the life of the VM (drafts)

> **Record only — not submitted, and not to be submitted without the author saying so**
> (author's decision, 2026-09-17: 「PR は記録だけ」). Nothing here was sent to mruby/mruby: no
> issue was opened, no pull request was opened. The point of this file is that the material is
> ready if the decision ever changes. The patch itself is
> [`patches/mruby-task-dormant-weak.patch`](patches/mruby-task-dormant-weak.patch), produced and
> tested against mruby 4.1.0-rc (`3cf73ee`) on the branch `task-dormant-weak` of the reference
> clone; the work is recorded in [`../worklog/2026-09-17-upstream-record.md`](../worklog/2026-09-17-upstream-record.md).
>
> The patch's commit message carries this session's `Co-Authored-By:` / `Claude-Session:`
> trailers, because that is the rule for every commit made here. They would be dropped before
> any real submission.

This is [`upstream-pr-candidates.md`](upstream-pr-candidates.md) item 4, worked out: measured on
the reference itself, checked against what is already reported upstream, and patched.

---

## 1. The issue text (English, as it would be filed)

**Title**: mruby-task: a finished task is kept for the life of the VM (`Task.stat[:dormant]` only
ever grows)

### What happens

A task that finishes goes to the dormant queue and nothing takes it out again. `mrb_close_task()`
(`mrbgems/mruby-task/src/task.c:1693`) is the only path to `mrb_task_free()` (62), and it is
reachable only from Ruby's `Task#close`. Meanwhile the task is pinned twice over:
`mrb_task_mark_all()` (94) walks **all four** queues — `q_dormant_` included
(`include/task.h:170`) — marking each task's `self`, `result` and `name` and the whole of its
stack and callinfo; and `task_create_common()` (926) calls `mrb_gc_register(mrb, task_obj)` on
top of that, which only `mrb_task_free()` undoes (68).

So a program that starts tasks over and over keeps every task it has ever run, with its result,
its name and its stack, until the VM is closed. On a microcontroller running a fixed set of tasks
this is invisible. In a host that restarts tasks — a shell that spawns one task per command, an
event loop, a game reloading a script — it is an unbounded leak.

### Minimal reproduction

mruby 4.1.0-rc (`3cf73ee`), a host build whose only addition to the default gembox is
`conf.gem core: 'mruby-task'` (the gem is in no gembox), POSIX HAL, gcc 12, x86_64 Linux:

```ruby
200.times { 10.times { Task.new { nil } }; Task.run; GC.start }
p Task.stat[:dormant][:count]    # => 2000
```

Measured with `GC.stat[:live]`, `ObjectSpace.count_objects` and `/proc/self/status`, always after
a final `GC.start`, against the same program's state before the loop:

| what the loop does | dormant count | live objects | RSS |
|---|---|---|---|
| 2000 tasks, nothing else | 2000 | +6 444 | +2 120 kB |
| 2000 tasks, each `join`ed from another task | 2200 | +7 444 | +2 388 kB |
| 2000 tasks, each `value` read afterwards | 2000 | +6 444 | +2 120 kB |
| 2000 tasks, each `close`d afterwards | **0** | **+43** | **+0 kB** |
| 10 000 tasks, nothing else | 10 000 | +32 044 | +10 608 kB |
| 100 000 tasks, each `close`d afterwards | **0** | **+43** | **+0 kB** |

Roughly 1 kB of RSS and 3.2 live objects per finished task, and `GC.start` never lowers either.
`join` does not reclaim anything (and leaks the joining task too); reading `Task#value` does not
either. Only `Task#close` does.

100 000 tasks without `close` do not finish at all:

```ruby
loop { 100.times { Task.new { nil } }; Task.run; GC.start }
# RuntimeError: too many irep references, after 65 500 tasks
```

Every finished task still holds the proc it ran, so the block's irep reference count
(`src/state.c:108`) fills up. A long-lived program that spawns from the same block dies at
65 535 spawns even if the memory would have been affordable.

### Why the test suite does not see it

Nothing in `mrbgems/mruby-task/test/` spawns in a loop, and no assertion in `test/task.rb`,
`test/queue.rb` or `test/gc_task.rb` reads the dormant queue's contents or a finished task's
liveness. `DORMANT` appears twice in `test/task.rb` — as one of the symbols `Task#status` may
answer (57) and as a substring of `Task#inspect` (76) — and the `Task.stat` assertion (147) only
checks that the four keys exist with an Integer `count` and an Array `tasks`.

### The question

Is this by design — tasks are zombies until `close`, the way a POSIX process is until `wait`?
PR #6947 (*Feature: `Task#close`*), which added the only reclamation path, says the reason for
not freeing automatically is that *"there are use cases where the reference needs to be
retained"*. That is exactly what reachability answers: a finished task **the program still
names** is kept, and one **nothing names** is not. If the intent really is that the scheduler
owns every task it has ever run, then the leak is the documented behaviour and `README.md` should
say so — it currently documents neither `Task#close` nor `Task#value` among the instance methods,
and its "GC Integration" section says tasks and their stacks "are properly marked and freed".

### The patch

Attached as a pull request: hold the dormant queue **weakly**. Three pieces, and the last two are
what the first one needs to be safe:

1. `mrb_task_mark_all()` skips `MRB_TASK_QUEUE_DORMANT`, and the `mrb_gc_register()` pin taken by
   `task_create_common()` is dropped when the task becomes dormant. A finished task is then kept
   by whatever still names it: a Ruby variable, a C handle that registered it, a joiner.
2. On its own, that is a use-after-free. An mruby data type has no mark hook, so a task's
   `result` and `name` are reachable *only* through the queue walk; stop walking the dormant
   queue and a finished task the program still holds answers a reused chunk:

   ```ruby
   t = Task.new { "result-#{1 + 1}" }
   Task.run; GC.start
   t.value      # => "646"   (with 1 alone)
   ```

   So the transition into the dormant queue also moves the result and the name into the wrapper
   object as instance variables, where the ordinary graph walk marks them for exactly as long as
   the `Task` object lives. The struct fields keep their values for C to read.
3. Nothing marks a dormant task's stack any more either, so the transition detaches the envs that
   escaped it — the same hazard PR #6983 fixed for the `close` path — and `dfree` unlinks the
   task from its queue and detaches its envs before freeing it, which `mrb_close_task()` used to
   do itself. Not while the heap is being torn down, for the reason `gc.c`'s `MRB_TT_FIBER` arm
   has its `end` flag; `mrb_mruby_task_gem_final()` sets a flag the free path reads.

Nothing observable changes for a program that keeps its tasks. `Task.list`, `Task.get`,
`Task.stat`, `Task#status` and `Task#value` answer for every task the program can still reach;
`Task#close` keeps its meaning (immediate, and `Task#status` on a closed task still raises). What
changes is that a finished task **nothing** can reach is collected — and, for a C embedder, that
a task handle held across a collection must be `mrb_gc_register()`ed like any other object.

With the patch: `dormant.count = 0`, `+43` live objects and `+0 kB` of RSS for every row of the
table above, 100 000 spawns of the same block included; 200 000 spawns run to the end.

**The alternative, if the zombie shape is deliberate** (patch B): keep all four queues as roots
and reclaim from `Task#join` and `Task#value` — a task whose result has been taken is closed. It
is smaller, but it does not fix the leak for code that never joins (the reproduction above never
does), so it needs `README.md` to say that a finished task stays until `close` or `join`. Patch A
is what this PR proposes; B has not been written.

### Tests

`test/gc_task.rb` gains two assertions: *a finished task nothing refers to is collected* (which
fails on master — the dormant count grows by 100 per run) and *a finished task the program still
holds keeps its result* (which is what piece 2 above is for). Suite: 2700 total, KO 0, Crash 0 on
the host build (2698 before, plus these two); 929 total, KO 0, Crash 0 on a `MRB_GC_STRESS`
build.

### Where this came from

A Rust port of mruby-task ([SabiRuby](https://github.com/sabiruby/sabiruby), which follows this
gem closely enough to run its test files) hit the same thing from the host side: 1000 tasks
spawned and finished left 2000 live objects behind a collection (612 → 2612, where the same
program with the queue held weakly gives 612 → 612). It showed up as a game freezing: a garden
demo that restarts ten creature scripts at once, six tasks each, and gets slower every round.

---

## 2. The pull request description (draft)

**Title**: mruby-task: hold the dormant queue weakly, so a finished task can be collected

> Fixes the leak reported in #NNNN.
>
> A task that finishes goes to the dormant queue and nothing takes it out again: `mrb_close_task()`
> is the only path to `mrb_task_free()`, reachable only from `Task#close`. Until then the task is
> pinned twice over — `mrb_task_mark_all()` walks all four queues, and `task_create_common()`
> `mrb_gc_register()`s the object on top of that. A program that starts tasks over and over keeps
> every task it has ever run, with its result, its name and its stack, for the life of the VM:
> `200.times { 10.times { Task.new { nil } }; Task.run; GC.start }` leaves `Task.stat[:dormant][:count]`
> at 2000. 10 000 tasks cost 10.6 MB of RSS and 32 044 unreachable live objects; past 65 500 spawns
> of the same block the program dies with "too many irep references".
>
> This holds the dormant queue weakly instead. A finished task is kept by whatever still names it.
>
> * `mrb_task_mark_all()` skips `MRB_TASK_QUEUE_DORMANT`.
> * `task_retire()`, called at each transition into the dormant queue and *before* it, while the
>   task is still in a queue the walk reaches: moves the result and the name into the wrapper
>   object as ivars (an mruby data type has no mark hook, so these were reachable only through the
>   queue walk — without this, `Task#value` on a finished task the program still holds answers a
>   reused chunk after the next collection); detaches the envs that escaped the task's stack, the
>   hazard #6983 fixed for the close path, except for the task the VM is standing in, which
>   `execute_task()`'s tail retires again once the context is restored; and drops the
>   `mrb_gc_register()` pin.
> * `dfree` unlinks the task from its queue and detaches its envs before freeing it —
>   `mrb_close_task()` did both itself, a task the collector frees has had neither done for it.
>   Not while the heap is being torn down (`mrb->task.finalizing`), for the reason `gc.c`'s fiber
>   arm has its `end` flag.
>
> `Task#close` keeps its meaning. `Task.list`, `Task.get`, `Task.stat`, `Task#status` and
> `Task#value` answer for every task a program can still reach. A C embedder that holds a task
> handle across a collection registers it, as for any other object.
>
> `test/gc_task.rb` gains the two assertions that tell the difference. Suite: 2700 total, KO 0,
> Crash 0; 929 total, KO 0, Crash 0 under `MRB_GC_STRESS`.

---

## 3. Prior art (read 2026-09-17, `gh` on mruby/mruby and the picoruby org)

Nothing reports this leak. What exists, and how it bears on it:

* **PR #6947 — Feature: `Task#close`** (hasumikin, merged 2026-07-14). The one that matters: it
  *is* the reclamation path, and it was added because of this leak, seen from PicoRuby.
  Verbatim: *"In R2P2 (PicoRuby), a single shell command spawns a single task. Without this
  feature, Task objects were leaking with every command executed. `Task#terminate` transitions the
  task to the 'Dormant' state, but since there are use cases where the reference needs to be
  retained, the task cannot be automatically freed. Therefore, a new API for explicit closing is
  required."* So upstream knows the shape of the problem and chose an explicit API, on a premise
  ("the reference may need to be retained") that reachability decides better than a global pin
  does. The issue text above quotes this and asks the question against it.
* **PR #6983 — Detach envs from a task stack before freeing it** (hasumikin, merged 2026-07-31).
  The safety argument the patch here had to respect, and evidence that `mrb_task_free()` *does*
  assume it is called from `close`: *"Not called from `mrb_task_free`. A task stays GC-registered
  for its whole life, so the GC frees one only while tearing the heap down, where detaching is
  both pointless and unsafe."* Once a dormant task can be swept mid-run, that sentence stops
  holding — hence the `dfree` that detaches, and the teardown flag that keeps the old behaviour
  where it was right.
* **PR #6931 — exclude the tick IRQ while GC marks the task queues** (sylph01, merged 2026-07-08).
  Why the walk in `mrb_task_mark_all()` is wrapped in `mrb_task_excl_enter()`/`_exit()`. The patch
  here keeps the walk inside that exclusion and takes it for the `dfree` unlink too.
* **PR #6930 — survive OOM during task context initialization** (sylph01, merged 2026-07-08) and
  **issue #6922 — two scheduler bugs** (`sleep` suspends the wrong task; the `terminate` self-check
  never fires; both fixed). Neighbouring bugs in the same file, no overlap.
* **Issues #6863 / #6865 / #6870 / #6872 / #6886 / #6642** — segfaults and `MRB_TT_FREE`
  assertions in mruby-task, all closed, all about marking or context switching rather than the
  dormant queue's lifetime.
* **PR #7491 — let an embedder disable the scheduler for one `mrb_state`** (closed, not merged).
  Unrelated.
* **The picoruby org** has no `mruby-task` repository (`gh repo list picoruby`, 34 repositories):
  the gem lives in mruby/mruby, and `picoruby/picoruby` consumes it. Nothing to report there
  separately.

No issue or PR mentions the dormant queue's lifetime, a task leak, or `Task.stat[:dormant]`
growing. Searches: `gh issue list -R mruby/mruby --search "task" --state all` (50 rows read),
`"dormant"`, `"task leak"`; `gh pr list -R mruby/mruby --search "mruby-task"`, `"dormant"`,
`"task close"`, all states.

## 4. What the README does and does not say

Asked because the answer decides whether the leak is a bug or a documented contract:
**`mrbgems/mruby-task/README.md` never says that a finished task must be closed.** It does not
document `Task#close` at all — the "Task Instance Methods" list is `#status`, `#name`/`#name=`,
`#priority`/`#priority=`, `#suspend`, `#resume`, `#terminate`, `#join`, and `#value` is missing
too. `#terminate` is described as "moving it to DORMANT state" with nothing about what happens
next. `close` appears in the file only as `Task::Queue#close`, a different thing. The nearest the
document comes to the question is "GC Integration": *"Task contexts are registered with the
garbage collector. Tasks and their stacks/callinfo are properly marked and freed."* A reader is
told the opposite of the behaviour.
