//! The entry points a host uses that have no other public way in: which task is running, an
//! object's instance variables and the globals by name, and whether a value is an exception
//! (`Vm::task_running`, `ivar_get`/`ivar_set`, `global_get`/`global_set`, `is_exception`).
//!
//! They wrap what the VM uses internally, so what is checked here is that they mean the same
//! thing as the Ruby side of each — that `ivar_set` writes the `@name` a script reads, that
//! `global_set` writes the `$name` it reads — and the one shape that made them necessary: the
//! host hangs a value on a task, and a native called from inside that task reads it back
//! without having been told which task it belongs to. That is how rubevy gives each script the
//! entity it drives.

use std::sync::{Arc, Mutex};

use sabiruby::{Value, Vm};

fn compile(src: &str) -> Vec<u8> {
    sabiruby_compiler::compile(src.as_bytes(), &sabiruby_compiler::Options {
        filename: "(test)".into(), debug_info: true, ..Default::default()
    }).expect("compile")
}

fn run(vm: &mut Vm, src: &str) -> String {
    vm.load_and_run(&compile(src)).expect("run");
    String::from_utf8_lossy(&vm.take_output()).into_owned()
}

#[test]
fn an_ivar_the_host_left_is_the_one_ruby_reads() {
    let mut vm = Vm::with_mrblib().expect("vm");
    let obj = match run_value(&mut vm, "Object.new") { Value::Obj(o) => o, v => panic!("{v:?}") };
    // nothing is there before it is put there, and a name nobody ever interned is nil too
    assert!(vm.ivar_get(obj, "@owner").is_nil());
    assert!(vm.ivar_get(obj, "@never_seen_by_anything_else").is_nil());
    vm.ivar_set(obj, "@owner", Value::Int(7));
    assert_eq!(vm.ivar_get(obj, "@owner"), Value::Int(7));
    // the same table Ruby reads and writes
    vm.global_set("$it", Value::Obj(obj));
    assert_eq!(run(&mut vm, "p $it.instance_variable_get(:@owner)\n$it.instance_variable_set(:@owner, 8)\n"), "7\n");
    assert_eq!(vm.ivar_get(obj, "@owner"), Value::Int(8));
    // setting twice replaces rather than adds
    vm.ivar_set(obj, "@owner", Value::Int(9));
    assert_eq!(run(&mut vm, "p $it.instance_variables, $it.instance_variable_get(:@owner)\n"), "[:@owner]\n9\n");
}

#[test]
fn a_global_the_host_set_is_the_one_ruby_reads() {
    let mut vm = Vm::with_mrblib().expect("vm");
    assert!(vm.global_get("$state").is_nil());
    let h = vm.hash_new();
    let k = Value::Sym(vm.intern("frame"));
    vm.hash_set(h, k, Value::Int(3)).expect("hash_set");
    vm.global_set("$state", h);
    assert_eq!(run(&mut vm, "p $state\n$state = 42\n"), "{frame: 3}\n");
    // and the other way: what Ruby set is what the host reads
    assert_eq!(vm.global_get("$state"), Value::Int(42));
    // the value is a root while it sits there (nothing else holds this array)
    vm.global_set("$kept", Value::Nil);
    let ary = vm.ary_new(vec![Value::Int(1), Value::Int(2)]);
    vm.global_set("$kept", ary);
    vm.gc_collect();
    assert_eq!(run(&mut vm, "p $kept\n"), "[1, 2]\n");
}

#[test]
fn a_native_finds_the_task_it_was_called_from() {
    // the rubevy shape: the host hangs an entity on each task, and `Rubevy.entity` — a native
    // with no argument — answers the one belonging to whichever script is running
    let mut vm = Vm::with_mrblib().expect("vm");
    // nothing runs under the scheduler yet
    assert_eq!(vm.task_running(), None);
    let object = vm.core.object;
    vm.define_closure(object, "whose_task", |vm, _s, _a, _b| {
        Ok(match vm.task_running() { Some(t) => vm.ivar_get(t, "@entity"), None => Value::Nil })
    });
    let irep = vm.load(&compile("p whose_task\n")).expect("load");
    for entity in [11i64, 22] {
        let task = vm.task_spawn(irep, 128, Some("script")).expect("spawn");
        vm.ivar_set(task, "@entity", Value::Int(entity));
    }
    // outside a task there is no running one, so the same native answers nil
    assert_eq!(run(&mut vm, "p whose_task\n"), "nil\n");
    // `task_pending`, not a nil answer: a task that ends with a nil result answers nil here
    // just as an empty scheduler does (`Vm::task_run_once`)
    while vm.task_pending() { vm.task_run_once().expect("run_once"); }
    assert_eq!(String::from_utf8_lossy(&vm.take_output()), "11\n22\n");
    assert_eq!(vm.task_running(), None);
}

#[test]
fn a_task_that_raised_is_told_apart_from_one_that_answered() {
    let mut vm = Vm::with_mrblib().expect("vm");
    let ok = vm.load(&compile("1 + 1\n")).expect("load");
    let bad = vm.load(&compile("raise ArgumentError, 'no'\n")).expect("load");
    let a = vm.task_spawn(ok, 128, Some("a")).expect("spawn");
    let b = vm.task_spawn(bad, 128, Some("b")).expect("spawn");
    vm.gc_register(a);
    vm.gc_register(b);
    // `task_pending`, not a nil answer: a task that ends with a nil result answers nil here
    // just as an empty scheduler does (`Vm::task_run_once`)
    while vm.task_pending() { vm.task_run_once().expect("run_once"); }
    assert!(vm.task_finished(a) && vm.task_finished(b));
    assert_eq!(vm.task_value(a), Value::Int(2));
    assert!(!vm.is_exception(vm.task_value(a)));
    assert!(vm.is_exception(vm.task_value(b)));
    let v = vm.task_value(b);
    assert_eq!(vm.inspect_str(v).expect("inspect"), "#<ArgumentError: no>");
}

#[test]
fn is_exception_answers_for_any_value_without_running_ruby() {
    let mut vm = Vm::with_mrblib().expect("vm");
    // an instance of Exception or of anything below it
    for src in ["RuntimeError.new('x')", "StandardError.new", "Exception.new", "Class.new(RuntimeError).new", "(begin; 1/0; rescue => e; e; end)"] {
        let v = run_value(&mut vm, src);
        assert!(vm.is_exception(v), "{src}");
    }
    // and nothing else, not even something that says it is one
    for src in ["nil", "1", "'x'", "Object.new", "RuntimeError", "(o = Object.new; def o.is_a?(k); true; end; o)"] {
        let v = run_value(&mut vm, src);
        assert!(!vm.is_exception(v), "{src}");
    }
    // a redefined `is_a?` is exactly what it does not ask, and it never re-enters the VM: a
    // closure counting calls stays at zero across the whole check
    let calls = Arc::new(Mutex::new(0u32));
    let sink = calls.clone();
    let object = vm.core.object;
    vm.define_closure(object, "counted", move |_vm, _s, _a, _b| { *sink.lock().unwrap() += 1; Ok(Value::True) });
    let v = run_value(&mut vm, "(o = RuntimeError.new('x'); def o.is_a?(k); counted; end; o)");
    assert!(vm.is_exception(v));
    assert_eq!(*calls.lock().unwrap(), 0);
}

fn run_value(vm: &mut Vm, src: &str) -> Value {
    vm.global_set("$__v", Value::Nil);
    let bin = compile(&format!("$__v = ({src})\n"));
    vm.load_and_run(&bin).expect("run");
    // the global keeps it rooted until the next call replaces it
    vm.global_get("$__v")
}

// Item 5 of docs/plans/from-mrubyedge-plan.md: RUBY_ENGINE stays the reference's, and the
// constant a script tests to know it is on SabiRuby is one the reference does not define.
#[test]
fn engine_constants_tell_sabiruby_apart_without_changing_ruby_engine() {
    let mut vm = Vm::with_mrblib().expect("vm");
    let out = run(&mut vm, r#"
        puts RUBY_ENGINE, MRUBY_PLATFORM, defined?(SABIRUBY_VERSION).inspect, SABIRUBY_VERSION
    "#);
    assert_eq!(out, format!("mruby\nrust-sabiruby\n\"constant\"\n{}\n", env!("CARGO_PKG_VERSION")));
}

// Item 8 of docs/plans/leftovers-plan.md: the three entry points rubevy was doing with
// `funcall` — walking a Hash's keys, and reading a `Task::Queue` from outside the VM.

#[test]
fn hash_keys_answers_what_ruby_keys_does_without_a_send() {
    let mut vm = Vm::with_mrblib().expect("vm");
    let h = run_value(&mut vm, r#"{ "b" => 1, a: 2, 3 => [4], nil => nil }"#);
    let keys = vm.hash_keys(h).expect("a Hash");
    // insertion order, exactly the Array `Hash#keys` builds
    let via_ruby = {
        vm.global_set("$h", h);
        let ks = run_value(&mut vm, "$h.keys");
        vm.ary_vals(ks).expect("an Array")
    };
    assert_eq!(keys, via_ruby);
    assert_eq!(keys.len(), 4);
    // and the values are still reachable key by key, which is the shape this is for
    let looked_up: Vec<Value> = keys.iter().map(|k| vm.hash_get(h, *k).unwrap_or(Value::Nil)).collect();
    let entries: Vec<Value> = vm.hash_entries(h).expect("a Hash").into_iter().map(|(_, v)| v).collect();
    assert_eq!(looked_up, entries);
    // the keys of an empty Hash are no keys, and anything that is not a Hash is None
    let empty = run_value(&mut vm, "{}");
    assert_eq!(vm.hash_keys(empty), Some(vec![]));
    assert_eq!(vm.hash_keys(Value::Int(1)), None);
    let ary = run_value(&mut vm, "[1, 2]");
    assert_eq!(vm.hash_keys(ary), None);
    // a Hash with a default block is still just its entries: asking for the keys runs nothing
    let with_default = run_value(&mut vm, "h = Hash.new { |_, k| raise k.to_s }; h[:x] = 1; h");
    assert_eq!(vm.hash_keys(with_default).expect("a Hash").len(), 1);
}

#[test]
fn a_host_reads_a_task_queue_without_sending_size_and_pop() {
    let mut vm = Vm::with_mrblib().expect("vm");
    let q = vm.task_queue_new().expect("a queue");
    vm.gc_register(q);
    assert_eq!(vm.task_queue_len(q).expect("len"), 0);
    assert_eq!(vm.task_queue_try_pop(q).expect("pop"), None);

    for i in 0..3 {
        vm.task_queue_push(q, Value::Int(i)).expect("push");
    }
    assert_eq!(vm.task_queue_len(q).expect("len"), 3);
    // first in, first out, and the length follows
    assert_eq!(vm.task_queue_try_pop(q).expect("pop"), Some(Value::Int(0)));
    assert_eq!(vm.task_queue_len(q).expect("len"), 2);

    // the same queue as the script's: what Ruby pushes the host pops, and the other way
    vm.global_set("$q", Value::Obj(q));
    assert_eq!(run(&mut vm, "$q.push(9); p $q.size"), "3\n");
    assert_eq!(vm.task_queue_try_pop(q).expect("pop"), Some(Value::Int(1)));
    assert_eq!(vm.task_queue_try_pop(q).expect("pop"), Some(Value::Int(2)));
    assert_eq!(vm.task_queue_try_pop(q).expect("pop"), Some(Value::Int(9)));
    assert_eq!(vm.task_queue_try_pop(q).expect("pop"), None);
    assert_eq!(run(&mut vm, "p $q.size, $q.empty?"), "0\ntrue\n");

    // a closed queue answers None rather than raising: the host is not a task and has nothing
    // to park, so there is nothing for `Task::Error` to interrupt
    vm.task_queue_push(q, Value::Int(5)).expect("push");
    run(&mut vm, "$q.close");
    assert_eq!(vm.task_queue_try_pop(q).expect("pop"), Some(Value::Int(5)));
    assert_eq!(vm.task_queue_try_pop(q).expect("pop"), None);
    assert_eq!(vm.task_queue_len(q).expect("len"), 0);

    // and something that is not a queue at all is an error, not a wrong answer
    let not_a_queue = match run_value(&mut vm, "Object.new") { Value::Obj(o) => o, v => panic!("{v:?}") };
    assert!(vm.task_queue_len(not_a_queue).is_err());
    assert!(vm.task_queue_try_pop(not_a_queue).is_err());
}

// `task_context` names the context a task runs in, so a host can pick that task's frames out of
// `Vm::snapshot` — what an in-game inspector shows for one robot.
#[test]
fn task_context_is_the_index_into_the_snapshot() {
    let mut vm = Vm::with_mrblib().expect("vm");
    run(&mut vm, r#"
        $t = Task.new(name: "probe") { def deep; sleep 1; end; deep }
        Task.pass
    "#);
    let task = vm.global_get("$t").obj().expect("task object");
    let i = vm.task_context(task).expect("a started task has a context");
    let snap = vm.snapshot(1);
    let ctx = snap.contexts.iter().find(|c| c.index == i).expect("the context is in the snapshot");
    assert!(ctx.index != snap.cur, "a parked task is not the current context");
    assert!(!vm.task_frames(task).is_empty(), "the parked task keeps its frames");
}
