//! Native methods a host builds at run time: `Vm::define_closure` (a closure that carries an
//! environment, next to `Vm::define_method`'s bare function pointer) and `Vm::set_host_state`
//! (one typed value the host leaves with the VM for its natives to read back).
//!
//! What is checked here is that a `Method::Closure` is treated everywhere a `Method::Native`
//! is: dispatched by SEND and by `funcall`, seen by `respond_to?` / `method_defined?` /
//! `instance_methods`, described the same by `Method#owner` / `#arity` / `#inspect`, aliased,
//! reached through `method_missing`, and that an `Err` it returns raises in Ruby.

use std::sync::{Arc, Mutex};

use sabiruby::{Value, Vm};

fn compile(src: &str) -> Vec<u8> {
    sabiruby_compiler::compile(src.as_bytes(), &sabiruby_compiler::Options {
        filename: "(test)".into(), debug_info: true, ..Default::default()
    }).expect("compile")
}

fn run(vm: &mut Vm, src: &str) -> String {
    let bin = compile(src);
    vm.load_and_run(&bin).expect("run");
    String::from_utf8_lossy(&vm.take_output()).into_owned()
}

/// Runs and gives back the error message instead of panicking.
fn run_err(vm: &mut Vm, src: &str) -> String {
    let bin = compile(src);
    match vm.load_and_run(&bin) {
        Ok(_) => panic!("expected a raise, got none"),
        Err(e) => vm.describe_error(&e),
    }
}

#[test]
fn a_closure_writes_to_the_environment_it_captured() {
    let log: Arc<Mutex<Vec<i64>>> = Arc::new(Mutex::new(Vec::new()));
    let mut vm = Vm::with_mrblib().expect("vm");
    let sink = log.clone();
    let object = vm.core.object;
    vm.define_closure(object, "record", move |_vm, _self_, args, _blk| {
        if let Some(Value::Int(i)) = args.first() { sink.lock().unwrap().push(*i); }
        Ok(Value::Nil)
    });
    assert_eq!(run(&mut vm, "record 1; record 2; [1,2,3].each { |i| record(i * 10) }; p :ok"), ":ok\n");
    assert_eq!(*log.lock().unwrap(), vec![1, 2, 10, 20, 30]);
}

#[test]
fn a_closure_returns_a_value_to_ruby() {
    let mut vm = Vm::with_mrblib().expect("vm");
    let base = 100i64;
    let object = vm.core.object;
    vm.define_closure(object, "plus_base", move |_vm, _self_, args, _blk| {
        match args.first() { Some(Value::Int(i)) => Ok(Value::Int(i + base)), _ => Ok(Value::Nil) }
    });
    assert_eq!(run(&mut vm, "p plus_base(5)"), "105\n");
    // through funcall from native code, not only from bytecode
    let mid = vm.intern("plus_base");
    let top = Value::Obj(vm.top_self);
    let v = vm.funcall(top, mid, &[Value::Int(7)], Value::Nil).expect("funcall");
    assert_eq!(v, Value::Int(107));
}

#[test]
fn an_err_from_a_closure_raises_in_ruby() {
    let mut vm = Vm::with_mrblib().expect("vm");
    let object = vm.core.object;
    vm.define_closure(object, "boom", |vm, _self_, _args, _blk| Err(vm.raise_arg("no good")));
    // rescued on the Ruby side: the closure's Err is an ordinary raise
    assert_eq!(run(&mut vm, "begin; boom; rescue ArgumentError => e; p e.message; end"), "\"no good\"\n");
    // and unrescued it comes back out of `load_and_run`
    assert!(run_err(&mut vm, "boom").contains("no good"));
}

#[test]
fn a_closure_can_call_back_into_the_vm() {
    let mut vm = Vm::with_mrblib().expect("vm");
    let object = vm.core.object;
    // the block the caller passed, run from the closure
    vm.define_closure(object, "twice", |vm, _self_, _args, blk| {
        vm.call_block(blk, &[Value::Int(1)])?;
        vm.call_block(blk, &[Value::Int(2)])
    });
    assert_eq!(run(&mut vm, "twice { |i| puts i }"), "1\n2\n");
}

#[test]
fn a_closure_looks_like_a_native_to_reflection() {
    let mut vm = Vm::with_mrblib().expect("vm");
    let c = vm.define_class("Widget", vm.core.object);
    vm.define_closure(c, "spin", |_vm, _self_, _args, _blk| Ok(Value::Int(1)));
    vm.define_method(c, "plain", |_vm, _self_, _args, _blk| Ok(Value::Int(1)));
    let out = run(&mut vm, r#"
      w = Widget.new
      p w.respond_to?(:spin), w.respond_to?(:plain)
      p Widget.method_defined?(:spin)
      p Widget.instance_methods(false).sort
      m = w.method(:spin)
      p m.owner, m.name, m.arity, m.receiver == w
      p m.call
      p Widget.instance_method(:spin).class
      p m.source_location
      p m.inspect
      p m == w.method(:spin), m == w.method(:plain)
    "#);
    assert_eq!(out, concat!(
        "true\ntrue\n",
        "true\n",
        "[:plain, :spin]\n",
        "Widget\n:spin\n-1\ntrue\n",
        "1\n",
        "UnboundMethod\n",
        "nil\n",
        "\"#<Method: Widget#spin>\"\n",
        "true\nfalse\n",
    ));
}

#[test]
fn a_closure_is_aliased_and_undefined_like_a_native() {
    let mut vm = Vm::with_mrblib().expect("vm");
    let c = vm.define_class("Gadget", vm.core.object);
    vm.define_closure(c, "tick", |_vm, _self_, _args, _blk| Ok(Value::Int(42)));
    let out = run(&mut vm, r#"
      class Gadget
        alias tock tick
      end
      g = Gadget.new
      p g.tock, g.tick
      class Gadget
        undef_method :tick
      end
      p g.respond_to?(:tick), g.respond_to?(:tock)
    "#);
    assert_eq!(out, "42\n42\nfalse\ntrue\n");
}

#[test]
fn a_closure_method_missing_takes_the_call() {
    let mut vm = Vm::with_mrblib().expect("vm");
    let c = vm.define_class("Proxy", vm.core.object);
    vm.define_closure(c, "method_missing", |vm, _self_, args, _blk| {
        let name = match args.first() { Some(Value::Sym(s)) => vm.sym_name(*s), _ => "?".into() };
        Ok(vm.str_from(format!("missing:{name}")))
    });
    // from bytecode (SEND) and from native code (funcall)
    assert_eq!(run(&mut vm, "p Proxy.new.whatever"), "\"missing:whatever\"\n");
    let obj = {
        let bin = compile("$o = Proxy.new\n");
        vm.load_and_run(&bin).expect("run");
        let g = vm.intern("$o");
        vm.globals.get(&g).map(|s| s.get()).expect("$o")
    };
    let mid = vm.intern("anything");
    let v = vm.funcall(obj, mid, &[], Value::Nil).expect("funcall");
    assert_eq!(vm.str_bytes(v).map(|b| String::from_utf8_lossy(b).into_owned()), Some("missing:anything".into()));
}

#[test]
fn a_closure_is_a_singleton_method_too() {
    let mut vm = Vm::with_mrblib().expect("vm");
    let c = vm.define_class("Single", vm.core.object);
    let sc = vm.singleton_class(Value::Obj(c)).expect("singleton");
    vm.define_closure(sc, "build", |_vm, _self_, _args, _blk| Ok(Value::Int(7)));
    assert_eq!(run(&mut vm, "p Single.build; p Single.singleton_methods.sort"), "7\n[:build]\n");
}

#[test]
fn host_state_is_reached_from_a_closure_and_from_a_plain_native() {
    struct Counters { frames: u64 }
    let mut vm = Vm::with_mrblib().expect("vm");
    vm.set_host_state(Counters { frames: 0 });
    let object = vm.core.object;
    vm.define_closure(object, "tick", |vm, _self_, _args, _blk| {
        let n = match vm.host_state_mut::<Counters>() { Some(c) => { c.frames += 1; c.frames } None => 0 };
        Ok(Value::Int(n as i64))
    });
    // a bare fn reaches it the same way: the VM is the way through, not the closure
    vm.define_method(object, "frames", |vm, _self_, _args, _blk| {
        Ok(Value::Int(vm.host_state::<Counters>().map(|c| c.frames).unwrap_or(0) as i64))
    });
    assert_eq!(run(&mut vm, "p tick, tick, tick, frames"), "1\n2\n3\n3\n");
    assert_eq!(vm.host_state::<Counters>().map(|c| c.frames), Some(3));
    // the wrong type answers None, and a replacement takes over
    assert!(vm.host_state::<String>().is_none());
    vm.set_host_state(String::from("hello"));
    assert_eq!(vm.host_state::<String>().map(|s| s.as_str()), Some("hello"));
    assert!(vm.host_state::<Counters>().is_none());
    assert_eq!(run(&mut vm, "p tick"), "0\n");
    let taken = vm.take_host_state();
    assert!(taken.is_some());
    assert!(vm.host_state::<String>().is_none());
}

#[test]
fn a_closure_survives_the_collector() {
    let log: Arc<Mutex<Vec<i64>>> = Arc::new(Mutex::new(Vec::new()));
    let mut vm = Vm::with_mrblib().expect("vm");
    let sink = log.clone();
    let object = vm.core.object;
    vm.define_closure(object, "record", move |_vm, _self_, args, _blk| {
        if let Some(Value::Int(i)) = args.first() { sink.lock().unwrap().push(*i); }
        Ok(Value::Nil)
    });
    vm.set_gc_stress(true);
    assert_eq!(run(&mut vm, "100.times { |i| record(i) }; GC.start; record(-1); p :ok"), ":ok\n");
    assert_eq!(log.lock().unwrap().len(), 101);
}

/// The reason both the closure and the host state are `Send + Sync`: a `Vm` is
/// (`tests/send_sync.rs`), and an engine that keeps one in its world requires it.
#[test]
fn a_vm_with_a_closure_and_host_state_is_still_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>(_: &T) {}
    let mut vm = Vm::with_mrblib().expect("vm");
    let shared = Arc::new(Mutex::new(0i64));
    let s = shared.clone();
    let object = vm.core.object;
    vm.define_closure(object, "bump", move |_vm, _self_, _args, _blk| {
        let mut g = s.lock().unwrap(); *g += 1; Ok(Value::Int(*g))
    });
    vm.set_host_state(Arc::new(Mutex::new(Vec::<u8>::new())));
    assert_send_sync(&vm);
    assert_eq!(run(&mut vm, "p bump, bump"), "1\n2\n");
}

/// `Vm::backtrace_line` and `Vm::next_line` are two different questions, and a native that
/// wants to record where it was called from wants the first one.
///
/// A native pushes no frame of its own, so the innermost `CallInfo` a native sees is the Ruby
/// frame that called it — and its `pc` is already past the SEND (`self.ci[top].pc = pc` runs
/// before the instruction does). `backtrace_line` steps back over that, as `Vm::backtrace`
/// does; `next_line` does not, which is what a debugger stopped at an instruction boundary
/// wants and what the playground's stepper reads.
#[test]
fn a_native_asks_backtrace_line_for_the_line_it_was_called_from() {
    let seen: Arc<Mutex<Vec<(Option<u32>, Option<u32>, Vec<String>)>>> = Arc::new(Mutex::new(Vec::new()));
    let mut vm = Vm::with_mrblib().expect("vm");
    let sink = seen.clone();
    let object = vm.core.object;
    vm.define_closure(object, "note", move |vm, _self_, _args, _blk| {
        sink.lock().unwrap().push((vm.backtrace_line(), vm.next_line(), vm.backtrace(None)));
        Ok(Value::Nil)
    });
    // a call on a line of its own, then one spread over two lines, then a trailing statement
    // so that neither is the last instruction of the program
    run(&mut vm, "note 1\nnote 2,\n     3\nx = 4\n");
    let seen = seen.lock().unwrap();
    let lines: Vec<_> = seen.iter().map(|(b, _, _)| *b).collect();
    // the call is on line 1; the call spread over two lines is located where it ends, which is
    // the line the SEND carries and the line an error raised inside the native would name
    assert_eq!(lines, [Some(1), Some(3)]);
    // the same number the backtrace's first frame carries — it is the same question
    for (line, _, bt) in seen.iter() {
        assert_eq!(bt.first().map(|s| s.as_str()), Some(format!("(test):{}", line.unwrap()).as_str()));
    }
    // and one instruction later is a different answer, which is why there are two of these
    assert_eq!(seen.iter().map(|(_, c, _)| *c).collect::<Vec<_>>(), [Some(2), Some(4)]);
}

#[test]
fn without_debug_information_there_is_no_line_and_no_backtrace() {
    let seen: Arc<Mutex<Vec<(Option<u32>, usize)>>> = Arc::new(Mutex::new(Vec::new()));
    let mut vm = Vm::with_mrblib().expect("vm");
    let sink = seen.clone();
    let object = vm.core.object;
    vm.define_closure(object, "note", move |vm, _self_, _args, _blk| {
        sink.lock().unwrap().push((vm.backtrace_line(), vm.backtrace(None).len()));
        Ok(Value::Nil)
    });
    let bin = sabiruby_compiler::compile(b"note 1\n", &sabiruby_compiler::Options {
        filename: "(test)".into(), debug_info: false, ..Default::default()
    }).expect("compile");
    vm.load_and_run(&bin).expect("run");
    // the frames `backtrace` leaves out are the frames this has no line for
    assert_eq!(*seen.lock().unwrap(), [(None, 0)]);
}

/// `Vm::current_line` is the former name of `Vm::next_line`, kept while its users move over
/// (`docs/plans/serde-declare-lines-plan.md` §4). Two things have to hold while it is here:
/// it answers exactly as the new name does, and calling it warns.
///
/// `#[expect]` is what checks the warning: unlike `#[allow]` it is itself a lint, and the
/// compiler reports an unfulfilled expectation. So this test stops compiling both if the
/// alias stops being deprecated and when the alias is finally removed.
#[expect(deprecated)]
#[test]
fn the_old_name_still_answers_the_same_and_says_it_is_deprecated() {
    let seen: Arc<Mutex<Vec<(Option<u32>, Option<u32>)>>> = Arc::new(Mutex::new(Vec::new()));
    let mut vm = Vm::with_mrblib().expect("vm");
    let sink = seen.clone();
    let object = vm.core.object;
    vm.define_closure(object, "note", move |vm, _self_, _args, _blk| {
        sink.lock().unwrap().push((vm.next_line(), vm.current_line()));
        Ok(Value::Nil)
    });
    run(&mut vm, "note 1\nnote 2,\n     3\nx = 4\n");
    let seen = seen.lock().unwrap();
    // the same answer, whichever name it is asked by
    for (new, old) in seen.iter() { assert_eq!(new, old); }
    // and it is still the next instruction's line, one past the call (`note 1` is line 1)
    assert_eq!(seen.iter().map(|(_, o)| *o).collect::<Vec<_>>(), [Some(2), Some(4)]);
}
