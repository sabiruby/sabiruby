//! mruby-binding (`mrbgems/mruby-binding/src/binding.c`): `Kernel#binding` and `Binding`.
//! No Ruby part.
//!
//! A Binding keeps the reference's four instance variables: `proc`, `env`, `recv` and the
//! `pc` the caller stood at (for `source_location`). The Proc is not the caller's own but a
//! **local variable space** wrapped around it: a Proc with an irep of no locals and a closed
//! environment holding only `self`, whose `upper` is the caller's Proc. A name the scope
//! never had (`local_variable_set(:x, 1)`) is merged into that space — the irep's table and
//! the environment grow together — so it is visible through this Binding without touching
//! the frame it came from; `dup` wraps a fresh space over the shared one, which is what
//! makes the copy see the variables set so far and not the ones set afterwards
//! (`mrb_proc_merge_lvar`, `binding_initialize_copy`).

use alloc::{format, string::String, vec, vec::Vec};

use crate::argc;
use crate::error::VmResult;
use crate::object::{EnvData, ObjKind, ProcData, Vis};
use crate::symbol::Sym;
use crate::value::{ObjId, Slot, Value};
use crate::vm::{VmIrep, Vm};

fn binding_class(vm: &mut Vm) -> ObjId {
    let n = vm.intern("Binding");
    match vm.const_get(vm.core.object, n) { Some(Value::Obj(c)) => c, _ => vm.core.object }
}

pub(crate) fn is_binding(vm: &Vm, v: Value) -> bool {
    let (p, e) = (vm.s.bproc, vm.s.benv);
    matches!(v, Value::Obj(o) if !vm.heap.ivar_get(o, p).is_nil() && !vm.heap.ivar_get(o, e).is_nil())
}

pub(crate) fn parts(vm: &mut Vm, v: Value) -> VmResult<(ObjId, ObjId)> {
    if !is_binding(vm, v) { return Err(vm.raise_type("not a binding")); }
    let o = v.obj().unwrap();
    let (p, e) = (vm.heap.ivar_get(o, vm.s.bproc), vm.heap.ivar_get(o, vm.s.benv));
    match (p, e) { (Value::Obj(p), Value::Obj(e)) => Ok((p, e)), _ => Err(vm.raise_type("not a binding")) }
}

/// `binding_wrap_lvspace`: a Proc with no locals of its own over `proc`, and a closed
/// environment holding the `self` of `env`.
fn wrap_lvspace(vm: &mut Vm, proc_: Option<ObjId>, env: Option<ObjId>) -> (ObjId, ObjId) {
    let irep = vm.ireps.len();
    vm.ireps.push(VmIrep {
        nlocals: 1, nregs: 1,
        iseq: vec![crate::opcode::Op::Return as u8, 0],
        catch: Vec::new(), pool: Vec::new(), syms: Vec::new(), reps: Vec::new(),
        lv: Vec::new(), lines: Vec::new(), filename: None,
    });
    let self_ = env.map(|e| {
        let ed = vm.heap.env(e);
        if ed.attached { vm.stack.get(ed.base).map(|s| s.get()).unwrap_or(Value::Nil) } else { ed.values.first().map(|s| s.get()).unwrap_or(Value::Nil) }
    }).unwrap_or(Value::Nil);
    let target_class = env.and_then(|e| vm.heap.env(e).target_class);
    let new_env = vm.heap.alloc(vm.core.object, ObjKind::Env(EnvData {
        ctx: vm.cur, base: 0, len: 1, bidx: 0, attached: false, values: vec![Slot::from(self_)],
        mid: None, target_class, vis: Vis::Public, modfunc: false, vis_break: false, svar: None, svar_fwd: None,
    }));
    // the space's own Proc keeps the environment it wraps, and the Binding takes the new
    // one: a name of the wrapped scope is then one step up from the Binding's environment
    // (`binding_wrap_lvspace`)
    let p = vm.heap.alloc(vm.core.proc_, ObjKind::Proc(ProcData {
        irep, upper: proc_, env, target_class,
        strict: false, scope: false, orphan: false, mid: None,
    }));
    (p, new_env)
}

/// `mrb_binding_new`.
fn new_binding(vm: &mut Vm, proc_: ObjId, recv: Value, env: ObjId, pc: Option<usize>) -> Value {
    let (lvspace, lvenv) = wrap_lvspace(vm, Some(proc_), Some(env));
    let c = binding_class(vm);
    let b = vm.heap.alloc(c, ObjKind::Object);
    let (sp, se, sr, spc) = (vm.s.bproc, vm.s.benv, vm.s.brecv, vm.s.bpc);
    vm.heap.ivar_set(b, sp, Value::Obj(lvspace));
    vm.heap.ivar_set(b, se, Value::Obj(lvenv));
    vm.heap.ivar_set(b, sr, recv);
    if let Some(pc) = pc { vm.heap.ivar_set(b, spc, Value::Int(pc as i64)); }
    Value::Obj(b)
}

/// `binding_local_variable_search`: where the name lives, as (env, index into its values).
pub(crate) fn search(vm: &mut Vm, proc_: ObjId, env: ObjId, name: Sym) -> Option<(ObjId, usize)> {
    let mut p = Some(proc_);
    let mut e = Some(env);
    while let Some(pid) = p {
        let pd = vm.heap.proc_data(pid);
        let (irep, scope, upper, penv) = (pd.irep, pd.scope, pd.upper, pd.env);
        let ir = &vm.ireps[irep];
        for i in 0..ir.nlocals.saturating_sub(1) {
            if ir.lv.get(i).copied().flatten() == Some(name) {
                let ev = e?;
                let len = vm.heap.env(ev).len;
                return if len > i { Some((ev, i + 1)) } else { None };
            }
        }
        if scope { break; }
        e = penv;
        p = upper;
    }
    None
}

/// Reads a slot of an environment, wherever it lives (a frame's registers while attached).
fn env_get(vm: &Vm, env: ObjId, i: usize) -> Value {
    let ed = vm.heap.env(env);
    if ed.attached { vm.stack.get(ed.base + i).map(|s| s.get()).unwrap_or(Value::Nil) }
    else { ed.values.get(i).map(|s| s.get()).unwrap_or(Value::Nil) }
}
fn env_set(vm: &mut Vm, env: ObjId, i: usize, v: Value) {
    let (attached, base) = { let ed = vm.heap.env(env); (ed.attached, ed.base) };
    if attached {
        if let Some(s) = vm.stack.get_mut(base + i) { s.set(v); }
    } else {
        let ed = vm.heap.env_mut(env);
        if ed.values.len() <= i { ed.values.resize(i + 1, Slot::NIL); }
        ed.values[i] = Slot::from(v);
    }
}

/// `mrb_proc_merge_lvar`: a name the scope never had goes into the local variable space,
/// growing its irep's table and its environment by one.
pub(crate) fn merge_lvar(vm: &mut Vm, proc_: ObjId, env: ObjId, name: Sym, v: Value) {
    let irep = vm.heap.proc_data(proc_).irep;
    let ir = &mut vm.ireps[irep];
    let at = ir.nlocals.saturating_sub(1);
    ir.lv.resize(at, None);
    ir.lv.push(Some(name));
    ir.nlocals = at + 2;
    ir.nregs = ir.nregs.max(ir.nlocals);
    let ed = vm.heap.env_mut(env);
    ed.len = at + 2;
    ed.values.resize(at + 1, Slot::NIL);
    ed.values.push(Slot::from(v));
}

fn name_arg(vm: &mut Vm, v: Value) -> VmResult<Sym> {
    let s = match v {
        Value::Sym(s) => s,
        _ => match vm.str_bytes(v).map(|b| b.to_vec()) {
            Some(b) => vm.syms.intern(&b),
            None => { let d = vm.describe_for_error(v); return Err(vm.raise_type(&format!("{d} is not a symbol nor a string"))); }
        },
    };
    // `binding_local_variable_name_check`
    let name = vm.syms.name(s).to_vec();
    let ok = match name.first() {
        None => false,
        Some(c) => (*c == b'_' || c.is_ascii_lowercase() || !c.is_ascii())
            && name[1..].iter().all(|c| *c == b'_' || c.is_ascii_alphanumeric() || !c.is_ascii()),
    };
    if !ok {
        let n = String::from_utf8_lossy(&name).into_owned();
        return Err(vm.raise(vm.core.name_error, &format!("wrong local variable name '{n}' for binding")));
    }
    Ok(s)
}

fn kernel_binding(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 0);
    let (proc_, pc) = match vm.ci.last() { Some(ci) => (ci.proc_, ci.pc), None => return Err(vm.raise(vm.core.runtime_error, "Cannot create Binding object for non-Ruby caller")) };
    let env = vm.caller_env();
    Ok(new_binding(vm, proc_, s, env, Some(pc)))
}

/// mruby-proc-binding's `Proc#binding`: the scope the Proc was made in — its own
/// environment, with the Proc above it as the scope (`mrb_proc_binding`).
fn proc_binding(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 0);
    let p = match s.obj() { Some(o) if matches!(vm.heap.get(o).kind, ObjKind::Proc(_)) => o, _ => return Err(vm.raise_type("not a proc")) };
    let pd = vm.heap.proc_data(p);
    let (env, upper) = (pd.env, pd.upper);
    match (env, upper) {
        (Some(env), Some(upper)) => {
            let recv = env_get(vm, env, 0);
            let b = new_binding(vm, upper, recv, env, None);
            // `Proc#binding` records where the *proc* was written, not where `binding` was called
            // (`mrb_proc_binding` sets the ivar and `binding_source_location` answers from it)
            let loc = super::ext_proc::source_location_of(vm, p);
            if let Some(o) = b.obj() { let k = vm.intern("@source_location"); vm.heap.ivar_set(o, k, loc); }
            Ok(b)
        }
        _ => {
            // no scope to bind to: a Binding with only its own space (`env = NULL`)
            let (lvspace, lvenv) = wrap_lvspace(vm, None, None);
            let c = binding_class(vm);
            let b = vm.heap.alloc(c, ObjKind::Object);
            let (sp, se, sr) = (vm.s.bproc, vm.s.benv, vm.s.brecv);
            vm.heap.ivar_set(b, sp, Value::Obj(lvspace));
            vm.heap.ivar_set(b, se, Value::Obj(lvenv));
            vm.heap.ivar_set(b, sr, Value::Nil);
            Ok(Value::Obj(b))
        }
    }
}

pub fn init(vm: &mut Vm) {
    let bc = vm.define_class("Binding", vm.core.object);
    // `MRB_UNDEF_ALLOCATOR`: neither `Binding.new` nor `Binding.allocate` makes one
    vm.heap.class_mut(bc).instance_kind = Some(crate::object::InstanceKind::NoAlloc);
    let k = vm.core.kernel;
    // `mrb_define_module_function_id(..., MRB_SYM(binding), ...)` (binding.c)
    vm.define_module_function(k, "binding", kernel_binding).expect("Kernel singleton");
    vm.define_method(vm.core.proc_, "binding", proc_binding);
    vm.define_methods(bc, &[
        ("initialize_copy", |vm, s, a, _b| {
            argc!(vm, a, 1);
            let (src_proc, src_env) = parts(vm, a[0])?;
            if let Value::Obj(o) = s { if vm.heap.get(o).frozen { return Err(vm.frozen_error(s)); } }
            let (lvspace, lvenv) = if vm.heap.env(src_env).len < 2 {
                // nothing merged yet: wrap the same scope the source wraps
                let under = vm.heap.proc_data(src_proc).upper;
                let env = vm.heap.proc_data(src_proc).env;
                let env = under.and_then(|u| vm.heap.proc_data(u).env).or(env);
                wrap_lvspace(vm, under, env)
            } else {
                // the copy shares what was merged so far; the source gets a fresh space, so
                // what either sets from now on stays its own
                let pair = wrap_lvspace(vm, Some(src_proc), Some(src_env));
                let (sp, se) = wrap_lvspace(vm, Some(src_proc), Some(src_env));
                if let Value::Obj(o) = a[0] {
                    let (kp, ke) = (vm.s.bproc, vm.s.benv);
                    vm.heap.ivar_set(o, kp, Value::Obj(sp));
                    vm.heap.ivar_set(o, ke, Value::Obj(se));
                }
                pair
            };
            if let Value::Obj(o) = s {
                let (kp, ke, kr) = (vm.s.bproc, vm.s.benv, vm.s.brecv);
                let recv = a[0].obj().map(|x| vm.heap.ivar_get(x, kr)).unwrap_or(Value::Nil);
                vm.heap.ivar_set(o, kp, Value::Obj(lvspace));
                vm.heap.ivar_set(o, ke, Value::Obj(lvenv));
                vm.heap.ivar_set(o, kr, recv);
            }
            Ok(s)
        }),
        ("local_variable_defined?", |vm, s, a, _b| {
            argc!(vm, a, 1);
            let (p, e) = parts(vm, s)?;
            let n = name_arg(vm, a[0])?;
            Ok(Value::bool(search(vm, p, e, n).is_some()))
        }),
        ("local_variable_get", |vm, s, a, _b| {
            argc!(vm, a, 1);
            let (p, e) = parts(vm, s)?;
            let n = name_arg(vm, a[0])?;
            match search(vm, p, e, n) {
                Some((env, i)) => Ok(env_get(vm, env, i)),
                None => { let name = vm.sym_name(n); Err(vm.raise(vm.core.name_error, &format!("local variable '{name}' is not defined"))) }
            }
        }),
        ("local_variable_set", |vm, s, a, _b| {
            argc!(vm, a, 2);
            let (p, e) = parts(vm, s)?;
            let n = name_arg(vm, a[0])?;
            match search(vm, p, e, n) {
                Some((env, i)) => env_set(vm, env, i, a[1]),
                None => merge_lvar(vm, p, e, n, a[1]),
            }
            Ok(a[1])
        }),
        ("local_variables", |vm, s, _a, _b| {
            let (p, _) = parts(vm, s)?;
            let mut out: Vec<Value> = vec![];
            let mut cur = Some(p);
            while let Some(pid) = cur {
                let pd = vm.heap.proc_data(pid);
                let (irep, scope, upper) = (pd.irep, pd.scope, pd.upper);
                let ir = &vm.ireps[irep];
                for i in 0..ir.nlocals.saturating_sub(1) {
                    if let Some(Some(sym)) = ir.lv.get(i) {
                        let n = vm.syms.name(*sym);
                        if n.first().map(|c| *c == b'*' || *c == b'&').unwrap_or(true) { continue; }
                        if !out.iter().any(|v| *v == Value::Sym(*sym)) { out.push(Value::Sym(*sym)); }
                    }
                }
                if scope { break; }
                cur = upper;
            }
            Ok(vm.ary_new(out))
        }),
        ("receiver", |vm, s, _a, _b| { let _ = parts(vm, s)?; Ok(s.obj().map(|o| vm.heap.ivar_get(o, vm.s.brecv)).unwrap_or(Value::Nil)) }),
        ("source_location", |vm, s, _a, _b| {
            // a Binding that was told where it came from answers that (`Proc#binding`)
            let k = vm.intern("@source_location");
            if let Some(o) = s.obj() {
                if vm.heap.get(o).ivars.iter().any(|(n, _)| *n == k) { return Ok(vm.heap.ivar_get(o, k)); }
            }
            let (p, _) = parts(vm, s)?;
            let pc = s.obj().map(|o| vm.heap.ivar_get(o, vm.s.bpc)).unwrap_or(Value::Nil);
            let upper = vm.heap.proc_data(p).upper;
            let (pc, upper) = match (pc, upper) { (Value::Int(pc), Some(u)) => (pc as usize, u), _ => return Ok(Value::Nil) };
            let irep = vm.heap.proc_data(upper).irep;
            let (line, file) = { let ir = &vm.ireps[irep]; (ir.line_of(pc), ir.filename.clone()) };
            match (line, file) {
                (Some(line), Some(file)) => { let f = vm.str_from(file); Ok(vm.ary_new(vec![f, Value::Int(line as i64)])) }
                _ => Ok(Value::Nil),
            }
        }),
    ]);
    // `mrb_define_private_method_id(..., MRB_SYM(initialize_copy), ...)` (binding.c)
    vm.mark_private(bc, &["initialize_copy"]);
}
