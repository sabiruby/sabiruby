//! `Vm::unload`: a program's ireps go back to the VM once nothing can run them
//! (`docs/plans/host-scale-plan.md`, H2). The replacing loop is the shape of a game whose player
//! rewrites a script: every Apply loads a different text and the one it replaced is handed back,
//! so what the VM holds does not grow with the number of Applies. The rest is every holder of a
//! program's code the VM knows of, each checked to keep the program in until it lets go.
//! `SABIRUBY_GC_STRESS=1` runs all of it with a collection at every boundary after an allocation.

use sabiruby::object::IrepId;
use sabiruby::value::ObjId;
use sabiruby::{UnloadError, Value, Vm, VmError};

fn new_vm() -> Vm {
    let mut vm = Vm::with_mrblib().expect("vm");
    vm.set_gc_stress(std::env::var("SABIRUBY_GC_STRESS").map(|v| !v.is_empty() && v != "0").unwrap_or(false));
    vm
}

fn compile(file: &str, src: &str) -> Vec<u8> {
    sabiruby_compiler::compile(src.as_bytes(), &sabiruby_compiler::Options {
        filename: file.into(), debug_info: true, ..Default::default()
    }).expect("compile")
}

fn load(vm: &mut Vm, file: &str, src: &str) -> IrepId {
    let bin = compile(file, src);
    vm.load(&bin).expect("load")
}

fn run(vm: &mut Vm, src: &str) -> Value {
    let bin = compile("(host)", src);
    let irep = vm.load(&bin).expect("load");
    match vm.run_irep(irep) { Ok(v) => v, Err(e) => panic!("{src}: {}", vm.describe_error(&e)) }
}

/// A host line's answer as text. The line is handed back afterwards — not before: `unload` may
/// collect, and a value the host has not registered is safe only until its next call into the VM.
fn inspect(vm: &mut Vm, src: &str) -> String {
    let bin = compile("(host)", src);
    let irep = vm.load(&bin).expect("load");
    let v = match vm.run_irep(irep) { Ok(v) => v, Err(e) => panic!("{src}: {}", vm.describe_error(&e)) };
    let text = match vm.inspect_str(v) { Ok(s) => s, Err(e) => panic!("{src}: inspect: {}", vm.describe_error(&e)) };
    vm.unload(irep).expect("unload the host's line");
    text
}

/// The ireps that hold code (an unloaded one is an empty entry), and the bytes behind them.
fn held(vm: &Vm) -> (usize, usize) {
    let live: Vec<_> = vm.ireps.iter().filter(|ir| !ir.iseq.is_empty()).collect();
    let bytes = live.iter().map(|ir| ir.iseq.len() + ir.syms.len() * 4 + ir.pool.len() * 16 + ir.lines.len() * 8).sum();
    (live.len(), bytes)
}

// ------------------------------------------------------------------ replacing a script

/// A script as a game runs one: a task that waits on the host's queue and does something with
/// each answer. `n` makes each text different, as a player's edits would.
fn script(n: usize) -> String {
    // the same length whatever `n` is, so that every version's ireps are the same size
    format!("loop {{ v = $inbox.pop; $seen << [\"{n:04}\", v] }}\n")
}

fn task_of(vm: &mut Vm, irep: IrepId) -> ObjId {
    let t = vm.task_spawn(irep, 128, Some("script")).expect("spawn");
    vm.gc_register(t);
    t
}

#[test]
fn replacing_a_script_again_and_again_holds_one_program() {
    let mut vm = new_vm();
    run(&mut vm, "$inbox = Task::Queue.new; $seen = []");
    let inbox = match run(&mut vm, "$inbox") { Value::Obj(o) => o, v => panic!("{v:?}") };
    vm.gc_register(inbox);
    let mut prog = load(&mut vm, "script.rb", &script(0));
    let mut task = task_of(&mut vm, prog);
    vm.task_run_budget(1_000_000).expect("run");
    let after_first = held(&vm);
    for n in 1..=30 {
        // the player's edit: the new text is loaded and started, the old task is stopped and
        // dropped, and the old program handed back
        let next = load(&mut vm, "script.rb", &script(n));
        let next_task = task_of(&mut vm, next);
        let terminate = vm.intern("terminate");
        vm.funcall(Value::Obj(task), terminate, &[], Value::Nil).expect("terminate");
        vm.gc_unregister(task);
        vm.unload(prog).expect("the replaced program is not used any more");
        prog = next;
        task = next_task;
        vm.task_queue_push(inbox, Value::Int(2)).expect("push");
        vm.task_run_budget(1_000_000).expect("run");
        assert_eq!(held(&vm), after_first, "after {n} replacements");
    }
    assert_eq!(inspect(&mut vm, "$seen.last"), r#"["0030", 2]"#);
    assert_eq!(inspect(&mut vm, "$seen.size"), "30");
}

// ------------------------------------------------------------------ what keeps a program in

#[test]
fn a_task_standing_in_the_program_keeps_it() {
    let mut vm = new_vm();
    run(&mut vm, "$inbox = Task::Queue.new; $seen = []");
    let prog = load(&mut vm, "a.rb", &script(1));
    let task = task_of(&mut vm, prog);
    vm.task_run_budget(1_000_000).expect("run");
    // parked on the queue, inside the program's block
    assert_eq!(vm.unload(prog), Err(UnloadError::StillInUse));
    // over, but the host still holds the task, whose Proc is the program's top level
    let terminate = vm.intern("terminate");
    vm.funcall(Value::Obj(task), terminate, &[], Value::Nil).expect("terminate");
    assert_eq!(vm.unload(prog), Err(UnloadError::StillInUse));
    vm.gc_unregister(task);
    assert_eq!(vm.unload(prog), Ok(()));
    assert_eq!(vm.unload(prog), Err(UnloadError::NotLoaded));
}

#[test]
fn a_block_a_method_or_a_fiber_of_the_program_keeps_it() {
    let mut vm = new_vm();
    let a = load(&mut vm, "a.rb", "$keep = proc { :from_a }\n");
    vm.run_irep(a).expect("run");
    assert_eq!(vm.unload(a), Err(UnloadError::StillInUse));
    run(&mut vm, "$keep = nil");
    assert_eq!(vm.unload(a), Ok(()));

    let b = load(&mut vm, "b.rb", "def helper_from_b; :b; end\n");
    vm.run_irep(b).expect("run");
    assert_eq!(vm.unload(b), Err(UnloadError::StillInUse));
    assert_eq!(inspect(&mut vm, "helper_from_b"), ":b");
    run(&mut vm, "Object.send(:remove_method, :helper_from_b)");
    assert_eq!(vm.unload(b), Ok(()));

    let c = load(&mut vm, "c.rb", "$f = Fiber.new { Fiber.yield 1; 2 }\n$f.resume\n");
    vm.run_irep(c).expect("run");
    assert_eq!(vm.unload(c), Err(UnloadError::StillInUse));
    // the fiber is suspended inside c's block: its frames name c even with the Proc gone
    run(&mut vm, "$g = $f; $f = nil");
    assert_eq!(vm.unload(c), Err(UnloadError::StillInUse));
    run(&mut vm, "$g = nil");
    assert_eq!(vm.unload(c), Ok(()));
}

#[test]
fn a_program_still_running_under_the_caller_is_not_unloaded() {
    let mut vm = new_vm();
    let slot = std::sync::Arc::new(std::sync::Mutex::new(None::<Result<(), UnloadError>>));
    let seen = slot.clone();
    let object = vm.core.object;
    vm.define_closure(object, "unload_me", move |vm, _s, a, _b| {
        let Value::Int(id) = a[0] else { panic!("an id") };
        *seen.lock().unwrap() = Some(vm.unload(id as usize));
        Ok(Value::Nil)
    });
    let a = load(&mut vm, "a.rb", "unload_me($id)\n:done\n");
    run(&mut vm, &format!("$id = {a}"));
    assert_eq!(vm.run_irep(a).expect("run"), Value::Sym(vm.intern("done")));
    // its own frame is below the host function that asked
    assert_eq!(*slot.lock().unwrap(), Some(Err(UnloadError::StillInUse)));
    assert_eq!(vm.unload(a), Ok(()));
}

// ------------------------------------------------------------------ what is left behind

#[test]
fn an_exception_raised_in_the_program_keeps_its_backtrace() {
    let mut vm = new_vm();
    let a = load(&mut vm, "a.rb", "def boom\n  Integer('x')\nend\nbegin\n  boom\nrescue => e\n  $err = e\nend\n");
    vm.run_irep(a).expect("run");
    // `boom` is a method of a: take it away so that only the exception is left
    run(&mut vm, "Object.send(:remove_method, :boom)");
    let before = inspect(&mut vm, "$err.backtrace");
    assert_eq!(before, r#"["a.rb:2:in Integer", "a.rb:2:in boom", "a.rb:5"]"#);
    assert_eq!(vm.unload(a), Ok(()));
    assert_eq!(inspect(&mut vm, "$err.backtrace"), before);
}

#[test]
fn an_unloaded_program_cannot_be_run_or_its_id_reused() {
    let mut vm = new_vm();
    let a = load(&mut vm, "a.rb", "[1].map { |x| x }\n");
    assert_eq!(vm.unload(a + 1), Err(UnloadError::NotLoaded), "a block of the program is not a program");
    assert_eq!(vm.unload(a), Ok(()));
    let describe = |vm: &mut Vm, e: VmError| vm.describe_error(&e);
    let e = vm.run_irep(a).expect_err("unloaded");
    assert_eq!(describe(&mut vm, e), "the program was unloaded (ArgumentError)");
    let e = vm.task_spawn(a, 128, None).expect_err("unloaded");
    assert_eq!(describe(&mut vm, e), "the program was unloaded (ArgumentError)");
    // the next program gets numbers of its own, so the old id still names nothing
    let b = load(&mut vm, "b.rb", ":b\n");
    assert!(b > a + 1);
    assert!(vm.run_irep(a).is_err());
    assert_eq!(vm.run_irep(b).expect("run"), Value::Sym(vm.intern("b")));
    assert_eq!(vm.unload(12345), Err(UnloadError::NotLoaded));
}
