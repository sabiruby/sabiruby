//! mruby-task's scheduler from the host's side: `Vm::task_run_once`, which is the shape a frame
//! loop wants where `Task.run` runs until every task is done. The Ruby surface is checked by the
//! reference's own tests (`tools/mrbtest.sh`, `gem_task`, `gem_queue`, `gem_gc_task`).

fn vm_with(src: &str) -> sabiruby::Vm {
    let bin = sabiruby_compiler::compile(src.as_bytes(), &sabiruby_compiler::Options {
        filename: "(test)".into(), debug_info: true, ..Default::default()
    }).expect("compile");
    let mut vm = sabiruby::Vm::with_mrblib().expect("vm");
    vm.load_and_run(&bin).expect("run");
    vm
}

#[test]
fn run_once_advances_one_task_at_a_time() {
    let mut vm = vm_with(r#"
      $order = []
      Task.new(name: "a") { $order << :a1; Task.pass; $order << :a2 }
      Task.new(name: "b") { $order << :b1 }
    "#);
    // one ready task per call, and nil once there is nothing left to run
    let mut turns = 0;
    while !vm.task_run_once().expect("run_once").is_nil() {
        turns += 1;
        assert!(turns < 10, "the scheduler never went idle");
    }
    let out = String::from_utf8_lossy(&vm.take_output()).into_owned();
    assert_eq!(out, "");
    let bin = sabiruby_compiler::compile(b"p $order\n", &sabiruby_compiler::Options {
        filename: "(test)".into(), debug_info: true, ..Default::default()
    }).expect("compile");
    vm.load_and_run(&bin).expect("run");
    assert_eq!(String::from_utf8_lossy(&vm.take_output()), "[:a1, :b1, :a2]\n");
}

#[test]
fn a_timeslice_is_a_fixed_amount_of_work() {
    // there is no timer here, so the tick is the instruction count: a task that never yields is
    // preempted all the same (`docs/design/gems.md`)
    let mut vm = vm_with(r#"
      $order = []
      Task.new { 200000.times { |i| $order << :a if i == 0 }; $order << :a_end }
      Task.new { $order << :b }
      Task.run
      p $order
    "#);
    assert_eq!(String::from_utf8_lossy(&vm.take_output()), "[:a, :b, :a_end]\n");
}

#[test]
fn a_host_spawns_tasks_and_drives_the_clock() {
    // the shape a frame loop wants: the host makes the tasks out of compiled programs, gives the
    // scheduler a budget per frame, and moves the clock on by the frame time
    let mut vm = sabiruby::Vm::with_mrblib().expect("vm");
    vm.task_external_clock(true);
    let spawn = |vm: &mut sabiruby::Vm, src: &str, name: &str| {
        let bin = sabiruby_compiler::compile(src.as_bytes(), &sabiruby_compiler::Options {
            filename: name.into(), debug_info: true, ..Default::default()
        }).expect("compile");
        let irep = vm.load(&bin).expect("load");
        let t = vm.task_spawn(irep, 128, Some(name)).expect("spawn");
        vm.gc_register(t);
        t
    };
    let a = spawn(&mut vm, "$order = ($order || []) << :a_start\nsleep 0.1\n$order << :a_woke\n:a_done\n", "a");
    let b = spawn(&mut vm, "$order = ($order || []) << :b\n:b_done\n", "b");

    // frame 1: both run, `a` parks on its sleep and `b` finishes
    vm.task_run_budget(100_000).expect("frame");
    assert!(!vm.task_finished(a), "a must be sleeping, not done");
    assert!(vm.task_finished(b));
    assert_eq!(vm.inspect_str(vm.task_value(b)).unwrap(), ":b_done");

    // a few frames of 16 ms each: nothing is ready until the sleep is over
    let per_frame = 16 / vm.task_tick_unit_ms();
    for _ in 0..3 {
        vm.task_advance_ticks(per_frame);
        vm.task_run_budget(100_000).expect("frame");
    }
    assert!(!vm.task_finished(a), "0.1 s has not passed yet");
    for _ in 0..4 {
        vm.task_advance_ticks(per_frame);
        vm.task_run_budget(100_000).expect("frame");
    }
    assert!(vm.task_finished(a), "the sleep should be over");
    assert_eq!(vm.inspect_str(vm.task_value(a)).unwrap(), ":a_done");

    let bin = sabiruby_compiler::compile(b"p $order\n", &sabiruby_compiler::Options {
        filename: "(test)".into(), debug_info: true, ..Default::default()
    }).expect("compile");
    vm.load_and_run(&bin).expect("run");
    assert_eq!(String::from_utf8_lossy(&vm.take_output()), "[:a_start, :b, :a_woke]\n");
}

#[test]
fn a_budget_bounds_one_turn_of_a_host_loop() {
    // a task that never yields is preempted at its timeslice, so a frame is not lost to it
    let mut vm = sabiruby::Vm::with_mrblib().expect("vm");
    vm.task_external_clock(true);
    let bin = sabiruby_compiler::compile(b"i = 0\nwhile true\n  i += 1\nend\n", &sabiruby_compiler::Options {
        filename: "spin".into(), debug_info: true, ..Default::default()
    }).expect("compile");
    let irep = vm.load(&bin).expect("load");
    let t = vm.task_spawn(irep, 128, Some("spin")).expect("spawn");
    vm.gc_register(t);
    let spent = vm.task_run_budget(50_000).expect("frame");
    assert!(spent >= 50_000 && spent < 200_000, "spent {spent}");
    assert!(!vm.task_finished(t));
}

#[test]
fn a_task_that_raises_keeps_the_scheduler_going() {
    let mut vm = vm_with(r#"
      t = Task.new { raise "boom" }
      Task.new { p :other }
      Task.run
      p [t.value.class, t.value.message, t.status]
    "#);
    assert_eq!(String::from_utf8_lossy(&vm.take_output()), ":other\n[RuntimeError, \"boom\", :DORMANT]\n");
}

#[test]
fn a_call_that_parks_answers_its_own_value() {
    // the switch is deferred to the next instruction boundary, as the reference defers it, so the
    // value the native returned is stored first (`docs/design/gems.md`, mruby-task)
    let mut vm = vm_with(r#"
      done = Task.new(name: "done") { :done_now }
      Task.run
      a = Task.new(name: "a") { sleep(0.05); :done_a }
      Task.new(name: "b") { p [Task.pass, sleep(0.01), a.join, done.join] }
      Task.run
      begin; a.join; rescue => e; p [e.class, e.message]; end
      begin; Task.new { }.join; rescue => e; p e.class; end
    "#);
    assert_eq!(
        String::from_utf8_lossy(&vm.take_output()),
        // Task.pass is nil, sleep answers the seconds asked for, a join that waited answers the
        // result as it stood (nil), a join on a task already done answers its result
        "[nil, 0, nil, :done_now]\n\
         [RuntimeError, \"join can only be called from running task\"]\n\
         RuntimeError\n"
    );
}

#[test]
fn a_host_waits_on_its_own_clock() {
    // what a browser or a frame loop needs to sleep for real: the scheduler says how long it may
    // wait and whether anything is left, and the host moves the clock (`docs/design/playground.md`)
    let mut vm = vm_with(r#"
      $log = []
      Task.new(name: "slow") { 2.times { |i| $log << "slow#{i}"; sleep 0.1 } }
      Task.new(name: "fast") { 4.times { |i| $log << "fast#{i}"; sleep 0.05 } }
    "#);
    vm.task_external_clock(true);
    let unit = vm.task_tick_unit_ms(); // 4 ms
    let mut clock_ms = 0u32;
    let mut waits = 0;
    while vm.task_pending() {
        vm.task_run_budget(100_000).expect("run");
        match vm.task_next_wakeup_ticks() {
            // nothing to run: a host would wait this long before coming back, and the clock is
            // moved by what it waited — never jumped by the scheduler itself
            Some(ticks) if ticks > 0 => {
                clock_ms += ticks * unit;
                vm.task_advance_ticks(ticks);
                waits += 1;
                assert!(waits < 20, "the host was asked to wait too many times");
            }
            Some(_) => {}
            None => break,
        }
    }
    // 2 x 100 ms and 4 x 50 ms, sleeping in parallel. A wait is rounded up to whole ticks, so
    // 50 ms is 13 of them (52 ms) and the last deadline is the fast task's 4th, at 208 ms
    assert_eq!(clock_ms, 208);
    let bin = sabiruby_compiler::compile(b"p $log\n", &sabiruby_compiler::Options {
        filename: "(test)".into(), debug_info: true, ..Default::default()
    }).expect("compile");
    vm.load_and_run(&bin).expect("run");
    assert_eq!(
        String::from_utf8_lossy(&vm.take_output()),
        "[\"slow0\", \"fast0\", \"fast1\", \"slow1\", \"fast2\", \"fast3\"]\n"
    );
}

#[test]
fn the_scheduler_is_not_re_entered_from_inside_a_task() {
    // `Task.run` from a task would take the head of the ready queue — the running task itself —
    // and resume the context it is standing in. The reference's own loop says "already running"
    // with a flag; a host that drives the scheduler a step at a time leaves that flag clear, so
    // the caller is asked instead (`docs/design/gems.md`).
    let src = r#"
      Task.new(name: "a") { 2.times { |i| puts "a#{i}"; Task.pass }; :a }
      p Task.run
      puts "after"
    "#;
    let bin = sabiruby_compiler::compile(src.as_bytes(), &sabiruby_compiler::Options {
        filename: "(test)".into(), debug_info: true, ..Default::default()
    }).expect("compile");
    let mut vm = sabiruby::Vm::with_mrblib().expect("vm");
    let irep = vm.load(&bin).expect("load");
    let main = vm.task_spawn(irep, 128, Some("main")).expect("spawn");
    vm.gc_register(main);
    let mut turns = 0;
    while vm.task_pending() {
        vm.task_run_budget(100_000).expect("run");
        turns += 1;
        assert!(turns < 20, "the host loop did not finish");
    }
    // the program's own `Task.run` answered nil and went on; the host ran the other task
    assert_eq!(String::from_utf8_lossy(&vm.take_output()), "nil\nafter\na0\na1\n");
}

#[test]
fn a_fiber_runs_inside_a_task() {
    // The reference says not to mix the two (its `MRB2TASK` is pointer arithmetic on the running
    // context, and a timeslice that expires inside a fiber leaves it orphaned). Neither applies
    // here, and the gems are useful together, so this pins that it works (`docs/design/gems.md`).
    let mut vm = vm_with(r#"
      $log = []
      Task.new(name: "a") do
        f = Fiber.new { 3.times { |i| Fiber.yield i }; :fiber_done }
        $log << [f.resume, f.resume, f.resume, f.resume, f.alive?]
        e = [10, 20, 30].each            # Enumerator#next is a fiber underneath
        $log << [e.next, e.next, e.next]
        :a_done
      end
      Task.new(name: "b") { $log << :b; :b_done }
      Task.run
      p $log
    "#);
    assert_eq!(
        String::from_utf8_lossy(&vm.take_output()),
        "[[0, 1, 2, :fiber_done, false], [10, 20, 30], :b]\n"
    );
}

#[test]
fn a_task_is_not_preempted_while_it_is_inside_a_fiber() {
    // A fiber is entered through a native frame, and the switch is deferred across one (the same
    // rule as `task_across_c_boundary`), so the resume answers its own value instead of being cut
    // short. The cost is fairness: the work inside a fiber is not divided into timeslices.
    let mut vm = vm_with(r#"
      $log = []
      Task.new(name: "fiber") { Fiber.new { 200_000.times { |i| i }; :done }.resume; $log << :fiber_end }
      Task.new(name: "plain") { 200_000.times { |i| i }; $log << :plain_end }
      Task.new(name: "watch") { 6.times { |i| $log << i; Task.pass } }
      Task.run
      p $log
    "#);
    // the fiber's 200,000 instructions run in one go; the same loop outside one is preempted
    assert_eq!(
        String::from_utf8_lossy(&vm.take_output()),
        "[0, :fiber_end, 1, 2, 3, 4, 5, :plain_end]\n"
    );
}

#[test]
fn a_host_answers_a_script_through_a_queue() {
    // an asynchronous host operation: the script asks, is parked on `pop`, and the host pushes
    // the answer when it has one — several frames later, with other tasks running meanwhile
    let mut vm = vm_with(r#"
      $asked = []
      def scan(q) = q.pop           # parks this task until the host answers
      Task.new(name: "robot") do
        3.times { |i| $asked << [i, scan($queue)] }
        :robot_done
      end
      Task.new(name: "other") { 6.times { |i| $asked << "tick#{i}"; Task.pass }; :other_done }
    "#);
    let q = vm.task_queue_new().expect("queue");
    vm.gc_register(q);
    let qn = vm.intern("$queue");
    vm.globals.insert(qn, sabiruby::value::Slot::from(sabiruby::value::Value::Obj(q)));

    // the host loop: run what is ready, then answer one pending request per turn
    for i in 0..3 {
        vm.task_run_budget(100_000).expect("run");
        vm.task_queue_push(q, sabiruby::value::Value::Int(100 + i)).expect("push");
    }
    vm.task_run_budget(100_000).expect("run");

    let bin = sabiruby_compiler::compile(b"p $asked\n", &sabiruby_compiler::Options {
        filename: "(test)".into(), debug_info: true, ..Default::default()
    }).expect("compile");
    vm.load_and_run(&bin).expect("run");
    // the robot parked on the first `pop`, so the other task ran to its end meanwhile; the three
    // answers are the host's, in the order it pushed them
    assert_eq!(
        String::from_utf8_lossy(&vm.take_output()),
        "[\"tick0\", \"tick1\", \"tick2\", \"tick3\", \"tick4\", \"tick5\", [0, 100], [1, 101], [2, 102]]\n"
    );
}

#[test]
fn a_host_can_see_what_a_task_spends_and_where_it_is() {
    // what a game's HUD shows per script: the instructions it has run, and the line it is on —
    // which works while it is parked as well as while it runs
    let src = "$log = []\n\
               Task.new(name: \"busy\") { 200.times { |i| $log << i }; :busy_done }\n\
               Task.new(name: \"idle\") { sleep 10; :idle_done }\n";
    let bin = sabiruby_compiler::compile(src.as_bytes(), &sabiruby_compiler::Options {
        filename: "robots.rb".into(), debug_info: true, ..Default::default()
    }).expect("compile");
    let mut vm = sabiruby::Vm::with_mrblib().expect("vm");
    vm.load_and_run(&bin).expect("run");
    let (busy, idle) = {
        let names = ["busy", "idle"];
        let mut found = [None, None];
        for (i, name) in names.into_iter().enumerate() {
            let src = format!("Task.get(\"{name}\")\n");
            let bin = sabiruby_compiler::compile(src.as_bytes(), &sabiruby_compiler::Options {
                filename: "(test)".into(), debug_info: true, ..Default::default()
            }).expect("compile");
            found[i] = vm.load_and_run(&bin).expect("run").obj();
        }
        (found[0].expect("busy"), found[1].expect("idle"))
    };
    assert_eq!(vm.task_instructions(busy), 0, "nothing has run yet");

    // the host owns the clock, so the sleeping task stays where it is instead of being woken
    // the moment nothing else is ready
    vm.task_external_clock(true);
    vm.task_run_budget(50_000).expect("run");
    assert!(vm.task_instructions(busy) > 100, "the busy task ran: {}", vm.task_instructions(busy));
    assert!(vm.task_instructions(idle) < vm.task_instructions(busy), "the sleeping one spent less");

    // the parked task is standing on the line its `sleep` is on, in the file it was compiled as
    let (file, line) = vm.task_location(idle).expect("a location for the sleeping task");
    assert_eq!(file, "robots.rb");
    assert_eq!(line, 3, "the `sleep 10` line");

    // and the frames below it are there for a host that shows one file of several
    let frames = vm.task_frames(idle);
    assert_eq!(frames.first(), Some(&("robots.rb".to_string(), 3)));
    assert!(frames.len() >= 1, "at least the frame it stands in");
}

// ------------------------------------------------------------------ time limits

use std::sync::atomic::{AtomicU64, Ordering};

fn spawn_src(vm: &mut sabiruby::Vm, src: &str, name: &str) -> sabiruby::value::ObjId {
    let bin = sabiruby_compiler::compile(src.as_bytes(), &sabiruby_compiler::Options {
        filename: name.into(), debug_info: true, ..Default::default()
    }).expect("compile");
    let irep = vm.load(&bin).expect("load");
    let t = vm.task_spawn(irep, 128, Some(name)).expect("spawn");
    vm.gc_register(t);
    t
}

fn class_name(vm: &mut sabiruby::Vm, v: sabiruby::Value) -> String {
    let class = vm.intern("class");
    let c = vm.funcall(v, class, &[], sabiruby::Value::Nil).expect("class");
    vm.inspect_str(c).expect("inspect")
}

/// A `to_s` that never returns: `[Stuck.new].join` is stuck under a native (`Array#join` calling
/// it back), which is still a native boundary.
const STUCK: &str = "class Stuck\n  def to_s\n    loop { }\n  end\nend\n";

#[test]
fn a_task_stuck_in_a_block_under_a_native_gets_overrun() {
    // `Array#join` is a native waiting for the `to_s` it called, so the task cannot be switched
    // out while that runs, and a timeslice never ends. Past the run's hard limit it gets
    // Task::Overrun instead of taking the host loop with it. Counted in instructions here, so no
    // clock is needed. (This used `Array.new(1) { loop { } }` until `Array.new` stopped being a
    // native boundary: `docs/design/wait-anywhere.md`.)
    let mut vm = sabiruby::Vm::with_mrblib().expect("vm");
    vm.task_external_clock(true);
    let stuck = spawn_src(&mut vm, &format!("{STUCK}[Stuck.new].join"), "stuck");
    let other = spawn_src(&mut vm, "$n = 0\nloop { $n += 1; Task.pass }", "other");
    let limits = sabiruby::RunLimits { instructions: Some(50_000), overrun_instructions: Some(300_000), ..Default::default() };
    let spent = vm.task_run_limits(limits).expect("frame");
    assert!(spent < 400_000, "the run came back near its hard limit, spent {spent}");
    assert!(vm.task_finished(stuck));
    let v = vm.task_value(stuck);
    assert_eq!(class_name(&mut vm, v), "Task::Overrun");
    let before = vm.task_instructions(other);
    vm.task_run_limits(limits).expect("next frame");
    assert!(vm.task_instructions(other) > before, "the other task keeps running");
}

#[test]
fn overrun_is_not_a_standard_error_and_a_task_that_rescues_it_still_yields() {
    let mut vm = sabiruby::Vm::with_mrblib().expect("vm");
    vm.task_external_clock(true);
    let plain = spawn_src(&mut vm, &format!("{STUCK}begin\n  [Stuck.new].join\nrescue => e\n  $plain = e\nend"), "plain");
    let limits = sabiruby::RunLimits { overrun_instructions: Some(100_000), ..Default::default() };
    // the exception unwinds out of the native into the `rescue` clause, which does not match it;
    // the switch Overrun leaves due is taken at that boundary, before the re-raise, so the task
    // ends on its next turn
    vm.task_run_limits(limits).expect("frame");
    vm.task_run_limits(limits).expect("next frame");
    assert!(vm.task_finished(plain), "`rescue => e` does not catch it");
    let v = vm.task_value(plain);
    assert_eq!(class_name(&mut vm, v), "Task::Overrun");

    // one that rescues Exception and walks back into the same call is switched out every turn
    // rather than holding the host loop
    let stubborn = spawn_src(&mut vm,
        &format!("{STUCK}$caught = 0\nloop do\n  begin\n    [Stuck.new].join\n  rescue Exception\n    $caught += 1\n  end\nend"), "stubborn");
    for _ in 0..3 {
        let spent = vm.task_run_limits(limits).expect("frame");
        assert!(spent < 200_000, "the run ends at its hard limit: spent {spent}");
    }
    assert!(!vm.task_finished(stubborn));
    let bin = sabiruby_compiler::compile(b"p $caught\n", &sabiruby_compiler::Options::default()).expect("compile");
    vm.load_and_run(&bin).expect("run");
    // each turn raises once; the rescue clause of one turn runs at the start of the next, since
    // the switch is taken at the boundary the exception lands on
    assert_eq!(String::from_utf8_lossy(&vm.take_output()), "2\n");
}

static SLICE_CLOCK: AtomicU64 = AtomicU64::new(0);
fn slice_clock() -> u64 { SLICE_CLOCK.fetch_add(1_000_000, Ordering::SeqCst) }

#[test]
fn a_time_slice_ends_on_the_host_clock_not_the_instruction_count() {
    // a clock that moves 1 ms every time it is read, so the test does not depend on the machine
    let run = |timeslice: sabiruby::Timeslice| {
        let mut vm = sabiruby::Vm::with_mrblib().expect("vm");
        vm.task_external_clock(true);
        vm.task_set_clock(Some(slice_clock));
        vm.task_set_timeslice(timeslice);
        let spin = spawn_src(&mut vm, "i = 0\nwhile true\n  i += 1\nend", "spin");
        spawn_src(&mut vm, "p :other", "other");
        vm.task_run_once().expect("one slice");
        vm.task_instructions(spin)
    };
    // three ticks of 10,000 instructions
    let counted = run(sabiruby::Timeslice::Instructions);
    assert!((29_000..40_000).contains(&counted), "instructions: {counted}");
    // 50 ms, read once a tick: about fifty ticks
    let timed = run(sabiruby::Timeslice::Time { nanos: 50_000_000 });
    assert!((400_000..600_000).contains(&timed), "time: {timed}");
    // whichever comes first
    let both = run(sabiruby::Timeslice::Both { nanos: 50_000_000 });
    assert!((29_000..40_000).contains(&both), "both: {both}");
}

static NATIVE_CLOCK: AtomicU64 = AtomicU64::new(0);
fn native_clock() -> u64 { NATIVE_CLOCK.fetch_add(1_000_000, Ordering::SeqCst) }

#[test]
fn natives_count_towards_a_look_at_the_clock_between_ticks() {
    // with ticks all but switched off, the clock is still read every 32 natives, which is what
    // notices a native that takes long
    let mut vm = sabiruby::Vm::with_mrblib().expect("vm");
    vm.task_external_clock(true);
    vm.task_set_clock(Some(native_clock));
    vm.task_set_timeslice(sabiruby::Timeslice::Time { nanos: 5_000_000 });
    vm.task.tick_every = 100_000_000;
    vm.task.tick_left = 100_000_000;
    let spin = spawn_src(&mut vm, "a = []\nwhile true\n  a.push(1)\n  a.pop\nend", "natives");
    spawn_src(&mut vm, "p :other", "other");
    vm.task_run_once().expect("one slice");
    let spent = vm.task_instructions(spin);
    assert!(spent < 5_000, "the slice ended after a few hundred natives, not a tick: {spent}");
}

static SOFT_CLOCK: AtomicU64 = AtomicU64::new(0);
fn soft_clock() -> u64 { SOFT_CLOCK.fetch_add(1_000_000, Ordering::SeqCst) }

#[test]
fn a_time_budget_cuts_the_running_slice_short_and_limits_go_with_the_run() {
    let mut vm = sabiruby::Vm::with_mrblib().expect("vm");
    vm.task_external_clock(true);
    vm.task_set_clock(Some(soft_clock));
    spawn_src(&mut vm, "i = 0\nwhile true\n  i += 1\nend", "spin");
    // the budget is up at the first look at the clock, a tick into the slice
    let limits = sabiruby::RunLimits { time_ns: Some(1_500_000), ..Default::default() };
    let spent = vm.task_run_limits(limits).expect("frame");
    assert!(spent < 25_000, "cut at the first tick rather than the end of the slice: {spent}");
    // a run without limits leaves the next one alone
    assert!(vm.task.run_soft.is_none() && vm.task.run_hard.is_none());
    let spent = vm.task_run_budget(1).expect("plain run");
    assert!(spent >= 29_000, "a whole slice again: {spent}");
}

#[test]
fn time_limits_need_a_clock() {
    // without one the time fields are ignored and the scheduler behaves as it always did
    let mut vm = sabiruby::Vm::with_mrblib().expect("vm");
    vm.task_external_clock(true);
    vm.task_set_timeslice(sabiruby::Timeslice::Time { nanos: 1 });
    let spin = spawn_src(&mut vm, "i = 0\nwhile true\n  i += 1\nend", "spin");
    let limits = sabiruby::RunLimits { instructions: Some(1), time_ns: Some(1), overrun_ns: Some(1), ..Default::default() };
    vm.task_run_limits(limits).expect("frame");
    assert!((29_000..40_000).contains(&vm.task_instructions(spin)));
    assert!(!vm.task_finished(spin));
}

#[test]
fn a_task_parks_on_a_queue_from_inside_method_missing() {
    // The point of dispatching `method_missing` in the calling frame (stage 6c of
    // `docs/plans/host-bridge-plan.md`): a proxy written with `method_missing` can block on
    // the host's answer. While `method_missing` ran in a nested run loop there was a native
    // boundary under the task, and `Queue#pop` refused with "blocking pop cannot be called
    // from within a C function boundary".
    let mut vm = vm_with(r##"
      $asked = []
      class Proxy
        def initialize(kind) = @kind = kind
        def method_missing(name, *args)
          $asked << ["#{@kind}.#{name}", args]
          $queue.pop
        end
        def respond_to_missing?(name, include_private = false) = true
      end
      $got = []
      Task.new(name: "robot") do
        robot = Proxy.new("robot")
        $got << robot.move_to(1, 2)
        $got << robot.hp
        :robot_done
      end
      Task.new(name: "other") { 4.times { |i| $asked << "tick#{i}"; Task.pass }; :other_done }
    "##);
    let q = vm.task_queue_new().expect("queue");
    vm.gc_register(q);
    vm.global_set("$queue", sabiruby::value::Value::Obj(q));

    for i in 0..2 {
        vm.task_run_budget(100_000).expect("run");
        vm.task_queue_push(q, sabiruby::value::Value::Int(70 + i)).expect("push");
    }
    vm.task_run_budget(100_000).expect("run");

    let bin = sabiruby_compiler::compile(b"p $asked; p $got\n", &sabiruby_compiler::Options {
        filename: "(test)".into(), debug_info: true, ..Default::default()
    }).expect("compile");
    vm.load_and_run(&bin).expect("run");
    // the two calls reach the host under `kind.name`, the task is parked between them (the
    // other task keeps ticking), and each answer comes back as the value of the call
    assert_eq!(
        String::from_utf8_lossy(&vm.take_output()),
        "[[\"robot.move_to\", [1, 2]], \"tick0\", \"tick1\", \"tick2\", \"tick3\", [\"robot.hp\", []]]\n[70, 71]\n"
    );
}

#[test]
fn a_task_sleeps_from_inside_method_missing() {
    // `sleep 0` is the other suspension point a task has; it needs the same frame.
    let mut vm = vm_with(r#"
      $order = []
      class Slow
        def method_missing(name, *args)
          $order << [:before, name]
          sleep 0
          $order << [:after, name]
          name
        end
      end
      Task.new(name: "a") { $order << [:done, Slow.new.nap] }
      Task.new(name: "b") { $order << :b }
      Task.run
      p $order
    "#);
    // `sleep 0` gives the turn up, so b runs in between
    assert_eq!(
        String::from_utf8_lossy(&vm.take_output()),
        "[[:before, :nap], :b, [:after, :nap], [:done, :nap]]\n"
    );
}

#[test]
fn a_task_parks_on_a_queue_from_inside_a_ruby_aref() {
    // The point of sending `[]` from the index opcodes instead of running it in a nested run
    // loop (`docs/worklog/2026-09-15-getidx-dispatch.md`): rubevy writes `e[:Transform]`, the
    // `[]` behind it asks the host and waits for the answer. While `OP_GETIDX` went through
    // `Vm::funcall` there was a native boundary under the task and `Queue#pop` refused with
    // "blocking pop cannot be called from within a C function boundary".
    let mut vm = vm_with(r##"
      $asked = []
      class Entity
        def initialize(id) = @id = id
        def [](name)
          $asked << [@id, name]
          $queue.pop
        end
        def []=(name, value)
          $asked << [@id, name, value]
          $queue.pop
        end
      end
      $got = []
      Task.new(name: "reader") do
        e = Entity.new(7)
        $got << e[:Transform]
        $got << (e[:Health] = 3)
        :reader_done
      end
      Task.new(name: "other") { 4.times { |i| $asked << "tick#{i}"; Task.pass }; :other_done }
    "##);
    let q = vm.task_queue_new().expect("queue");
    vm.gc_register(q);
    vm.global_set("$queue", sabiruby::value::Value::Obj(q));

    for i in 0..2 {
        vm.task_run_budget(100_000).expect("run");
        vm.task_queue_push(q, sabiruby::value::Value::Int(70 + i)).expect("push");
    }
    vm.task_run_budget(100_000).expect("run");

    let bin = sabiruby_compiler::compile(b"p $asked; p $got\n", &sabiruby_compiler::Options {
        filename: "(test)".into(), debug_info: true, ..Default::default()
    }).expect("compile");
    vm.load_and_run(&bin).expect("run");
    // the read parks, the other task ticks while it waits, the answer comes back as the value
    // of `e[:Transform]`, and the assignment is still worth its right-hand side (3, not 71)
    assert_eq!(
        String::from_utf8_lossy(&vm.take_output()),
        "[[7, :Transform], \"tick0\", \"tick1\", \"tick2\", \"tick3\", [7, :Health, 3]]\n[70, 3]\n"
    );
}

#[test]
fn a_task_sleeps_from_inside_a_ruby_aref() {
    // `sleep 0` is the other suspension point a task has; it needs the same frame. `x[0]` is
    // `OP_GETIDX0`, which has its own fallback (it writes the receiver and the literal 0 into
    // the call's registers before it sends), so it is the one this asks for.
    let mut vm = vm_with(r#"
      $order = []
      class Slow
        def [](i)
          $order << [:before, i]
          sleep 0
          $order << [:after, i]
          i
        end
      end
      Task.new(name: "a") { $order << [:done, Slow.new[0]] }
      Task.new(name: "b") { $order << :b }
      Task.run
      p $order
    "#);
    // `sleep 0` gives the turn up, so b runs in between
    assert_eq!(
        String::from_utf8_lossy(&vm.take_output()),
        "[[:before, 0], :b, [:after, 0], [:done, 0]]\n"
    );
}

#[test]
fn a_burst_of_tasks_ending_with_nil_does_not_end_the_frame() {
    // A task whose block is worth nil (rubevy: a reflex task whose `Queue#pop` raised and whose
    // `rescue` clause is empty) ends with a nil result. `Vm::task_run_once` answers that result,
    // and a frame loop must not read it as "nothing was ready": ten of them ending at once would
    // then cost ten frames, during which nothing else in the VM runs
    // (`docs/worklog/2026-09-17-task-end-nil.md`).
    let mut vm = sabiruby::Vm::with_mrblib().expect("vm");
    vm.task_external_clock(true);
    let mut enders = Vec::new();
    for i in 0..10 {
        enders.push(spawn_src(&mut vm, "nil", &format!("ender{i}")));
    }
    // behind them in the ready queue, at the same priority: FIFO, so it only gets the CPU once
    // every ender has had its turn
    let worker = spawn_src(&mut vm, "$n = 0\nloop { $n += 1; Task.pass }", "worker");
    let limits = sabiruby::RunLimits { instructions: Some(200_000), ..Default::default() };
    let spent = vm.task_run_limits(limits).expect("frame");
    let ran = enders.iter().filter(|t| vm.task_finished(**t)).count();
    assert!(
        vm.task_instructions(worker) > 0,
        "the tasks that ended with nil ate the whole frame: {ran} of 10 of them finished, the \
         worker ran {} instructions, and the frame spent {spent} of its 200000",
        vm.task_instructions(worker)
    );
    for (i, t) in enders.iter().enumerate() {
        assert!(vm.task_finished(*t), "ender{i} did not finish in the frame");
        assert!(vm.task_value(*t).is_nil(), "ender{i} answered something other than nil");
    }
}

#[test]
fn finished_tasks_are_not_kept_for_the_life_of_the_vm() {
    // `stop_task` moves a finished task to the dormant queue, and the queues are GC roots, so
    // nothing but `Task#close` ever dropped one: a host that restarts scripts every few seconds
    // accumulates a Task object (with its result and its name) per restart, forever. A finished
    // task nothing refers to any more is garbage like any other object.
    let mut vm = sabiruby::Vm::with_mrblib().expect("vm");
    vm.task_external_clock(true);
    let bin = sabiruby_compiler::compile(b"nil\n", &sabiruby_compiler::Options {
        filename: "short".into(), debug_info: true, ..Default::default()
    }).expect("compile");
    let irep = vm.load(&bin).expect("load");
    vm.gc_collect();
    let before = vm.heap.live_count();
    for _ in 0..1000 {
        // not `gc_register`ed: the host does not keep these, the scheduler does
        vm.task_spawn(irep, 128, None).expect("spawn");
    }
    let mut frames = 0;
    while vm.task_pending() {
        vm.task_run_limits(sabiruby::RunLimits { instructions: Some(1_000_000), ..Default::default() }).expect("frame");
        frames += 1;
        assert!(frames < 2000, "the scheduler never finished the tasks");
    }
    vm.gc_collect();
    let after = vm.heap.live_count();
    assert!(
        after < before + 200,
        "1000 finished tasks are still live after a collection: {before} objects before, {after} after"
    );
}
