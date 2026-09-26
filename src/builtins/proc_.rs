//! Proc.

use alloc::{vec};

use crate::error::VmResult;
use crate::object::ObjKind;
use crate::value::Value;
use crate::vm::Vm;

/// `Proc#==`: the same body and the same environment (mruby `mrb_proc_eql`).
fn proc_eq(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    let (o1, o2) = match (s, a.first().copied()) { (Value::Obj(x), Some(Value::Obj(y))) => (x, y), _ => return Ok(Value::False) };
    if o1 == o2 { return Ok(Value::True); }
    if !matches!(vm.heap.get(o2).kind, ObjKind::Proc(_)) { return Ok(Value::False); }
    let (p1, p2) = (vm.heap.proc_data(o1), vm.heap.proc_data(o2));
    Ok(Value::bool(p1.irep == p2.irep && p1.env == p2.env && p1.target_class == p2.target_class))
}

/// `Proc#dup`: a copy that is an orphan (`break` inside it has no home).
fn proc_dup(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let o = s.obj().unwrap();
    let (irep, upper, env, tc, strict, scope) = { let p = vm.heap.proc_data(o); (p.irep, p.upper, p.env, p.target_class, p.strict, p.scope) };
    let cls = vm.heap.get(o).class;
    let np = vm.heap.alloc(cls, ObjKind::Proc(crate::object::ProcData { irep, upper, env, target_class: tc, strict, scope, orphan: true, mid: None }));
    Ok(Value::Obj(np))
}

fn proc_call(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    vm.call_block(s, a)
}

pub fn init(vm: &mut Vm) {
    let c = vm.core;
    // `call` & co. are Ruby-level methods whose body is `OP_CALL` (mruby `call_proc`),
    // so a block call is a frame replacement, not a VM re-entry. `proc_call`
    // (native, re-entrant) stays available for `Vm::call_block` from Rust.
    let cp = vm.call_proc;
    for name in ["call", "()", "[]", "yield", "==="] {
        let n = vm.intern(name);
        vm.heap.class_mut(c.proc_).methods.insert(n, crate::object::Method::Ruby(cp));
    }
    let _ = proc_call;
    vm.define_methods(c.proc_, &[
        ("to_proc", |_vm, s, _a, _b| Ok(s)),
        ("lambda?", |vm, s, _a, _b| Ok(Value::bool(s.obj().map(|o| vm.heap.proc_data(o).strict).unwrap_or(false)))),
        ("arity", |vm, s, _a, _b| { let o = s.obj().unwrap(); let pd = vm.heap.proc_data(o); let irep = &vm.ireps[pd.irep]; Ok(Value::Int(arity_of(irep, pd.strict))) }),
        ("initialize", |_vm, s, _a, _b| Ok(s)),
        ("==", proc_eq),
        ("eql?", proc_eq),
        ("hash", |vm, s, _a, _b| { let p = vm.heap.proc_data(s.obj().unwrap()); Ok(Value::Int((p.irep as i64) * 31 + p.env.map(|e| e.0 as i64).unwrap_or(0))) }),
        ("dup", proc_dup),
        ("clone", proc_dup),
        ("parameters", |vm, _s, _a, _b| Ok(vm.ary_new(vec![]))),
        ("inspect", |vm, s, _a, _b| { let t = super::object::any_to_s(vm, s); Ok(vm.str_from(t)) }),
        ("to_s", |vm, s, _a, _b| { let t = super::object::any_to_s(vm, s); Ok(vm.str_from(t)) }),
    ]);
    let sc = vm.singleton_class(Value::Obj(c.proc_)).unwrap();
    vm.define_method(sc, "new", |vm, s, _a, b| match b {
        Value::Obj(o) if matches!(vm.heap.get(o).kind, ObjKind::Proc(_)) => {
            // copy the block into a fresh Proc of the requested class (mruby `mrb_proc_s_new`)
            let (irep, upper, env, tc, strict, scope) = { let p = vm.heap.proc_data(o); (p.irep, p.upper, p.env, p.target_class, p.strict, p.scope) };
            let cls = s.obj().unwrap();
            let np = vm.heap.alloc(cls, ObjKind::Proc(crate::object::ProcData { irep, upper, env, target_class: tc, strict, scope, orphan: false, mid: None }));
            let init = vm.s.initialize;
            if vm.find_method(cls, init).map(|(_, owner)| owner != vm.core.proc_).unwrap_or(false) { vm.funcall(Value::Obj(np), init, &[], b)?; }
            // a block created by the calling frame is an orphan once wrapped: `break` has no home
            let caller_env = vm.ci.last().and_then(|c| c.env);
            if !strict && env.is_some() && env == caller_env { if let ObjKind::Proc(pd) = &mut vm.heap.get_mut(np).kind { pd.orphan = true; } }
            Ok(Value::Obj(np))
        }
        // `mrb_proc_s_new`: `mrb_get_args(mrb, "&!", ...)`
        _ => Err(vm.raise_arg("no block given")),
    });
}

/// Decodes the leading `OP_ENTER` to compute `Proc#arity` (mruby `mrb_proc_arity`).
pub(crate) fn arity_of(irep: &crate::vm::VmIrep, lambda: bool) -> i64 {
    if irep.iseq.first() != Some(&(crate::opcode::Op::Enter as u8)) { return 0; }
    let a = ((irep.iseq[1] as u32) << 16) | ((irep.iseq[2] as u32) << 8) | irep.iseq[3] as u32;
    let m1 = ((a >> 18) & 0x1f) as i64;
    let o = ((a >> 13) & 0x1f) as i64;
    let r = ((a >> 12) & 1) as i64;
    let m2 = ((a >> 7) & 0x1f) as i64;
    let k = ((a >> 2) & 0x1f) as i64;
    let kd = ((a >> 1) & 1) as i64;
    let _ = (k, kd);
    let req = m1 + m2;
    if r == 1 || (lambda && o > 0) { -(req + 1) } else { req }
}
