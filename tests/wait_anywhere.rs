//! Waiting inside a block: every path of `tests/wait/probe.rb` × every way a task waits, run in
//! a task beside another one that marks the time, and what each pair must do — wait (the
//! other task's marks fall between the start and the end of the wait) or raise the error that
//! names the native in the way (`docs/design/wait-anywhere.md`). The judgment is the one the
//! reference was measured with (mruby 4.1.0-rc2, 36 paths; the book's
//! `docs/notes/mruby-task.md`, "ブロックの内側で待つ").
//!
//! The sixth way, `host`, is what an embedding host does with a queue of its own (rubevy's
//! `Rubevy.ask(...).pop`): `ask` is a host function that hands out an empty `Task::Queue`, and
//! the host, which drives the scheduler, answers on it between two runs.

use sabiruby::{Value, Vm};

const PROBE: &str = include_str!("wait/probe.rb");

/// The ways a task waits, and the name the error gives each.
const KINDS: &[(&str, &str)] = &[
    ("pop", "Task::Queue#pop"),
    ("sleep", "sleep"),
    ("sleep0", "sleep"),
    ("pass", "Task.pass"),
    ("usleep", "usleep"),
    ("sleep_ms", "sleep_ms"),
    ("host", "Task::Queue#pop"),
];

/// The paths that are still a native boundary, and how the error names them. Every other path
/// of the probe waits.
const BOUNDARIES: &[(&str, &str)] = &[
    // the reference keeps this one too: the collector is walking the heap
    ("c_each_object", "ObjectSpace.each_object's call to a block"),
    // `sort! { }` keeps its nested loop: the frame cost a light block 8% (the author's choice,
    // 2026-09-26; `docs/design/wait-anywhere.md`)
    ("c_sort", "Array#sort!'s call to a block"),
    // mruby-regexp's loops over the matches (left as they are: `docs/design/wait-anywhere.md`)
    ("c_sub", "String#sub's call to a block"),
    ("c_gsub", "String#gsub's call to a block"),
    ("c_scan", "String#scan's call to a block"),
    // callbacks a native makes on its own (the plan's stage 3)
    ("cb_to_s", "Array#join's call to #to_s"),
    ("cb_eq", "Array#include?'s call to #=="),
];

/// Paths that need `Regexp`.
const REGEXP_PATHS: &[&str] = &["c_regexp_match", "c_sub", "c_gsub", "c_scan"];

fn compile(src: &str) -> Vec<u8> {
    sabiruby_compiler::compile(src.as_bytes(), &sabiruby_compiler::Options {
        filename: "probe.rb".into(), debug_info: true, ..Default::default()
    }).expect("compile")
}

/// A VM with the compiler as its host, for the `eval` paths.
fn new_vm() -> Vm {
    let mut vm = Vm::with_mrblib().expect("vm");
    vm.set_host(Box::new(sabiruby_compiler::Compiler::new()));
    vm
}

fn run_program(vm: &mut Vm, src: &str) -> String {
    let bin = compile(src);
    vm.load_and_run(&bin).expect("run");
    String::from_utf8_lossy(&vm.take_output()).into_owned()
}

/// The empty queues `ask` has handed out and the host has not answered yet.
struct Asks(Vec<sabiruby::value::ObjId>);

/// One pair: the probe's own report line.
fn probe(path: &str, kind: &str) -> String {
    let mut vm = new_vm();
    let src = format!("ARGV = [{path:?}, {kind:?}]\n{PROBE}");
    if kind != "host" {
        return run_program(&mut vm, &src);
    }
    vm.set_host_state(Asks(Vec::new()));
    let object = vm.core.object;
    vm.define_closure(object, "ask", |vm, _s, _a, _b| {
        let q = vm.task_queue_new()?;
        vm.host_state_mut::<Asks>().expect("asks").0.push(q);
        Ok(Value::Obj(q))
    });
    run_program(&mut vm, &src);
    let answer = Value::Sym(vm.intern("answer"));
    // turns of the host's loop; the probe needs two (park, then answered)
    for _ in 0..16 {
        vm.task_run_budget(100_000).expect("turn");
        let asks = core::mem::take(&mut vm.host_state_mut::<Asks>().expect("asks").0);
        for q in asks { vm.task_queue_push(q, answer).expect("answer"); }
        if !vm.task_pending() { break; }
    }
    run_program(&mut vm, "$report.call\n")
}

fn expected(path: &str, kind: &str, what: &str) -> String {
    match BOUNDARIES.iter().find(|(p, _)| *p == path) {
        Some((_, at)) => format!("{path} {kind}: raised RuntimeError: can't wait inside {at} ({what})\n"),
        None if path == "ensure_raise" => format!("{path} {kind}: waited, then raised x\n"),
        None => format!("{path} {kind}: waited\n"),
    }
}

#[test]
fn every_path_waits_or_names_its_boundary() {
    let t = std::thread::Builder::new().stack_size(64 << 20).spawn(|| {
        let mut vm = new_vm();
        let paths = run_program(&mut vm, &format!("ARGV = [\"list\"]\n{PROBE}"));
        let mut failures = Vec::new();
        let mut n = 0;
        for path in paths.split_whitespace() {
            if !cfg!(feature = "regexp") && REGEXP_PATHS.contains(&path) { continue; }
            for (kind, what) in KINDS {
                let got = probe(path, kind);
                let want = expected(path, kind, what);
                if got != want { failures.push(format!("  got  {}  want {}", got.trim_end(), want.trim_end())); }
                n += 1;
            }
        }
        assert!(n > 300, "the probe lists its paths ({n} pairs)");
        assert!(failures.is_empty(), "{} of {n} pairs:\n{}", failures.len(), failures.join("\n"));
    }).expect("spawn");
    t.join().expect("probe thread");
}

#[test]
fn the_frame_a_wait_leaves_answers_where_the_call_was() {
    // `instance_exec`, `Method#call` and `Class#new` no longer run their block in a loop of
    // their own; what they answer must still land in the caller's register, and a `break` out
    // of the block still ends the call that took it
    let mut vm = new_vm();
    let out = run_program(&mut vm, r#"
      class P; def initialize(a, k: 2); @v = [a, k]; end; attr_reader :v; end
      def m(x, y: 0) = [x, y, block_given?]
      q = Task::Queue.new
      log = []
      t = Task.new do
        log << Object.new.instance_exec(1, 2) { |a, b| q.pop; a + b }
        log << method(:m).call(3, y: 4) { }
        log << P.new(5, k: 6).v
        log << [1].instance_exec { break :broke }
        log << eval("q.pop; 7")
        log << P.new(8).public_send(:v)
      end
      Task.new { 3.times { q.push(:x); Task.pass } }
      Task.run
      p log, t.status
    "#);
    assert_eq!(out, "[3, [3, 4, true], [5, 6], :broke, 7, [8, 2]]\n:DORMANT\n");
}

#[test]
fn the_loops_handed_to_a_frame_do_what_the_natives_did() {
    // `index { }`, `rindex { }` and `Array.new(n) { }` run their loop in a native loop frame when a
    // SEND called them (`Vm::push_loop_frame`); the answers are the natives', and a `break` now ends
    // the call that took the block. `sort { }` keeps its nested loop and is checked here as it was.
    let mut vm = new_vm();
    let out = run_program(&mut vm, r#"
      a = [1, 2, 3, 2]
      p [a.index { |x| x == 2 }, a.rindex { |x| x == 2 }, a.index { false }]
      b = [1, 2, 3, 4]
      p b.rindex { |x| b.pop if x == 4; x == 1 }
      p [Array.new(3) { |i| i * i }, Array.new(0) { raise "no" }]
      n = 0
      p [5, 4, 3, 2, 1, 0].sort { |x, y| n += 1; n % 3 - 1 }
      p [[1, :a], [0, :b], [1, :c], [0, :d]].sort { |x, y| x[0] <=> y[0] }
      p [3, 1, 2].sort { |x, y| (x <=> y).to_f }
      begin; [3, 1].sort { nil }; rescue => e; p e.message; end
      p [[1, 2, 3].index { break :b }, Array.new(3) { |i| break :early if i == 1; i }, [2, 1].sort { break :s }]
      p [(Class.new { break :c }), catch(:t) { [1, 2].each { |x| throw :t, x if x == 2 } }]
      h = Hash.new { |hh, k| hh[k] = k * 2 }
      p [h[3], h[0], h.default(5), [].delete(1) { :none }]
      p catch { |t| throw t, 7 }
      class F; def initialize; begin; yield; ensure; @e = 1; end; end; end
      class G; def initialize; return 5; end; end
      p [F.new { break 9 }, (Class.new { begin; break 1; ensure; end }), G.new.class, Class.new { next 3 }.class]
      p [[].index { true }, [].sort { raise "never" }, catch(:o) { [3, 1, 2].sort { |a, b| throw :o, :thrown } }]
    "#);
    assert_eq!(out, concat!(
        "[1, 3, nil]\n",
        "0\n",
        "[[0, 1, 4], []]\n",
        // what the native's merge sort gave for the same block (`3170b63`)
        "[0, 5, 4, 1, 3, 2]\n",
        "[[0, :b], [0, :d], [1, :a], [1, :c]]\n",
        "[1, 2, 3]\n",
        "\"comparison of Integer with Integer failed\"\n",
        "[:b, :early, :s]\n",
        "[:c, 2]\n",
        "[6, 0, 10, :none]\n",
        "7\n",
        // `initialize` and the block of `Class.new` answer the receiver, unless a `break` ends
        // them, also on its way through an `ensure` (`Cci::KeepSelf`)
        "[9, 1, G, Class]\n",
        "[nil, [], :thrown]\n",
    ));
}

#[cfg(feature = "regexp")]
#[test]
fn a_match_block_answers_what_it_did() {
    let mut vm = new_vm();
    let out = run_program(&mut vm, r#"p [(/b/.match("abc") { |m| m[0] * 2 }), "abc".match(/c/) { |m| m.pre_match }]"#);
    assert_eq!(out, "[\"bb\", \"ab\"]\n");
}
