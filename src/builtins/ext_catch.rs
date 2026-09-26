//! mruby-catch (`mrbgems/mruby-catch/src/catch.c`): `Kernel#catch`/`throw`. Its Ruby part
//! (`UncaughtThrowError`) is `src/mrblib/catch.mrb`.
//!
//! The reference's `catch` is a bytecode method (`r2.call(r1)`, `catch_iseq`) that `throw`
//! recognises on the call stack by its proc and its first argument (`find_catcher`); `throw`
//! then raises an `RBreak` aimed at that frame, so `ensure` bodies run and `rescue Exception`
//! does not see it. Here `catch` is a native that leaves its block to a native loop frame
//! (`Vm::push_loop_frame`, kind `LOOP_CATCH`, the tag in R4): called by a SEND the block is an
//! ordinary frame, so a task can wait inside it, and called from native code the same frame
//! runs nested. `throw` finds the innermost such frame whose tag is the same object and returns
//! a `Break` to it, the mechanism `return` from a nested block already uses.

use crate::argc;
use crate::error::{VmError, VmResult};
use crate::object::BreakTag;
use crate::value::Value;
use crate::value::Slot;
use crate::vm::{LoopNext, Vm, LOOP_IREP, LOOP_RESULT};

use super::object::same_object;

fn catch_m(vm: &mut Vm, s: Value, a: &[Value], b: Value) -> VmResult<Value> {
    argc!(vm, a, 0, 1);
    let tag = match a.first() {
        Some(t) => *t,
        None => { let new = vm.intern("new"); vm.funcall(Value::Obj(vm.core.object), new, &[], Value::Nil)? }
    };
    if b.is_nil() {
        // `r2.call(r1)` with no block
        let call = vm.intern("call");
        return Err(vm.no_method_error(call, Value::Nil, "undefined method 'call' for nil"));
    }
    vm.push_loop_frame(LOOP_CATCH, s, b, &[tag])
}

/// The kind of native loop frame `catch` runs its block in (`builtins::array::loop_step`).
pub(crate) const LOOP_CATCH: i64 = 4;

/// One step of `catch`'s frame: call the block with the tag, then answer what it answered.
pub(crate) fn catch_step(vm: &mut Vm, base: usize) -> VmResult<LoopNext> {
    if vm.stack[base + 3].get() == Value::Int(1) { return Ok(LoopNext::Done(vm.stack[base + LOOP_RESULT].get())); }
    vm.stack[base + 3] = Slot::from(Value::Int(1));
    let tag = vm.stack[base + 4].get();
    vm.loop_arg(base, 0, tag);
    Ok(LoopNext::Call(1))
}

fn throw_m(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1, 2);
    let tag = a[0];
    let obj = a.get(1).copied().unwrap_or(Value::Nil);
    // `find_catcher`: the frames of the running context, innermost first
    let target = (0..vm.ci.len()).rev().find(|&i| {
        let ci = &vm.ci[i];
        ci.irep == LOOP_IREP && vm.stack.get(ci.base + 2).map(|r| r.get()) == Some(Value::Int(LOOP_CATCH))
            && vm.stack.get(ci.base + 4).map(|r| same_object(r.get(), tag)).unwrap_or(false)
    });
    match target {
        Some(depth) => {
            let brk = vm.break_new(BreakTag::Break, depth, obj);
            Err(VmError::Break(brk))
        }
        None => {
            let name = vm.intern("UncaughtThrowError");
            let cls = match vm.const_get(vm.core.object, name) { Some(c) => c, None => return Err(vm.raise_arg("uncaught throw")) };
            let new = vm.intern("new");
            let exc = vm.funcall(cls, new, &[tag, obj], Value::Nil)?;
            Err(VmError::Raise(exc))
        }
    }
}

pub fn init(vm: &mut Vm) {
    let k = vm.core.kernel;
    vm.define_methods(k, &[("catch", catch_m), ("throw", throw_m)]);
    // module functions: public on `Kernel` itself, private as instance methods
    let ksc = vm.singleton_class(Value::Obj(k)).expect("Kernel singleton");
    for name in ["catch", "throw"] {
        let n = vm.intern(name);
        if let Some((m, _)) = vm.find_method(k, n) {
            vm.def_method_raw(ksc, n, m);
            let _ = vm.set_visibility(k, n, crate::object::Vis::Private);
        }
    }
}
