//! mruby-task (`mrbgems/mruby-task`): cooperative multitasking with a priority scheduler.
//! `Task`, `Task::Queue`, `Task::Error` and the task-aware `sleep` family.
//!
//! A task is a context of its own, as a Fiber is (`Vm::contexts`), and the scheduler hands it
//! the CPU by resuming that context the way `Fiber#resume` does. What the reference gets from a
//! timer interrupt — the tick that ends a timeslice and wakes a sleeper — a `no_std` VM has no
//! source for, so the tick is counted in instructions instead and the scheduler's idle jumps the
//! counter to the next wakeup. `docs/design/gems.md` lists what that costs.

use alloc::{format, string::String, vec::Vec};

use crate::argc;
use crate::error::VmResult;
use crate::inspect::SwitchKind;
use crate::object::{ObjKind, TaskData};
use crate::value::{ObjId, Slot, Value};
use crate::vm::{FiberState, Timeslice, Vm, ROOT};

// `MRB_TASK_STATUS_*`
const DORMANT: u8 = 0x00;
const READY: u8 = 0x02;
const RUNNING: u8 = 0x03;
const WAITING: u8 = 0x04;
const SUSPENDED: u8 = 0x08;

// `MRB_TASK_REASON_*`
const REASON_NONE: u8 = 0x00;
const REASON_SLEEP: u8 = 0x01;
const REASON_JOIN: u8 = 0x04;
const REASON_QUEUE: u8 = 0x08;

// queue indices (`MRB_TASK_QUEUE_*`)
const Q_DORMANT: usize = 0;
const Q_READY: usize = 1;
const Q_WAITING: usize = 2;
const Q_SUSPENDED: usize = 3;

/// Milliseconds a tick stands for (`MRB_TICK_UNIT`).
pub(crate) const TICK_UNIT_MS: u32 = 4;
/// Ticks a task runs before it is preempted (`MRB_TIMESLICE_TICK_COUNT`).
const TIMESLICE: u8 = 3;
/// `MRB_TASK_PRIORITY_DEFAULT`.
const PRIORITY_DEFAULT: u8 = 128;

// ------------------------------------------------------------------ the task object

fn task_id(vm: &mut Vm, v: Value) -> VmResult<ObjId> {
    match v.obj() {
        Some(o) if matches!(vm.heap.get(o).kind, ObjKind::Task(_)) => Ok(o),
        _ => Err(vm.raise_type("not a Task")),
    }
}

fn td(vm: &Vm, o: ObjId) -> &TaskData {
    match &vm.heap.get(o).kind { ObjKind::Task(t) => t, _ => unreachable!("not a Task") }
}
fn td_mut(vm: &mut Vm, o: ObjId) -> &mut TaskData {
    match &mut vm.heap.get_mut(o).kind { ObjKind::Task(t) => t, _ => unreachable!("not a Task") }
}

/// Which queue a status belongs in (`mrb_task_q_insert`).
fn q_of(status: u8) -> usize {
    match status {
        READY | RUNNING => Q_READY,
        WAITING => Q_WAITING,
        SUSPENDED => Q_SUSPENDED,
        _ => Q_DORMANT,
    }
}

/// Puts the task in the queue its status names, the ready one ordered by priority and FIFO
/// within one priority (`mrb_task_q_insert`).
fn q_insert(vm: &mut Vm, o: ObjId) {
    let (q, pri) = { let t = td(vm, o); (q_of(t.status), t.priority) };
    if q == Q_READY {
        let at = vm.task.queues[q].iter().position(|x| td(vm, *x).priority > pri).unwrap_or(vm.task.queues[q].len());
        vm.task.queues[q].insert(at, o);
    } else {
        vm.task.queues[q].push(o);
    }
}

/// Takes the task out of whichever queue holds it (`mrb_task_q_delete`).
fn q_delete(vm: &mut Vm, o: ObjId) {
    for q in 0..4 {
        if let Some(i) = vm.task.queues[q].iter().position(|x| *x == o) {
            vm.task.queues[q].remove(i);
            return;
        }
    }
}

fn set_status(vm: &mut Vm, o: ObjId, status: u8) {
    q_delete(vm, o);
    td_mut(vm, o).status = status;
    q_insert(vm, o);
}

/// Whether the task is one the scheduler still knows (`Task#close` drops it).
fn is_queued(vm: &Vm, o: ObjId) -> bool {
    vm.task.queues.iter().any(|q| q.contains(&o))
}

// ------------------------------------------------------------------ the tick

/// One tick (`mrb_tick`): the running task's timeslice shrinks, and a sleeper whose deadline
/// passed becomes ready. Called from the instruction loop where nothing else drives it.
pub(crate) fn tick(vm: &mut Vm) {
    // a look at the clock the native count asked for, between two ticks: no tick is counted, and
    // the instructions that were left until the real one are put back
    if let Some(left) = vm.task.forced.take() {
        if left > 1 {
            vm.task.tick_left = left - 1;
            check_limits(vm);
            return;
        }
    }
    vm.task.tick_left = vm.task.tick_every;
    // the instruction count ends a timeslice unless the host said time does (and gave a clock)
    let counted = match vm.task.timeslice {
        Timeslice::Instructions | Timeslice::Both { .. } => true,
        Timeslice::Time { .. } => vm.task.clock.is_none(),
    };
    if counted {
        if let Some(r) = vm.task.running {
            if td(vm, r).status == RUNNING && td(vm, r).timeslice > 0 {
                let left = td(vm, r).timeslice - 1;
                td_mut(vm, r).timeslice = left;
                if left == 0 { vm.task.switching = true; }
            }
        }
    }
    if vm.task.limits_active { check_limits(vm); }
    // a host with a clock of its own moves it instead (`Vm::task_advance_ticks`)
    if vm.task.clock_from_instructions { advance_ticks(vm, 1); }
}

// ------------------------------------------------------------------ time limits

/// Works out whether a tick has anything to look at, and whether natives count towards a look
/// at the clock. Called whenever the clock, the timeslice or the run's limits change.
pub(crate) fn update_limits(vm: &mut Vm) {
    let t = &mut vm.task;
    let timed_slice = !matches!(t.timeslice, Timeslice::Instructions);
    let on_clock = t.clock.is_some() && (timed_slice || t.run_soft.is_some() || t.run_hard.is_some());
    t.native_sampling = on_clock;
    t.limits_active = on_clock || t.run_hard_instructions.is_some();
    if !on_clock { t.forced = None; }
}

/// The native count (`Vm::call_native`, only while a limit is kept on the clock): every
/// `native_every` natives, the next instruction boundary looks at the clock, since a native can
/// take longer than the ten thousand instructions between two ticks.
pub(crate) fn count_native(vm: &mut Vm) {
    let t = &mut vm.task;
    t.native_left = t.native_left.saturating_sub(1);
    if t.native_left != 0 { return; }
    t.native_left = t.native_every.max(1);
    if t.running.is_some() && t.forced.is_none() && t.tick_left > 1 {
        t.forced = Some(t.tick_left);
        t.tick_left = 1;
    }
}

/// Reads the clock and compares it with the limits: the running slice ends at the next
/// boundary where its time is up or the run's is, and a hard limit passed is marked for
/// `overrun`.
fn check_limits(vm: &mut Vm) {
    let now = vm.task.clock.map(|c| c());
    if let Some(now) = now {
        if let Timeslice::Time { nanos } | Timeslice::Both { nanos } = vm.task.timeslice {
            if now.saturating_sub(vm.task.slice_start) >= nanos { vm.task.switching = true; }
        }
        if vm.task.run_soft.is_some_and(|soft| now >= soft) { vm.task.switching = true; }
        if vm.task.run_hard.is_some_and(|hard| now >= hard) { vm.task.overrun = true; }
    }
    if vm.task.run_hard_instructions.is_some_and(|hard| vm.instructions >= hard) { vm.task.overrun = true; }
}

/// A hard limit is past (`check_limits`). A task that can be switched out at this boundary just
/// is; one that cannot — it stands in a block a native is waiting for, or in a fiber — gets
/// `Task::Overrun`, which unwinds the native as any exception does. Either way the switch stays
/// due, so a task that rescues the exception is switched out at its next boundary instead of
/// walking back into the same call.
pub(crate) fn overrun(vm: &mut Vm) -> Option<crate::error::VmError> {
    vm.task.overrun = false;
    vm.task.switching = true;
    let c = vm.cur;
    let switchable = c != ROOT && vm.exc.is_none() && !vm.fiber_check_native(c) && vm.contexts[c].vmexec;
    if switchable { return None; }
    let class = vm.task.overrun_class.unwrap_or(vm.core.exception);
    Some(vm.raise(class, "the task ran past its time limit inside a call that cannot be switched out"))
}

/// Moves the clock on and wakes what was sleeping until then (the tail of `mrb_tick`).
pub(crate) fn advance_ticks(vm: &mut Vm, n: u32) {
    vm.task.tick = vm.task.tick.wrapping_add(n);
    wake_sleepers(vm);
}

/// A task that runs the top level of a compiled program, for a host rather than for Ruby
/// (`mrb_create_task`, which takes an `RProc`). Ready at once; nothing runs until the scheduler
/// is asked to.
pub(crate) fn task_spawn(vm: &mut Vm, irep: crate::object::IrepId, priority: u8, name: Option<&str>) -> VmResult<ObjId> {
    let proc_ = vm.heap.alloc(vm.core.proc_, ObjKind::Proc(crate::object::ProcData {
        irep, upper: None, env: None, target_class: Some(vm.core.object),
        strict: false, scope: true, orphan: false, mid: None,
    }));
    let cls = {
        let tn = vm.intern("Task");
        match vm.const_get(vm.core.object, tn) { Some(Value::Obj(c)) => c, _ => vm.core.object }
    };
    let name = match name { Some(n) => vm.str_new(n.as_bytes()), None => Value::Nil };
    let t = create_task(vm, cls, proc_, name, priority)?;
    t.obj().ok_or_else(|| vm.raise(vm.core.runtime_error, "could not create the task"))
}

/// Instructions a task has run since it was made, for a host that shows what a script spends.
pub(crate) fn task_instructions(vm: &Vm, task: ObjId) -> u64 {
    match &vm.heap.get(task).kind { ObjKind::Task(t) => t.instructions, _ => 0 }
}

/// Where a task stands in its own source: the innermost frame of its context that has debug
/// info, as a file name and a line. `None` where it has run out, or where the program was
/// compiled without it.
pub(crate) fn task_location(vm: &Vm, task: ObjId) -> Option<(alloc::string::String, u32)> {
    let ctx = match &vm.heap.get(task).kind { ObjKind::Task(t) => t.ctx, _ => return None };
    if ctx == usize::MAX { return None; }
    // the running task's frames are the VM's own; a parked one keeps them in its context
    let frames = if ctx == vm.cur { &vm.ci } else { &vm.contexts.get(ctx)?.ci };
    for ci in frames.iter().rev() {
        let Some(ir) = vm.ireps.get(ci.irep) else { continue };
        if ir.lines.is_empty() { continue; }
        let file = ir.filename.clone().unwrap_or_else(|| alloc::string::String::from("(unknown)"));
        // `pc` is past the instruction being executed
        let line = ir.line_of(ci.pc.saturating_sub(1)).unwrap_or(0);
        return Some((file, line));
    }
    None
}

/// Every frame of a task's context with debug info, innermost first, as file and line. A host
/// that shows a script's own source wants the innermost frame *in that file*, which is not
/// always the innermost frame — a script waiting inside a library method stands in the library.
pub(crate) fn task_frames(vm: &Vm, task: ObjId) -> alloc::vec::Vec<(alloc::string::String, u32)> {
    let mut out = alloc::vec::Vec::new();
    let ctx = match &vm.heap.get(task).kind { ObjKind::Task(t) => t.ctx, _ => return out };
    if ctx == usize::MAX { return out; }
    let frames = if ctx == vm.cur { &vm.ci } else { match vm.contexts.get(ctx) { Some(c) => &c.ci, None => return out } };
    for ci in frames.iter().rev() {
        let Some(ir) = vm.ireps.get(ci.irep) else { continue };
        if ir.lines.is_empty() { continue; }
        let file = ir.filename.clone().unwrap_or_else(|| alloc::string::String::from("(unknown)"));
        out.push((file, ir.line_of(ci.pc.saturating_sub(1)).unwrap_or(0)));
    }
    out
}

/// What a task answered, for a host (`mrb_task_value`).
pub(crate) fn task_result(vm: &Vm, task: ObjId) -> Value {
    match &vm.heap.get(task).kind { ObjKind::Task(t) => t.result.get(), _ => Value::Nil }
}

/// Whether a task has run to its end, for a host.
/// The index of a task's context in the VM's context table, `None` for what is not a task or
/// a task that has no context (never started, or finished and released).
pub(crate) fn task_context(vm: &Vm, task: ObjId) -> Option<usize> {
    match &vm.heap.get(task).kind { ObjKind::Task(t) if t.ctx != usize::MAX => Some(t.ctx), _ => None }
}

pub(crate) fn task_is_dormant(vm: &Vm, task: ObjId) -> bool {
    match &vm.heap.get(task).kind { ObjKind::Task(t) => t.status == DORMANT, _ => false }
}

/// Moves every waiting task whose deadline has passed to the ready queue, and records the next
/// deadline (the tail of `mrb_tick`).
fn wake_sleepers(vm: &mut Vm) {
    if vm.task.wakeup_tick == u32::MAX { return; }
    let now = vm.task.tick;
    if (vm.task.wakeup_tick.wrapping_sub(now) as i32) > 0 { return; }
    let mut next = u32::MAX;
    for o in vm.task.queues[Q_WAITING].clone() {
        let (reason, deadline) = { let t = td(vm, o); (t.reason, t.wakeup_tick) };
        if reason != REASON_SLEEP && reason != REASON_QUEUE { continue; }
        if deadline == u32::MAX { continue; }
        if (deadline.wrapping_sub(now) as i32) <= 0 {
            q_delete(vm, o);
            {
                let t = td_mut(vm, o);
                t.status = READY;
                t.reason = REASON_NONE;
                t.queue = None;
                t.wakeup_tick = u32::MAX;
            }
            q_insert(vm, o);
            vm.task.switching = true;
        } else if next == u32::MAX || (deadline.wrapping_sub(next) as i32) < 0 {
            next = deadline;
        }
    }
    vm.task.wakeup_tick = next;
}

/// A deadline never equals the "no timed wakeup" sentinel (`mrb_task_normalize_wakeup`).
fn normalize(deadline: u32) -> u32 {
    if deadline == u32::MAX { u32::MAX - 1 } else { deadline }
}

fn note_wakeup(vm: &mut Vm, deadline: u32) {
    if vm.task.wakeup_tick == u32::MAX || (deadline.wrapping_sub(vm.task.wakeup_tick) as i32) < 0 {
        vm.task.wakeup_tick = deadline;
    }
}

// ------------------------------------------------------------------ running a task

/// The task that owns the running context, where one does (`MRB2TASK`).
fn current_task(vm: &Vm) -> Option<ObjId> {
    if vm.cur == ROOT { return None; }
    vm.task.running.filter(|t| td(vm, *t).ctx == vm.cur)
}

/// Hands `t` the CPU until it yields, is preempted or finishes (`execute_task`). An exception
/// the task does not handle becomes its result, as it does there, so the scheduler carries on.
fn execute_task(vm: &mut Vm, t: ObjId) {
    let ctx = td(vm, t).ctx;
    if ctx == usize::MAX || vm.contexts[ctx].status == FiberState::Terminated {
        stop_task(vm, t);
        return;
    }
    let old = vm.cur;
    {
        let d = td_mut(vm, t);
        d.timeslice = TIMESLICE;
        d.status = RUNNING;
    }
    vm.task.running = Some(t);
    vm.task.switching = false;
    vm.task.tick_left = vm.task.tick_every;
    vm.task.forced = None;
    if vm.task.limits_active {
        if let Some(clock) = vm.task.clock { vm.task.slice_start = clock(); }
    }
    let first = vm.contexts[ctx].status == FiberState::Created;
    vm.contexts[old].status = FiberState::Resumed;
    vm.contexts[ctx].prev = Some(old);
    vm.contexts[ctx].vmexec = true;
    vm.switch_context(ctx, SwitchKind::Resume);
    if first { start_block(vm, ctx); }
    let before = vm.instructions;
    let r = vm.run_loop_ctx(ctx, 0);
    let spent = vm.instructions - before;
    td_mut(vm, t).instructions += spent;
    vm.loop_exit = None;
    vm.task.running = None;
    vm.task.switching = false;
    vm.contexts[old].status = FiberState::Running;
    if vm.cur != old { vm.switch_context(old, SwitchKind::Reset); }
    let done = vm.contexts[ctx].status == FiberState::Terminated;
    match r {
        Ok(v) => { if done { td_mut(vm, t).result = Slot::from(v); } }
        Err(crate::error::VmError::Raise(exc)) => {
            // an unhandled exception is the task's result, not the scheduler's (`execute_task`)
            td_mut(vm, t).result = Slot::from(exc);
            vm.exc = None;
            vm.contexts[ctx].status = FiberState::Terminated;
            vm.contexts[ctx].ci.clear();
            vm.contexts[ctx].stack.clear();
            stop_task(vm, t);
            return;
        }
        Err(_) => {
            vm.contexts[ctx].status = FiberState::Terminated;
            stop_task(vm, t);
            return;
        }
    }
    if done {
        stop_task(vm, t);
    } else if td(vm, t).status == RUNNING {
        // the task gave the CPU back but is still runnable: round-robin within its priority
        set_status(vm, t, READY);
    }
}

/// Lays the task's block on its fresh context, as `fiber_switch` does for a `Created` one.
fn start_block(vm: &mut Vm, ctx: usize) {
    let p = match vm.contexts[ctx].proc_ { Some(p) => p, None => return };
    let (irep, env, tc) = { let pd = vm.heap.proc_data(p); (pd.irep, pd.env, pd.target_class) };
    let self_ = match env.map(|e| vm.env_value(e, 0)) { Some(v) => v, None => Value::Obj(vm.top_self) };
    let nregs = vm.ireps[irep].nregs.max(4);
    vm.stack.clear();
    vm.stack.resize(nregs, Slot::NIL);
    vm.stack[0] = Slot::from(self_);
    let tc = tc.unwrap_or(vm.core.object);
    vm.ci.clear();
    vm.ci.push(crate::vm::CallInfo {
        base: 0, pc: 0, irep, proc_: p, n: 0, kw: false, mid: None, target_class: tc, env: None,
        cci: crate::vm::Cci::None, vis: crate::object::Vis::Public, modfunc: false, vis_break: false,
    });
}

/// The task is over: dormant, and whoever joined it becomes ready (`execute_task`'s tail).
fn stop_task(vm: &mut Vm, t: ObjId) {
    if is_queued(vm, t) { set_status(vm, t, DORMANT); } else { td_mut(vm, t).status = DORMANT; }
    let ctx = td(vm, t).ctx;
    if ctx != usize::MAX && ctx != vm.cur {
        vm.detach_context_envs(ctx);
        vm.contexts[ctx].status = FiberState::Terminated;
        vm.contexts[ctx].ci.clear();
        vm.contexts[ctx].stack.clear();
    }
    wake_join_waiters(vm, t);
}

fn wake_join_waiters(vm: &mut Vm, done: ObjId) {
    for o in vm.task.queues[Q_WAITING].clone() {
        let (reason, join) = { let t = td(vm, o); (t.reason, t.join) };
        if reason == REASON_JOIN && join == Some(done) {
            q_delete(vm, o);
            { let t = td_mut(vm, o); t.status = READY; t.reason = REASON_NONE; t.join = None; }
            q_insert(vm, o);
        }
    }
}

/// Hands the CPU back to the scheduler, which the reference does by raising a flag the run loop
/// reads at the next instruction boundary (`switching_ = TRUE`) rather than by switching inside
/// the native. Doing the same here is what lets a parking call still answer a value: the SEND
/// stores what the native returned, and only then does the context go (`Vm::exec_frames`).
fn park(vm: &mut Vm) {
    vm.task.switching = true;
}

// ------------------------------------------------------------------ the scheduler

/// One turn of the scheduler (`task_run_one_iteration`): `Some(task)` where one ran.
fn scheduler_step(vm: &mut Vm) -> Option<ObjId> {
    if let Some(f) = vm.task.hook { f(vm); }
    let t = vm.task.queues[Q_READY].first().copied()?;
    if td(vm, t).status == DORMANT { q_delete(vm, t); return Some(t); }
    // the running task is at the head of the ready queue while it runs, so a scheduler reached
    // from inside it would resume the context it is standing in
    if td(vm, t).ctx == vm.cur { return None; }
    execute_task(vm, t);
    Some(t)
}

/// Whether anything is left that could still run (`task_run_body`'s exit test), and whether the
/// scheduler can make progress without a task running (a timed wakeup).
fn idle(vm: &mut Vm) -> bool {
    if vm.task.wakeup_tick == u32::MAX { return false; }
    // nothing is ready, so the clock is free to jump to the next deadline: the reference idles
    // the CPU here and waits for the timer (`mrb_hal_task_idle_cpu`)
    let target = vm.task.wakeup_tick;
    if (target.wrapping_sub(vm.task.tick) as i32) > 0 { vm.task.tick = target; }
    wake_sleepers(vm);
    true
}

/// A `Task::Queue` a host can hand values to (`Vm::task_queue_new`). The Ruby side is the gem's
/// own class, so a task waits on it with `pop` and everything the gem's tests check still holds.
pub(crate) fn queue_new(vm: &mut Vm) -> VmResult<ObjId> {
    let tn = vm.intern("Task");
    let task = match vm.const_get(vm.core.object, tn) {
        Some(Value::Obj(t)) => t,
        _ => return Err(vm.raise(vm.core.runtime_error, "Task is not defined")),
    };
    let qn = vm.intern("Queue");
    let cls = match vm.const_get(task, qn) {
        Some(Value::Obj(c)) => c,
        _ => return Err(vm.raise(vm.core.runtime_error, "Task::Queue is not defined")),
    };
    let new = vm.intern("new");
    let q = vm.funcall(Value::Obj(cls), new, &[], Value::Nil)?;
    q.obj().ok_or_else(|| vm.raise(vm.core.runtime_error, "could not create the queue"))
}

/// Puts a value in the queue and makes the task waiting on it ready again (`Queue#push` from the
/// host's side): how a host answers a request a script is parked on.
pub(crate) fn queue_push(vm: &mut Vm, queue: ObjId, value: Value) -> VmResult<()> {
    let s = Value::Obj(queue);
    let items = q_items(vm, s)?;
    if q_closed(vm, s) { return Err(task_error(vm, "queue closed")); }
    let push = vm.intern("push");
    vm.funcall(items, push, &[value], Value::Nil)?;
    wake_queue_waiters(vm, queue, false);
    Ok(())
}

/// How many items are waiting in a queue made by `queue_new` (`Vm::task_queue_len`): what
/// `Queue#size` answers, without going through a send.
pub(crate) fn queue_len(vm: &mut Vm, queue: ObjId) -> VmResult<usize> {
    let items = q_items(vm, Value::Obj(queue))?;
    Ok(ary_len(vm, items))
}

/// One item out of a queue, or `None` where it is empty (`Vm::task_queue_try_pop`). The host's
/// side of `Queue#__pop_try(true)`: it never parks, because the host is not a task and has
/// nothing to park. A closed queue answers `None` like an empty one — for a reader there is no
/// difference, and `Queue#closed?` is the question that has one.
pub(crate) fn queue_try_pop(vm: &mut Vm, queue: ObjId) -> VmResult<Option<Value>> {
    let items = q_items(vm, Value::Obj(queue))?;
    if ary_len(vm, items) == 0 { return Ok(None); }
    let shift = vm.intern("shift");
    let v = vm.funcall(items, shift, &[], Value::Nil)?;
    Ok(Some(v))
}

/// Ticks until the earliest deadline, for a host that waits on a clock of its own
/// (`Vm::task_next_wakeup_ticks`). `Some(0)` where one has passed already.
pub(crate) fn next_wakeup_ticks(vm: &Vm) -> Option<u32> {
    if vm.task.wakeup_tick == u32::MAX { return None; }
    let left = vm.task.wakeup_tick.wrapping_sub(vm.task.tick) as i32;
    Some(if left > 0 { left as u32 } else { 0 })
}

/// Whether the scheduler still has something that can run: a ready task, or one waiting for a
/// deadline. A task that only something else could wake (suspended, joining) is not counted,
/// which is the same test `Task.run` ends on.
pub(crate) fn pending(vm: &Vm) -> bool {
    if !vm.task.queues[Q_READY].is_empty() { return true; }
    vm.task.queues[Q_WAITING].iter().any(|o| {
        let t = td(vm, *o);
        (t.reason == REASON_SLEEP || t.reason == REASON_QUEUE) && t.wakeup_tick != u32::MAX
    })
}

fn task_run(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 0);
    // The scheduler is already running this caller: the reference's own loop says so with
    // `loop_running`, and a host that drives it a step at a time (`Vm::task_run_once`, the shape
    // a browser or a frame loop uses) leaves that flag clear — so ask the caller instead. Without
    // this, `Task.run` from inside a task would take the head of the ready queue, which is the
    // running task itself, and re-enter its own context.
    if vm.task.loop_running || current_task(vm).is_some() { return Ok(Value::Nil); }
    vm.task.loop_running = true;
    loop {
        if let Some(f) = vm.task.hook { f(vm); }
        if vm.task.queues[Q_READY].is_empty() {
            if vm.task.queues[Q_WAITING].is_empty() && vm.task.queues[Q_SUSPENDED].is_empty() { break; }
            // The reference idles here until a tick makes a task ready, forever where none can.
            // Nothing else runs in this VM, so a wait nothing can end is the end of the loop
            // instead of a hang (`docs/design/gems.md`).
            // the CPU would idle here, which is what a scheduler-driven collector spends
            // (`mrb_gc_scheduler_pending` / `mrb_gc_step`)
            if vm.task.gc_driven { vm.gc_start(); }
            if !idle(vm) { break; }
            continue;
        }
        scheduler_step(vm);
    }
    vm.task.loop_running = false;
    Ok(Value::Nil)
}

/// What one turn of the scheduler did ([`task_step`]). The value a finished task answered is
/// not in here on purpose: it says nothing about whether the scheduler can go on, and reading
/// it as if it did is what made a task ending with nil end the host's whole run
/// (`docs/worklog/2026-09-17-task-end-nil.md`).
pub(crate) enum Step {
    /// this task got the CPU (it may have finished, parked or been preempted)
    Ran(ObjId),
    /// nothing was ready, but the clock moved on to the next deadline and woke a sleeper
    Idled,
    /// nothing ran and nothing could be made ready: a host loop stops here
    Stuck,
}

/// One turn of the scheduler for a host loop (`mrb_task_run_once`'s body). This is the shape a
/// host loop wants — a frame of a game, a turn of an event loop — where `Task.run` would block
/// until every task is done.
pub(crate) fn task_step(vm: &mut Vm) -> VmResult<Step> {
    if let Some(f) = vm.task.hook { f(vm); }
    if vm.task.queues[Q_READY].is_empty() {
        // where the host owns the clock, a turn that found nothing ready is simply over: moving
        // the clock is the host's to do (`Vm::task_advance_ticks`)
        if !vm.task.clock_from_instructions { return Ok(Step::Stuck); }
        if !vm.task.queues[Q_WAITING].is_empty() && idle(vm) { return Ok(Step::Idled); }
        return Ok(Step::Stuck);
    }
    // `None` where the head of the ready queue is the task the caller is standing in: nothing
    // ran and nothing will until it gives the CPU back, so this is stuck too
    Ok(match scheduler_step(vm) { Some(t) => Step::Ran(t), None => Step::Stuck })
}

/// One ready task, then back to the caller (`mrb_task_run_once`). It has no Ruby name in the
/// reference either; embedders reach it through [`Vm::task_run_once`].
pub(crate) fn task_run_once(vm: &mut Vm) -> VmResult<Value> {
    Ok(match task_step(vm)? {
        Step::Ran(t) if td(vm, t).status == DORMANT => td(vm, t).result.get(),
        Step::Ran(_) | Step::Idled => Value::True,
        Step::Stuck => Value::Nil,
    })
}

// ------------------------------------------------------------------ Task class methods

fn hash_entries(vm: &Vm, h: Value) -> Vec<(Value, Value)> {
    match h.obj().map(|o| &vm.heap.get(o).kind) {
        Some(ObjKind::Hash(hd)) => hd.entries().iter().map(|(k, v)| (k.get(), v.get())).collect(),
        _ => Vec::new(),
    }
}

fn kw_of(vm: &Vm, a: &[Value]) -> Option<Value> {
    match (vm.pending_kw, a.last()) { (Some(k), Some(l)) if !k.is_nil() && k == *l => Some(k), _ => None }
}

fn task_new(vm: &mut Vm, s: Value, a: &[Value], b: Value) -> VmResult<Value> {
    let kw = kw_of(vm, a);
    let positional = a.len() - usize::from(kw.is_some());
    if positional != 0 { return Err(vm.argnum_error(positional, "0")); }
    let (mut name, mut priority) = (Value::Nil, Value::Nil);
    if let Some(k) = kw {
        for (key, v) in hash_entries(vm, k) {
            match key { Value::Sym(sym) if vm.sym_name(sym) == "name" => name = v,
                        Value::Sym(sym) if vm.sym_name(sym) == "priority" => priority = v,
                        _ => { let d = vm.inspect_str(key)?; return Err(vm.raise_arg(&format!("unknown keyword: {d}"))); } }
        }
    }
    let p = match b.obj().filter(|o| matches!(vm.heap.get(*o).kind, ObjKind::Proc(_))) {
        Some(p) => p,
        None => return Err(vm.raise_arg("tried to create a Task without a block")),
    };
    if !name.is_nil() && vm.str_bytes(name).is_none() {
        let d = vm.describe_for_error(name);
        return Err(vm.raise_type(&format!("wrong argument type {d} (expected String)")));
    }
    let pri = match priority {
        Value::Nil => PRIORITY_DEFAULT,
        v => {
            let n = vm.expect_int(v, "priority")?;
            if !(0..=255).contains(&n) { return Err(vm.raise_arg("priority must be 0..255")); }
            n as u8
        }
    };
    let cls = match s.obj() { Some(c) => c, None => vm.core.object };
    create_task(vm, cls, p, name, pri)
}

/// Makes a task out of a block and puts it in the ready queue (`task_create_common`).
fn create_task(vm: &mut Vm, cls: ObjId, proc_: ObjId, name: Value, priority: u8) -> VmResult<Value> {
    let mut ctx = crate::vm::Context::new(FiberState::Created);
    ctx.proc_ = Some(proc_);
    vm.contexts.push(ctx);
    let ctx_id = vm.contexts.len() - 1;
    let o = vm.heap.alloc(cls, ObjKind::Task(alloc::boxed::Box::new(TaskData {
        ctx: ctx_id, priority, status: READY, reason: REASON_NONE, timeslice: TIMESLICE,
        name: Slot::from(name), result: Slot::from(Value::Nil), wakeup_tick: u32::MAX,
        join: None, queue: None, instructions: 0,
    })));
    q_insert(vm, o);
    Ok(Value::Obj(o))
}

/// `Task.current` (`mrb_task_s_current`): in the root context the task that stands for the
/// program itself, made on first use and never scheduled.
fn task_current(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 0);
    if let Some(t) = current_task(vm) { return Ok(Value::Obj(t)); }
    if let Some(t) = vm.task.main { return Ok(Value::Obj(t)); }
    let cls = match s.obj() { Some(c) => c, None => vm.core.object };
    let name = vm.str_new(b"main");
    let o = vm.heap.alloc(cls, ObjKind::Task(alloc::boxed::Box::new(TaskData {
        ctx: ROOT, priority: 0, status: RUNNING, reason: REASON_NONE, timeslice: TIMESLICE,
        name: Slot::from(name), result: Slot::from(Value::Nil), wakeup_tick: u32::MAX,
        join: None, queue: None, instructions: 0,
    })));
    vm.task.main = Some(o);
    Ok(Value::Obj(o))
}

fn task_list(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 0);
    let mut all = Vec::new();
    for q in 0..4 { for o in &vm.task.queues[q] { all.push(Value::Obj(*o)); } }
    Ok(vm.ary_new(all))
}

fn task_get(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    let want = vm.expect_str(a[0], "name")?;
    for q in 0..4 {
        for o in vm.task.queues[q].clone() {
            let n = td(vm, o).name.get();
            if vm.str_bytes(n).map(|b| b == &want[..]).unwrap_or(false) { return Ok(Value::Obj(o)); }
        }
    }
    Ok(Value::Nil)
}

/// `Task.pass` (`mrb_task_s_pass`): from a task, back to the scheduler; from the root context,
/// one turn of the scheduler (`task_run_one_iteration`).
fn task_pass(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 0);
    let Some(t) = current_task(vm) else {
        if vm.cur == ROOT && !vm.task.loop_running { scheduler_step(vm); }
        return Ok(Value::Nil);
    };
    if vm.fiber_check_native(vm.cur) {
        return Err(vm.raise(vm.core.runtime_error, "can't switch task across C function boundary"));
    }
    set_status(vm, t, READY);
    park(vm);
    Ok(Value::Nil)
}

fn task_stat(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 0);
    let h = vm.hash_new();
    let (tick, wakeup) = (vm.task.tick as i64, vm.task.wakeup_tick as i64);
    let k = vm.intern("tick"); vm.hash_set(h, Value::Sym(k), Value::Int(tick))?;
    let k = vm.intern("wakeup_tick"); vm.hash_set(h, Value::Sym(k), Value::Int(wakeup))?;
    for (q, name) in [(Q_DORMANT, "dormant"), (Q_READY, "ready"), (Q_WAITING, "waiting"), (Q_SUSPENDED, "suspended")] {
        let tasks: Vec<Value> = vm.task.queues[q].iter().map(|o| Value::Obj(*o)).collect();
        let sub = vm.hash_new();
        let ck = vm.intern("count");
        vm.hash_set(sub, Value::Sym(ck), Value::Int(tasks.len() as i64))?;
        let ary = vm.ary_new(tasks);
        let tk = vm.intern("tasks");
        vm.hash_set(sub, Value::Sym(tk), ary)?;
        let nk = vm.intern(name);
        vm.hash_set(h, Value::Sym(nk), sub)?;
    }
    Ok(h)
}

fn task_tick_m(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 0);
    Ok(Value::Int(vm.task.tick as i64 * TICK_UNIT_MS as i64))
}


/// Gives a task a fresh context running `proc_`, dropping the one it had (`mrb_task_init_context`,
/// which picoruby-sandbox's `Sandbox#execute` drives). The old context's escaped environments
/// take their values with them first.
pub(crate) fn reinit_context(vm: &mut Vm, t: ObjId, proc_: ObjId) {
    let old = td(vm, t).ctx;
    if old != usize::MAX && old != vm.cur {
        vm.detach_context_envs(old);
        vm.contexts[old] = crate::vm::Context::new(FiberState::Terminated);
    }
    let mut ctx = crate::vm::Context::new(FiberState::Created);
    ctx.proc_ = Some(proc_);
    vm.contexts.push(ctx);
    let id = vm.contexts.len() - 1;
    let d = td_mut(vm, t);
    d.ctx = id;
    d.result = Slot::from(Value::Nil);
    if d.status == DORMANT {
        q_delete(vm, t);
        td_mut(vm, t).status = READY;
        q_insert(vm, t);
    }
}

/// Runs one proc as a task of its own and waits for it (`mrb_execute_proc_synchronously`, which
/// picoruby-wasm drives): the scheduler is not entered, so nothing else runs meanwhile.
pub(crate) fn run_sync(vm: &mut Vm, proc_: ObjId) -> VmResult<Value> {
    let cls = {
        let tn = vm.intern("Task");
        match vm.const_get(vm.core.object, tn) { Some(Value::Obj(c)) => c, _ => vm.core.object }
    };
    let t = create_task(vm, cls, proc_, Value::Nil, 0)?;
    let id = t.obj().expect("task");
    // the task is not left in the queues: this is not scheduling, it is one call
    while td(vm, id).status != DORMANT {
        if !is_queued(vm, id) { break; }
        execute_task(vm, id);
        if td(vm, id).status == WAITING || td(vm, id).status == SUSPENDED {
            return Err(vm.raise(vm.core.runtime_error, "synchronous execution cannot block"));
        }
    }
    let result = td(vm, id).result.get();
    q_delete(vm, id);
    Ok(result)
}

// ------------------------------------------------------------------ Task::Queue

/// The items a queue holds, or `None` where the object never went through `initialize`.
fn q_items(vm: &mut Vm, s: Value) -> VmResult<Value> {
    let o = match s.obj() { Some(o) => o, None => return Err(vm.raise_arg("invalid queue")) };
    let k = vm.intern("@items");
    let items = vm.heap.ivar_get(o, k);
    if items.obj().map(|i| matches!(vm.heap.get(i).kind, ObjKind::Array(_))).unwrap_or(false) {
        Ok(items)
    } else {
        Err(vm.raise_arg("invalid queue"))
    }
}

fn ary_len(vm: &Vm, v: Value) -> usize {
    match v.obj().map(|o| &vm.heap.get(o).kind) { Some(ObjKind::Array(a)) => a.len(), _ => 0 }
}

fn q_closed(vm: &mut Vm, s: Value) -> bool {
    let k = vm.intern("@closed");
    s.obj().map(|o| vm.heap.ivar_get(o, k).truthy()).unwrap_or(false)
}

fn task_error(vm: &mut Vm, msg: &str) -> crate::error::VmError {
    let tn = vm.intern("Task");
    let en = vm.intern("Error");
    let cls = match vm.const_get(vm.core.object, tn) {
        Some(Value::Obj(t)) => match vm.const_get(t, en) { Some(Value::Obj(e)) => e, _ => vm.core.standard_error },
        _ => vm.core.standard_error,
    };
    vm.raise(cls, msg)
}

/// Wakes the tasks parked on `queue`: the first one for a push, all of them for a close.
fn wake_queue_waiters(vm: &mut Vm, queue: ObjId, all: bool) {
    for o in vm.task.queues[Q_WAITING].clone() {
        let (reason, target) = { let t = td(vm, o); (t.reason, t.queue) };
        if reason != REASON_QUEUE || target != Some(queue) { continue; }
        q_delete(vm, o);
        { let t = td_mut(vm, o); t.status = READY; t.reason = REASON_NONE; t.queue = None; t.wakeup_tick = u32::MAX; }
        q_insert(vm, o);
        vm.task.switching = true;
        if !all { break; }
    }
}

fn queue_init(vm: &mut Vm) -> ObjId {
    let tn = vm.intern("Task");
    let task = match vm.const_get(vm.core.object, tn) { Some(Value::Obj(t)) => t, _ => vm.core.object };
    let qn = vm.intern("Queue");
    let queue = vm.heap.alloc(vm.core.class, ObjKind::Class(crate::object::ClassData {
        name: Some(qn), superclass: Some(vm.core.object), outer: Some(task), ..Default::default()
    }));
    vm.heap.class_mut(task).consts.insert(qn, Slot::from(Value::Obj(queue)));
    vm.singleton_class(Value::Obj(queue)).expect("Task::Queue metaclass");
    // the two answers `__pop_try` gives that no item can be: objects of their own, as the
    // reference makes them
    for name in ["WAIT_RETRY", "WAIT_TIMEOUT"] {
        let o = vm.heap.alloc(vm.core.object, ObjKind::Object);
        let n = vm.intern(name);
        vm.heap.class_mut(queue).consts.insert(n, Slot::from(Value::Obj(o)));
    }
    vm.define_methods(queue, &[
        ("initialize", |vm, s, a, _b| {
            argc!(vm, a, 0);
            let o = match s.obj() { Some(o) => o, None => return Err(vm.raise_type("not a Queue")) };
            let items = vm.ary_new(Vec::new());
            let k = vm.intern("@items");
            vm.heap.ivar_set(o, k, items);
            let k = vm.intern("@closed");
            vm.heap.ivar_set(o, k, Value::False);
            Ok(s)
        }),
        ("__push", |vm, s, a, _b| {
            argc!(vm, a, 1);
            let items = q_items(vm, s)?;
            if q_closed(vm, s) { return Err(task_error(vm, "queue closed")); }
            let push = vm.intern("push");
            vm.funcall(items, push, &[a[0]], Value::Nil)?;
            if let Some(o) = s.obj() { wake_queue_waiters(vm, o, false); }
            Ok(s)
        }),
        ("__deadline", |vm, s, a, _b| {
            argc!(vm, a, 1);
            let _ = s;
            let ms = vm.expect_int(a[0], "timeout_ms")?;
            if ms < 0 { return Err(vm.raise_arg("timeout_ms must be non-negative")); }
            let ticks = (ms as u64).div_ceil(TICK_UNIT_MS as u64);
            if ticks > i32::MAX as u64 { return Err(vm.raise(vm.core.range_error, "timeout_ms is too large")); }
            Ok(Value::Int(normalize(vm.task.tick.wrapping_add(ticks as u32)) as i64))
        }),
        ("__pop_try", queue_pop_try),
        ("size", |vm, s, a, _b| { argc!(vm, a, 0); let items = q_items(vm, s)?; Ok(Value::Int(ary_len(vm, items) as i64)) }),
        ("length", |vm, s, a, _b| { argc!(vm, a, 0); let items = q_items(vm, s)?; Ok(Value::Int(ary_len(vm, items) as i64)) }),
        ("empty?", |vm, s, a, _b| { argc!(vm, a, 0); let items = q_items(vm, s)?; Ok(Value::bool(ary_len(vm, items) == 0)) }),
        ("clear", |vm, s, a, _b| {
            argc!(vm, a, 0);
            let items = q_items(vm, s)?;
            if let Some(o) = items.obj() { if let ObjKind::Array(v) = &mut vm.heap.get_mut(o).kind { v.clear(); } }
            Ok(s)
        }),
        ("close", |vm, s, a, _b| {
            argc!(vm, a, 0);
            q_items(vm, s)?;
            if !q_closed(vm, s) {
                let k = vm.intern("@closed");
                if let Some(o) = s.obj() { vm.heap.ivar_set(o, k, Value::True); wake_queue_waiters(vm, o, true); }
            }
            Ok(s)
        }),
        ("closed?", |vm, s, a, _b| { argc!(vm, a, 0); q_items(vm, s)?; Ok(Value::bool(q_closed(vm, s))) }),
        ("num_waiting", |vm, s, a, _b| {
            argc!(vm, a, 0);
            q_items(vm, s)?;
            let me = s.obj();
            let n = vm.task.queues[Q_WAITING].clone().into_iter()
                .filter(|o| { let t = td(vm, *o); t.reason == REASON_QUEUE && t.queue == me })
                .count();
            Ok(Value::Int(n as i64))
        }),
    ]);
    queue
}

/// `Task::Queue#__pop_try` (`queue_pop_try`): one item, or the sentinel that says the caller was
/// parked and should ask again once it runs.
fn queue_pop_try(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 0, 2);
    let items = q_items(vm, s)?;
    let non_block = a.first().map(|v| v.truthy()).unwrap_or(false);
    let deadline_v = a.get(1).copied().unwrap_or(Value::Nil);
    if ary_len(vm, items) > 0 {
        let shift = vm.intern("shift");
        return vm.funcall(items, shift, &[], Value::Nil);
    }
    if q_closed(vm, s) { return Ok(Value::Nil); }
    if non_block { return Err(task_error(vm, "queue empty")); }
    let sentinel = |vm: &mut Vm, name: &str| -> Value {
        let cls = vm.class_of(s);
        let n = vm.intern(name);
        vm.const_get(cls, n).unwrap_or(Value::Nil)
    };
    let deadline = match deadline_v {
        Value::Nil => None,
        v => {
            let d = vm.expect_int(v, "deadline")? as u32;
            if (d.wrapping_sub(vm.task.tick) as i32) <= 0 { return Ok(sentinel(vm, "WAIT_TIMEOUT")); }
            Some(d)
        }
    };
    let Some(me) = current_task(vm) else {
        return Err(vm.raise(vm.core.runtime_error, "blocking pop can only be called from within a task"));
    };
    if vm.fiber_check_native(vm.cur) {
        return Err(vm.raise(vm.core.runtime_error, "blocking pop cannot be called from within a C function boundary"));
    }
    {
        let t = td_mut(vm, me);
        t.reason = REASON_QUEUE;
        t.queue = s.obj();
        t.wakeup_tick = deadline.unwrap_or(u32::MAX);
    }
    set_status(vm, me, WAITING);
    if let Some(d) = deadline { note_wakeup(vm, d); }
    vm.task.switching = true;
    Ok(sentinel(vm, "WAIT_RETRY"))
}

// ------------------------------------------------------------------ Task instance methods

/// A task the scheduler no longer knows answers nothing (`Task#close` took it out).
fn live(vm: &mut Vm, s: Value) -> VmResult<ObjId> {
    let o = task_id(vm, s)?;
    if !is_queued(vm, o) && vm.task.main != Some(o) { return Err(vm.raise_arg("task is closed")); }
    Ok(o)
}

/// Whether `o` is the task running now, the main one included (`Task#close` refuses it).
fn is_current(vm: &Vm, o: ObjId) -> bool {
    current_task(vm) == Some(o) || (vm.cur == ROOT && vm.task.main == Some(o))
}

fn status_sym(vm: &mut Vm, status: u8) -> Value {
    let name = match status {
        DORMANT => "DORMANT", READY => "READY", RUNNING => "RUNNING",
        WAITING => "WAITING", SUSPENDED => "SUSPENDED", _ => "UNKNOWN",
    };
    Value::Sym(vm.intern(name))
}

pub fn init(vm: &mut Vm) {
    let task = vm.define_class("Task", vm.core.object);
    // `Task::Error` is what a closed or empty `Task::Queue` raises
    let en = vm.intern("Error");
    let err = vm.heap.alloc(vm.core.class, ObjKind::Class(crate::object::ClassData {
        name: Some(en), superclass: Some(vm.core.standard_error), outer: Some(task), ..Default::default()
    }));
    vm.heap.class_mut(task).consts.insert(en, Slot::from(Value::Obj(err)));
    vm.singleton_class(Value::Obj(err)).expect("Task::Error metaclass");
    // `Task::Overrun` is SabiRuby's: what a task that cannot be switched out gets past a hard
    // limit of `Vm::task_run_limits`. An Exception rather than a StandardError, as Interrupt is,
    // so that `rescue => e` in a loop does not swallow it
    let on = vm.intern("Overrun");
    let overrun = vm.heap.alloc(vm.core.class, ObjKind::Class(crate::object::ClassData {
        name: Some(on), superclass: Some(vm.core.exception), outer: Some(task), ..Default::default()
    }));
    vm.heap.class_mut(task).consts.insert(on, Slot::from(Value::Obj(overrun)));
    vm.singleton_class(Value::Obj(overrun)).expect("Task::Overrun metaclass");
    vm.task.overrun_class = Some(overrun);
    queue_init(vm);
    let tsc = vm.singleton_class(Value::Obj(task)).expect("Task singleton");
    vm.define_methods(tsc, &[
        ("new", task_new),
        ("current", task_current),
        ("list", task_list),
        ("get", task_get),
        ("pass", task_pass),
        ("stat", task_stat),
        ("run", task_run),
        ("tick", task_tick_m),
    ]);
    vm.define_methods(task, &[
        ("status", |vm, s, a, _b| { argc!(vm, a, 0); let o = live(vm, s)?; let st = td(vm, o).status; Ok(status_sym(vm, st)) }),
        ("name", |vm, s, a, _b| {
            argc!(vm, a, 0);
            let o = task_id(vm, s)?;
            let n = td(vm, o).name.get();
            if n.is_nil() { Ok(vm.str_new(b"(noname)")) } else { Ok(n) }
        }),
        ("name=", |vm, s, a, _b| { argc!(vm, a, 1); let o = task_id(vm, s)?; td_mut(vm, o).name = Slot::from(a[0]); Ok(a[0]) }),
        ("priority", |vm, s, a, _b| { argc!(vm, a, 0); let o = task_id(vm, s)?; Ok(Value::Int(td(vm, o).priority as i64)) }),
        ("priority=", |vm, s, a, _b| {
            argc!(vm, a, 1);
            let o = live(vm, s)?;
            let n = vm.expect_int(a[0], "priority")?;
            if !(0..=255).contains(&n) { return Err(vm.raise_arg("priority must be 0..255")); }
            q_delete(vm, o);
            td_mut(vm, o).priority = n as u8;
            q_insert(vm, o);
            Ok(a[0])
        }),
        ("inspect", |vm, s, a, _b| {
            argc!(vm, a, 0);
            let o = task_id(vm, s)?;
            let (status, name) = { let t = td(vm, o); (t.status, t.name.get()) };
            let name = if name.is_nil() { String::from("(noname)") } else { String::from_utf8_lossy(vm.str_bytes(name).unwrap_or(&[])).into_owned() };
            let st = match status_sym(vm, status) { Value::Sym(x) => String::from(vm.sym_name(x)), _ => String::new() };
            let text = format!("#<Task: {name} {st}>");
            Ok(vm.str_new(text.as_bytes()))
        }),
        ("suspend", |vm, s, a, _b| {
            argc!(vm, a, 0);
            let o = live(vm, s)?;
            if td(vm, o).status == DORMANT { return Ok(s); }
            let running = current_task(vm) == Some(o);
            set_status(vm, o, SUSPENDED);
            if running { park(vm); }
            Ok(s)
        }),
        ("resume", |vm, s, a, _b| {
            argc!(vm, a, 0);
            let o = live(vm, s)?;
            let status = td(vm, o).status;
            if status == SUSPENDED || (status == WAITING && td(vm, o).reason == REASON_SLEEP) {
                { let t = td_mut(vm, o); t.reason = REASON_NONE; t.wakeup_tick = u32::MAX; }
                set_status(vm, o, READY);
            }
            Ok(s)
        }),
        ("terminate", |vm, s, a, _b| {
            argc!(vm, a, 0);
            let o = live(vm, s)?;
            if td(vm, o).status == DORMANT { return Ok(s); }
            let running = current_task(vm) == Some(o);
            stop_task(vm, o);
            if running {
                // the task terminated itself: its context is gone, so the scheduler takes over
                let c = vm.cur;
                let prev = vm.contexts[c].prev.take().unwrap_or(ROOT);
                vm.contexts[c].status = FiberState::Terminated;
                let vmexec = core::mem::take(&mut vm.contexts[c].vmexec);
                vm.switch_context(prev, SwitchKind::Terminate);
                if vmexec { vm.loop_exit = Some(Value::Nil); }
            }
            Ok(s)
        }),
        ("close", |vm, s, a, _b| {
            argc!(vm, a, 0);
            let o = task_id(vm, s)?;
            if is_current(vm, o) { return Err(vm.raise(vm.core.runtime_error, "cannot close the current task")); }
            q_delete(vm, o);
            let ctx = td(vm, o).ctx;
            if ctx != usize::MAX && ctx != vm.cur && vm.contexts[ctx].status != FiberState::Terminated {
                vm.detach_context_envs(ctx);
                vm.contexts[ctx].status = FiberState::Terminated;
                vm.contexts[ctx].ci.clear();
                vm.contexts[ctx].stack.clear();
            }
            td_mut(vm, o).status = DORMANT;
            Ok(Value::Nil)
        }),
        ("join", |vm, s, a, _b| {
            argc!(vm, a, 0);
            let o = live(vm, s)?;
            // the root has no task to suspend, which the reference says in these words
            let Some(me) = current_task(vm) else {
                return Err(vm.raise(vm.core.runtime_error, "join can only be called from running task"));
            };
            if me == o { return Err(vm.raise_arg("can't join self")); }
            if td(vm, o).status == DORMANT { return Ok(td(vm, o).result.get()); }
            { let t = td_mut(vm, me); t.reason = REASON_JOIN; t.join = Some(o); }
            set_status(vm, me, WAITING);
            park(vm);
            // the result as it stands, which is nil where the wait is real: the value is stored
            // before the context goes, as it is in the reference (`docs/design/gems.md`)
            Ok(td(vm, o).result.get())
        }),
        ("value", |vm, s, a, _b| { argc!(vm, a, 0); let o = task_id(vm, s)?; Ok(td(vm, o).result.get()) }),
    ]);

    // Kernel#sleep and friends, which replace mruby-sleep's where both are there
    let k = vm.core.kernel;
    vm.define_methods(k, &[
        ("sleep", kernel_sleep),
        ("sleep_ms", |vm, _s, a, _b| {
            argc!(vm, a, 1);
            let ms = vm.expect_int(a[0], "ms")?;
            if ms < 0 { return Err(vm.raise_arg("time interval must be positive")); }
            sleep_us(vm, (ms as u64).saturating_mul(1000))?;
            Ok(Value::Nil)
        }),
        ("usleep", |vm, _s, a, _b| {
            argc!(vm, a, 1);
            let us = vm.expect_int(a[0], "usec")?;
            if us < 0 { return Err(vm.raise_arg("time interval must be positive")); }
            let before = wall_micros(vm);
            sleep_us(vm, us as u64)?;
            // mruby-sleep answers the microseconds actually waited, mruby-task the ones asked for
            Ok(Value::Int(match (before, wall_micros(vm)) {
                (Some(b), Some(e)) if e >= b => (e - b) as i64,
                _ => us,
            }))
        }),
    ]);
    let ksc = vm.singleton_class(Value::Obj(k)).expect("Kernel singleton");
    vm.define_methods(ksc, &[
        ("sleep", kernel_sleep),
    ]);
    for name in ["sleep", "usleep", "sleep_ms"] {
        let n = vm.intern(name);
        let _ = vm.set_visibility(k, n, crate::object::Vis::Private);
    }
}

/// `Kernel#sleep` (`mrb_f_sleep` of mruby-task, `f_sleep` of mruby-sleep). Both gems define this
/// name; the reference lets mruby-task's win where both are there (its README), and what
/// mruby-sleep adds is what happens *outside* a task — a real wait rather than a yield. The two
/// are one function here. No argument suspends the calling task until someone resumes it.
fn kernel_sleep(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 0, 1);
    if a.is_empty() {
        let Some(t) = current_task(vm) else { return Ok(Value::Nil) };
        if vm.fiber_check_native(vm.cur) {
            return Err(vm.raise(vm.core.runtime_error, "can't sleep across C function boundary"));
        }
        set_status(vm, t, SUSPENDED);
        park(vm);
        return Ok(Value::Nil);
    }
    let secs = match a[0] {
        Value::Int(i) => i as f64,
        Value::Float(f) => f,
        v => vm.expect_int(v, "sec")? as f64,
    };
    if secs < 0.0 { return Err(vm.raise_arg("time interval must be positive")); }
    let micros = (secs * 1_000_000.0).clamp(0.0, u32::MAX as f64) as u64;
    let before = wall_micros(vm);
    sleep_us(vm, micros)?;
    // the seconds actually waited, which is what mruby-sleep answers; a task that was parked
    // answers the seconds it asked for, since the wait is the scheduler's to measure
    Ok(Value::Int(match (before, wall_micros(vm)) {
        (Some(b), Some(e)) if e >= b => ((e - b) / 1_000_000) as i64,
        _ => (micros / 1_000_000) as i64,
    }))
}

/// Microseconds since the epoch, where the host lends a clock.
fn wall_micros(vm: &Vm) -> Option<u64> {
    let (s, ns) = vm.wall_clock?();
    Some((s.max(0) as u64) * 1_000_000 + (ns.max(0) as u64) / 1000)
}

/// The wait itself (`sleep_us_impl`). A task is parked until the tick reaches its deadline and
/// the scheduler takes over; anywhere else — the root context, or a task with a native frame on
/// the host stack — the host is asked to wait for real, and the clock moves on.
fn sleep_us(vm: &mut Vm, micros: u64) -> VmResult<Value> {
    let in_task = current_task(vm).filter(|_| !vm.fiber_check_native(vm.cur));
    let ticks = (micros.div_ceil(1000) as u32).div_ceil(TICK_UNIT_MS);
    let Some(t) = in_task else {
        if let Some(f) = vm.sleep_hook { f(micros); }
        vm.task.tick = vm.task.tick.wrapping_add(ticks);
        wake_sleepers(vm);
        return Ok(Value::Nil);
    };
    let deadline = normalize(vm.task.tick.wrapping_add(ticks));
    {
        let d = td_mut(vm, t);
        d.reason = REASON_SLEEP;
        d.wakeup_tick = deadline;
    }
    set_status(vm, t, WAITING);
    note_wakeup(vm, deadline);
    park(vm);
    Ok(Value::Nil)
}
