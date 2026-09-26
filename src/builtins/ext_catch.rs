//! mruby-catch (`mrbgems/mruby-catch/src/catch.c`): `Kernel#catch`/`throw`. Its Ruby part
//! (`UncaughtThrowError`) is `src/mrblib/catch.mrb`.
//!
//! `catch` is the reference's own bytecode method, `def catch(tag = Object.new, &b) = b.call(tag)`
//! (`catch_iseq`), so its block runs in an ordinary frame: no native boundary, and a task can
//! wait inside it. `throw` finds the innermost frame of that method whose tag (its R1) is the
//! same object (`find_catcher`) and returns a `Break` to that frame, the mechanism `return` from
//! a nested block already uses, so `ensure` bodies run and `rescue Exception` does not see it.
//! Called from native code the method runs nested, as any Ruby method does, and `throw` finds it
//! the same way.

use alloc::vec;

use crate::argc;
use crate::error::{VmError, VmResult};
use crate::object::BreakTag;
use crate::value::Value;
use crate::vm::Vm;

use super::object::same_object;

fn throw_m(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1, 2);
    let tag = a[0];
    let obj = a.get(1).copied().unwrap_or(Value::Nil);
    // `find_catcher`: frames of the running context, innermost first, the entry frame left out
    let target = vm.catch_proc.and_then(|cp| (1..vm.ci.len()).rev().find(|&i| {
        let ci = &vm.ci[i];
        ci.proc_ == cp && vm.stack.get(ci.base + 1).map(|r| same_object(r.get(), tag)).unwrap_or(false)
    }));
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

/// `catch_iseq` (the reference's, byte for byte):
/// ```text
/// 000 ENTER    0:1:0:0:0:0:1   # def catch(r1 = Object.new, &r2)
/// 004 JMP      013             #   no argument: make the tag
/// 007 MOVE     R3 R1           #   one argument: that is the tag
/// 010 JMP      023
/// 013 GETCONST R3 Object
/// 016 SEND     R3 :new n=0
/// 020 MOVE     R1 R3
/// 023 SEND     R2 :call n=1    #   r2.call(r1)
/// 027 RETURN   R2
/// ```
fn catch_irep(vm: &mut Vm) -> crate::vm::VmIrep {
    use crate::opcode::Op;
    let iseq = vec![
        Op::Enter as u8, 0x00, 0x20, 0x01,
        Op::Jmp as u8, 0x00, 0x06,
        Op::Move as u8, 0x03, 0x01,
        Op::Jmp as u8, 0x00, 0x0a,
        Op::Getconst as u8, 0x03, 0x00,
        Op::Send as u8, 0x03, 0x01, 0x00,
        Op::Move as u8, 0x01, 0x03,
        Op::Send as u8, 0x02, 0x02, 0x01,
        Op::Return as u8, 0x02,
    ];
    let syms = vec![vm.intern("Object"), vm.intern("new"), vm.intern("call")];
    crate::vm::VmIrep { nlocals: 3, nregs: 5, iseq, catch: vec![], pool: vec![], syms, reps: vec![], lv: vec![], lines: vec![], filename: None }
}

pub fn init(vm: &mut Vm) {
    let k = vm.core.kernel;
    let irep = catch_irep(vm);
    vm.ireps.push(irep);
    let id = vm.ireps.len() - 1;
    let p = vm.heap.alloc(vm.core.proc_, crate::object::ObjKind::Proc(crate::object::ProcData {
        irep: id, upper: None, env: None, target_class: None, strict: true, scope: true, orphan: false, mid: None,
    }));
    vm.catch_proc = Some(p);
    let catch = vm.intern("catch");
    vm.def_method_raw(k, catch, crate::object::Method::Ruby(p));
    vm.define_methods(k, &[("throw", throw_m)]);
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
