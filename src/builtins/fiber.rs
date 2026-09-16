//! `Fiber` (mruby-fiber gem, `mrbgems/mruby-fiber/src/fiber.c`). The context
//! switching itself lives in `Vm` (`fiber_switch`, `fiber_yield`, ...); these
//! are the Ruby-visible methods.

use alloc::{format, vec::Vec};

use crate::error::VmResult;
use crate::object::ObjKind;
use crate::value::Value;
use crate::vm::{FiberState, Vm};

pub fn init(vm: &mut Vm) {
    let fiber = vm.core.fiber;
    vm.define_methods(fiber, &[
        ("initialize", |vm, s, a, b| {
            if !a.is_empty() { return Err(vm.argnum_error(a.len(), "0")); }
            let p = match b { Value::Obj(o) if matches!(vm.heap.get(o).kind, ObjKind::Proc(_)) => o, _ => return Err(vm.raise_arg("no block given")) };
            vm.fiber_init(s, p)?;
            Ok(s)
        }),
        ("resume", |vm, s, a, _b| {
            // called from bytecode: switch inside the running loop; from native code: nested loop
            let vmexec = !vm.direct_send;
            vm.fiber_switch(s, a, true, vmexec)
        }),
        ("transfer", |vm, s, a, _b| vm.fiber_transfer(s, a)),
        ("alive?", |vm, s, _a, _b| Ok(Value::bool(vm.fiber_state(s)? != FiberState::Terminated))),
        ("==", |_vm, s, a, _b| { if a.len() != 1 { return Ok(Value::False); } Ok(Value::bool(matches!((s, a[0]), (Value::Obj(x), Value::Obj(y)) if x == y))) }),
        ("to_s", fiber_to_s),
        ("inspect", fiber_to_s),
    ]);
    // `mrb_define_method_id(..., MRB_SYM(initialize), fiber_init, ...)` (mruby-fiber
    // fiber.c): no flag of its own, but `mrb_define_method_raw` makes every `initialize`
    // private, and that path is the one a C definition outside a ROM table takes
    vm.mark_private(fiber, &["initialize"]);
    let sc = vm.singleton_class(Value::Obj(fiber)).expect("Fiber singleton");
    vm.define_methods(sc, &[
        ("yield", |vm, _s, a, _b| vm.fiber_yield(a)),
        ("current", |vm, _s, _a, _b| Ok(vm.fiber_current())),
    ]);
}

/// `#<Fiber:0x... (status)>`; the `file:line` part needs debug info, which is not read.
fn fiber_to_s(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let st = match vm.fiber_state(s)? {
        FiberState::Created => "created",
        FiberState::Running => "resumed",
        FiberState::Resumed => "suspended by resuming",
        FiberState::Suspended | FiberState::Transferred => "suspended",
        FiberState::Terminated => "terminated",
    };
    let base = super::object::any_to_s(vm, s);
    let body: Vec<u8> = base.as_bytes()[..base.len() - 1].to_vec();
    let text = format!("{} ({st})>", alloc::string::String::from_utf8_lossy(&body));
    Ok(vm.str_from(text))
}
