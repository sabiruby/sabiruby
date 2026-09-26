//! mruby-task's scheduler with many tasks waiting (`docs/plans/host-scale-plan.md`, H1).
//!
//! Two kinds of test. The first fixes the order things happen in — which task a wake-up makes
//! ready first, where a task goes back in the ready queue, what `Task.list` and `Task.stat` list
//! — as the scheduler answered before its queues changed shape (`docs/worklog/2026-09-26-release-0.7-b.md`),
//! so that a change of data structure cannot change what a program sees. The second counts the
//! queue elements the scheduler looks at or moves one by one (`TaskState::walked`) for one turn,
//! with a hundred and with a thousand other tasks waiting: the count is what the turn costs, and
//! it must not grow with the number of tasks that are only waiting. It is a count, not a time.

use sabiruby::{Value, Vm};

fn compile(src: &str) -> Vec<u8> {
    sabiruby_compiler::compile(src.as_bytes(), &sabiruby_compiler::Options {
        filename: "(test)".into(), debug_info: true, ..Default::default()
    }).expect("compile")
}

fn run(vm: &mut Vm, src: &str) -> Value {
    let bin = compile(src);
    vm.load_and_run(&bin).expect("run")
}

fn inspect(vm: &mut Vm, src: &str) -> String {
    let v = run(vm, src);
    vm.inspect_str(v).expect("inspect")
}

/// Runs the scheduler until nothing is ready: with the host's clock, that is where a turn of a
/// frame loop ends (`Vm::task_run_limits` stops at the first turn that finds nothing to run).
fn drain(vm: &mut Vm) {
    // a bound on the work rather than on the turns; none of these scripts comes near it
    vm.task_run_budget(10_000_000).expect("run");
}

/// A VM with mrblib; `SABIRUBY_GC_STRESS=1` collects at every boundary after an allocation, which
/// is what finds a task an index holds that the collector does not.
fn new_vm() -> Vm {
    let mut vm = Vm::with_mrblib().expect("vm");
    vm.set_gc_stress(std::env::var("SABIRUBY_GC_STRESS").map(|v| !v.is_empty() && v != "0").unwrap_or(false));
    vm.task_set_index_from(INDEX_FROM.with(|m| m.get()));
    vm
}

std::thread_local! {
    /// What the VMs this thread makes index their waiting tasks from (`Vm::task_set_index_from`).
    static INDEX_FROM: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Runs an order test twice: with the waiting tasks indexed from the first, and never indexed
/// (a wake-up walks the waiting queue). The order must be the same either way.
fn both(body: fn()) {
    for n in [0, usize::MAX] {
        INDEX_FROM.with(|m| m.set(n));
        body();
    }
    INDEX_FROM.with(|m| m.set(0));
}

fn host_vm(src: &str) -> Vm {
    let mut vm = new_vm();
    vm.task_external_clock(true);
    run(&mut vm, src);
    vm
}

// ------------------------------------------------------------------ the order things happen in

fn sleepers_woken_together_run_in_the_order_they_went_to_sleep_body() {
    // `a` sleeps longest and went to sleep first. A clock that jumps past every deadline at once
    // wakes them in the order they entered the waiting queue, not in the order of their deadlines
    // (`wake_sleepers` walked the waiting queue from its head).
    let mut vm = host_vm(r#"
      $order = []
      Task.new(name: "a") { sleep_ms 30; $order << :a }
      Task.new(name: "b") { sleep_ms 10; $order << :b }
      Task.new(name: "c") { sleep_ms 20; $order << :c }
      Task.new(name: "d") { sleep_ms 10; $order << :d }
    "#);
    drain(&mut vm);
    assert_eq!(inspect(&mut vm, "$order"), "[]");
    // 3 ticks of 4 ms: b and d are due, c and a are not
    vm.task_advance_ticks(3);
    drain(&mut vm);
    assert_eq!(inspect(&mut vm, "$order"), "[:b, :d]");
    // the next deadline is c's (5 ticks), and from here on a's and c's pass together
    assert_eq!(vm.task_next_wakeup_ticks(), Some(2));
    vm.task_advance_ticks(100);
    drain(&mut vm);
    assert_eq!(inspect(&mut vm, "$order"), "[:b, :d, :a, :c]");
    assert_eq!(vm.task_next_wakeup_ticks(), None);
}

fn a_sleeper_woken_early_leaves_no_deadline_behind_body() {
    // `Task#resume` wakes a sleeper before its deadline. What the scheduler reports afterwards
    // (the next wakeup, `Task.stat`) is what it reported before the queues changed shape: the
    // resumed task's deadline stays the recorded next wakeup until the clock passes it.
    let mut vm = host_vm(r#"
      $order = []
      $s = Task.new(name: "s") { sleep_ms 40; $order << :s }
      Task.new(name: "t") { sleep_ms 80; $order << :t }
    "#);
    drain(&mut vm);
    assert_eq!(vm.task_next_wakeup_ticks(), Some(10));
    run(&mut vm, "$s.resume");
    drain(&mut vm);
    assert_eq!(inspect(&mut vm, "$order"), "[:s]");
    assert_eq!(inspect(&mut vm, "Task.stat[:wakeup_tick]"), "10");
    vm.task_advance_ticks(10);
    assert_eq!(vm.task_next_wakeup_ticks(), Some(10));
    vm.task_advance_ticks(10);
    drain(&mut vm);
    assert_eq!(inspect(&mut vm, "$order"), "[:s, :t]");
    assert_eq!(vm.task_next_wakeup_ticks(), None);
}

fn readers_of_one_queue_are_woken_in_the_order_they_waited_body() {
    let mut vm = host_vm(r#"
      $order = []
      $q = Task::Queue.new
      $other = Task::Queue.new
      Task.new(name: "r1") { $order << [:r1, $q.pop] }
      Task.new(name: "o")  { $order << [:o, $other.pop] }
      Task.new(name: "r2") { $order << [:r2, $q.pop] }
      Task.new(name: "r3") { $order << [:r3, $q.pop] }
    "#);
    drain(&mut vm);
    assert_eq!(inspect(&mut vm, "$q.num_waiting"), "3");
    assert_eq!(inspect(&mut vm, "$other.num_waiting"), "1");
    run(&mut vm, "$q.push 1");
    drain(&mut vm);
    assert_eq!(inspect(&mut vm, "$order"), "[[:r1, 1]]");
    run(&mut vm, "$q.push 2; $q.push 3");
    drain(&mut vm);
    assert_eq!(inspect(&mut vm, "$order"), "[[:r1, 1], [:r2, 2], [:r3, 3]]");
    // a close wakes every reader, in order, each answering nil
    run(&mut vm, r#"
      Task.new(name: "r4") { $order << [:r4, $q.pop] }
      Task.new(name: "r5") { $order << [:r5, $q.pop] }
    "#);
    drain(&mut vm);
    run(&mut vm, "$q.close");
    drain(&mut vm);
    assert_eq!(inspect(&mut vm, "$order[3..]"), "[[:r4, nil], [:r5, nil]]");
    assert_eq!(inspect(&mut vm, "$other.num_waiting"), "1");
}

fn a_reader_with_a_timeout_is_woken_by_whichever_comes_first_body() {
    let mut vm = host_vm(r#"
      $order = []
      $q = Task::Queue.new
      Task.new(name: "short") { $order << [:short, $q.pop(timeout_ms: 8)] }
      Task.new(name: "long")  { $order << [:long, $q.pop(timeout_ms: 400)] }
      Task.new(name: "none")  { $order << [:none, $q.pop] }
    "#);
    drain(&mut vm);
    assert_eq!(vm.task_next_wakeup_ticks(), Some(2));
    vm.task_advance_ticks(2);
    drain(&mut vm);
    assert_eq!(inspect(&mut vm, "$order"), "[[:short, nil]]");
    // the push goes to the first reader still waiting — `long`, whose deadline is then dropped
    run(&mut vm, "$q.push :x");
    drain(&mut vm);
    assert_eq!(inspect(&mut vm, "$order"), "[[:short, nil], [:long, :x]]");
    run(&mut vm, "$q.push :y");
    drain(&mut vm);
    assert_eq!(inspect(&mut vm, "$order"), "[[:short, nil], [:long, :x], [:none, :y]]");
    vm.task_advance_ticks(200);
    drain(&mut vm);
    assert_eq!(vm.task_next_wakeup_ticks(), None);
    assert!(!vm.task_pending());
}

fn joiners_are_woken_in_the_order_they_joined_body() {
    let mut vm = host_vm(r#"
      $order = []
      $q = Task::Queue.new
      $w = Task.new(name: "w") { $q.pop; :done }
      Task.new(name: "j1") { $order << [:j1, $w.join] }
      Task.new(name: "x")  { $order << :x }
      Task.new(name: "j2") { $order << [:j2, $w.join] }
    "#);
    drain(&mut vm);
    assert_eq!(inspect(&mut vm, "$order"), "[:x]");
    run(&mut vm, "$q.push 1");
    drain(&mut vm);
    // `join` answers the result as it stood when it parked, nil (`docs/design/gems.md`); what is
    // checked here is who ran first
    assert_eq!(inspect(&mut vm, "$order"), "[:x, [:j1, nil], [:j2, nil]]");
}

fn the_ready_queue_is_priority_first_and_round_robin_within_one_body() {
    let mut vm = host_vm(r#"
      $order = []
      Task.new(name: "a", priority: 200) { 2.times { |i| $order << "a#{i}"; Task.pass } }
      Task.new(name: "b", priority: 100) { 2.times { |i| $order << "b#{i}"; Task.pass } }
      Task.new(name: "c", priority: 100) { 2.times { |i| $order << "c#{i}"; Task.pass } }
      $d = Task.new(name: "d", priority: 150) { 2.times { |i| $order << "d#{i}"; Task.pass } }
      Task.new(name: "e", priority: 100) { 2.times { |i| $order << "e#{i}"; Task.pass } }
    "#);
    // a new priority puts the task at the end of the tasks of that priority
    run(&mut vm, "$d.priority = 100");
    assert_eq!(inspect(&mut vm, "Task.stat[:ready][:tasks].map(&:name)"), r#"["b", "c", "e", "d", "a"]"#);
    drain(&mut vm);
    assert_eq!(inspect(&mut vm, "$order"),
        r#"["b0", "c0", "e0", "d0", "b1", "c1", "e1", "d1", "a0", "a1"]"#);
}

fn task_list_and_stat_list_each_queue_in_its_order_body() {
    let mut vm = host_vm(r#"
      $q = Task::Queue.new
      $keep = []
      $keep << Task.new(name: "w1") { $q.pop }
      $keep << Task.new(name: "s1") { sleep }
      $keep << Task.new(name: "w2") { sleep_ms 100 }
      $keep << Task.new(name: "f1") { :f }
      $keep << Task.new(name: "w3") { $q.pop }
      $keep << Task.new(name: "s2") { sleep }
      $keep << Task.new(name: "r1") { Task.pass; $q.pop }
    "#);
    // one turn each for the first six: they park, suspend themselves or finish
    for _ in 0..6 { vm.task_run_once().expect("run_once"); }
    let stat = inspect(&mut vm, "s = Task.stat; [:dormant, :ready, :waiting, :suspended].map { |k| s[k][:tasks].map(&:name) }");
    assert_eq!(stat, r#"[["f1"], ["r1"], ["w1", "w2", "w3"], ["s1", "s2"]]"#);
    drain(&mut vm);
    let stat = inspect(&mut vm, "s = Task.stat; [:dormant, :ready, :waiting, :suspended].map { |k| s[k][:tasks].map(&:name) }");
    assert_eq!(stat, r#"[["f1"], [], ["w1", "w2", "w3", "r1"], ["s1", "s2"]]"#);
    assert_eq!(inspect(&mut vm, "Task.list.map(&:name)"), r#"["f1", "w1", "w2", "w3", "r1", "s1", "s2"]"#);
    assert_eq!(inspect(&mut vm, "Task.get('w3').name"), r#""w3""#);
    run(&mut vm, "Task.get('s1').resume; $q.push 1");
    let stat = inspect(&mut vm, "s = Task.stat; [:dormant, :ready, :waiting, :suspended].map { |k| s[k][:tasks].map(&:name) }");
    assert_eq!(stat, r#"[["f1"], ["s1", "w1"], ["w2", "w3", "r1"], ["s2"]]"#);
}

// ------------------------------------------------------------------ what one turn costs

/// `n` tasks parked on queues of their own (the shape of rubevy's scripts waiting for the host's
/// answer), `n` more sleeping far into the future, and one task that loops: parks on its own
/// queue, is woken by a push, runs, parks again.
#[test]
fn sleepers_woken_together_run_in_the_order_they_went_to_sleep() { both(sleepers_woken_together_run_in_the_order_they_went_to_sleep_body) }

#[test]
fn a_sleeper_woken_early_leaves_no_deadline_behind() { both(a_sleeper_woken_early_leaves_no_deadline_behind_body) }

#[test]
fn readers_of_one_queue_are_woken_in_the_order_they_waited() { both(readers_of_one_queue_are_woken_in_the_order_they_waited_body) }

#[test]
fn a_reader_with_a_timeout_is_woken_by_whichever_comes_first() { both(a_reader_with_a_timeout_is_woken_by_whichever_comes_first_body) }

#[test]
fn joiners_are_woken_in_the_order_they_joined() { both(joiners_are_woken_in_the_order_they_joined_body) }

#[test]
fn the_ready_queue_is_priority_first_and_round_robin_within_one() { both(the_ready_queue_is_priority_first_and_round_robin_within_one_body) }

#[test]
fn task_list_and_stat_list_each_queue_in_its_order() { both(task_list_and_stat_list_each_queue_in_its_order_body) }

fn crowd(n: usize) -> Vm {
    let mut vm = new_vm();
    vm.task_external_clock(true);
    run(&mut vm, &format!(r#"
      $qs = Array.new({n}) {{ Task::Queue.new }}
      $qs.each {{ |q| Task.new {{ q.pop }} }}
      {n}.times {{ Task.new {{ sleep 100000 }} }}
      $mine = Task::Queue.new
      $turns = 0
      Task.new(name: "mine") {{ loop {{ $mine.pop; $turns += 1 }} }}
      $bell = Task::Queue.new
      Task.new(name: "bell") {{ loop {{ $bell.pop(timeout_ms: 4); $turns += 1 }} }}
    "#));
    drain(&mut vm);
    vm
}

/// Elements walked by: a push that wakes the one task waiting on that queue, and the turn that
/// runs it until it parks again; then a tick that wakes the one sleeper whose deadline came.
fn one_turn(vm: &mut Vm) -> (u64, u64) {
    let mine = match run(vm, "$mine") { Value::Obj(o) => o, v => panic!("not a queue: {v:?}") };
    vm.gc_register(mine);
    let before = vm.task.walked;
    vm.task_queue_push(mine, Value::Int(1)).expect("push");
    vm.task_run_once().expect("run_once");
    let push = vm.task.walked - before;
    let before = vm.task.walked;
    vm.task_advance_ticks(1);
    vm.task_run_once().expect("run_once");
    let tick = vm.task.walked - before;
    vm.gc_unregister(mine);
    (push, tick)
}

/// `n` tasks that each give the CPU back at every turn (`Task.pass`): all of them ready at once,
/// which is what a frame loop's scripts are between two waits.
fn ready_crowd(n: usize) -> Vm {
    let mut vm = new_vm();
    vm.task_external_clock(true);
    run(&mut vm, &format!("$ts = Array.new({n}) {{ Task.new {{ loop {{ Task.pass }} }} }}"));
    vm
}

/// Elements walked by one task's turn when every task is ready, and by asking a task its status
/// (`Task#status` first checks that the scheduler still knows the task).
fn one_ready_turn(vm: &mut Vm) -> (u64, u64) {
    let before = vm.task.walked;
    vm.task_run_once().expect("run_once");
    let turn = vm.task.walked - before;
    let before = vm.task.walked;
    assert_eq!(inspect(vm, "$ts.last.status"), ":READY");
    (turn, vm.task.walked - before)
}

#[test]
fn a_waiting_task_costs_nothing_per_turn_of_another() {
    let mut small = crowd(100);
    let mut large = crowd(1000);
    // warm up: the first turn of each loop is not the steady state
    one_turn(&mut small);
    one_turn(&mut large);
    let s = one_turn(&mut small);
    let l = one_turn(&mut large);
    assert_eq!(run(&mut large, "$turns"), Value::Int(4));
    eprintln!("walked per turn: 100 waiting -> push {} tick {}; 1000 waiting -> push {} tick {}", s.0, s.1, l.0, l.1);
    assert_eq!(s, l, "one turn walked more queue elements with more tasks waiting");
}

#[test]
fn a_ready_task_costs_the_same_turn_with_more_tasks_ready() {
    let mut small = ready_crowd(100);
    let mut large = ready_crowd(1000);
    for _ in 0..3 { one_ready_turn(&mut small); one_ready_turn(&mut large); }
    let s = one_ready_turn(&mut small);
    let l = one_ready_turn(&mut large);
    eprintln!("walked per ready turn: 100 ready -> turn {} status {}; 1000 ready -> turn {} status {}", s.0, s.1, l.0, l.1);
    assert_eq!(s, l, "one turn walked more queue elements with more tasks ready");
}
