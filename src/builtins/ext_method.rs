//! mruby-method (`mrbgems/mruby-method/src/method.c`): `Method` and
//! `UnboundMethod` as plain objects with the reference's five instance
//! variables (`_owner`, `_recv`, `_name`, `_proc`, `_klass`). A native method
//! has no Proc; it is found again by owner and name when called.

use alloc::{format, vec, vec::Vec};

use crate::error::VmResult;
use crate::object::{Method, ObjKind};
use crate::symbol::Sym;
use crate::value::{ObjId, Value};
use crate::vm::Vm;

use super::object::sym_arg;

struct Ivs { owner: Sym, recv: Sym, name: Sym, proc_: Sym, klass: Sym, missing: Sym }
fn ivs(vm: &mut Vm) -> Ivs { Ivs { owner: vm.intern("_owner"), recv: vm.intern("_recv"), name: vm.intern("_name"), proc_: vm.intern("_proc"), klass: vm.intern("_klass"), missing: vm.intern("_missing") } }

fn iv(vm: &Vm, s: Value, k: Sym) -> Value { s.obj().map(|o| vm.heap.ivar_get(o, k)).unwrap_or(Value::Nil) }

fn class_id(vm: &mut Vm, name: &str) -> ObjId { let s = vm.intern(name); match vm.const_get(vm.core.object, s) { Some(Value::Obj(c)) => c, _ => vm.core.object } }

/// `mrb_class_real`: skip singleton and include classes.
fn class_real(vm: &Vm, mut c: ObjId) -> ObjId {
    loop { let cd = vm.heap.class(c); if cd.is_singleton || cd.iclass_of.is_some() || cd.origin_of.is_some() { match cd.superclass { Some(s) => c = s, None => return c } } else { return c; } }
}
/// The module or class that owns the table of `c`.
fn table_class(vm: &Vm, c: ObjId) -> ObjId {
    let cd = vm.heap.class(c);
    if let Some(m) = cd.iclass_of { return m; }
    if let Some(cls) = cd.origin_of { return cls; }
    c
}

fn bind_check(vm: &mut Vm, recv: Value, owner: ObjId) -> VmResult<()> {
    let od = vm.heap.class(owner);
    if !od.is_module && Some(owner) != Some(vm.real_class_of(recv)) && !vm.obj_is_kind_of(recv, owner) {
        if vm.heap.class(owner).is_singleton { return Err(vm.raise_type("singleton method called for a different object")); }
        let d = vm.inspect_str(Value::Obj(owner))?;
        return Err(vm.raise_type(&format!("bind argument must be an instance of {d}")));
    }
    Ok(())
}

fn new_method_obj(vm: &mut Vm, klass_name: &str, owner: ObjId, recv: Value, name: Sym, proc_: Value, klass: ObjId, missing: bool) -> Value {
    let cls = class_id(vm, klass_name);
    let o = vm.heap.alloc(cls, ObjKind::Object);
    let i = ivs(vm);
    vm.heap.ivar_set(o, i.owner, Value::Obj(owner));
    vm.heap.ivar_set(o, i.recv, recv);
    vm.heap.ivar_set(o, i.name, Value::Sym(name));
    vm.heap.ivar_set(o, i.proc_, proc_);
    vm.heap.ivar_set(o, i.klass, Value::Obj(klass));
    vm.heap.ivar_set(o, i.missing, Value::bool(missing));
    Value::Obj(o)
}

/// `method_search_vm`: the method and the class whose table holds it.
fn search(vm: &Vm, c: ObjId, mid: Sym) -> Option<(Method, ObjId)> {
    match vm.find_method(c, mid) { Some((Method::Undef, _)) | None => None, Some(x) => Some(x) }
}

fn method_alloc(vm: &mut Vm, c: ObjId, obj: Value, name: Sym, unbound: bool, singleton: bool) -> VmResult<Value> {
    let found = search(vm, c, name);
    let (owner, proc_, missing) = match found {
        Some((m, owner)) => (owner, match m { Method::Ruby(p) => Value::Obj(p), _ => Value::Nil }, false),
        None => {
            let n = vm.sym_name(name);
            if unbound { let cn = vm.class_name(c); return Err(vm.name_error(name, &format!("undefined method '{n}' for class '{cn}'"))); }
            let rtm = vm.intern("respond_to_missing?");
            let ok = vm.respond_to(obj, rtm) && vm.funcall(obj, rtm, &[Value::Sym(name), Value::True], Value::Nil)?.truthy();
            if !ok {
                if singleton { let d = vm.inspect_str(obj)?; return Err(vm.name_error(name, &format!("undefined singleton method '{n}' for '{d}'"))); }
                let cn = vm.class_name(c);
                return Err(vm.name_error(name, &format!("undefined method '{n}' for class '{cn}'")));
            }
            (c, Value::Nil, true)
        }
    };
    if singleton && !(vm.heap.class(owner).is_singleton || vm.heap.class(owner).iclass_of.is_some()) {
        let n = vm.sym_name(name); let d = vm.inspect_str(obj)?;
        return Err(vm.name_error(name, &format!("undefined singleton method '{n}' for '{d}'")));
    }
    let owner = table_class(vm, owner);
    Ok(new_method_obj(vm, if unbound { "UnboundMethod" } else { "Method" }, owner, if unbound { Value::Nil } else { obj }, name, proc_, c, missing))
}

/// `mcall`: run the method on `recv` (the bound receiver when `recv` is None).
fn mcall(vm: &mut Vm, s: Value, recv: Option<Value>, a: &[Value], b: Value) -> VmResult<Value> {
    let i = ivs(vm);
    let owner = match iv(vm, s, i.owner) { Value::Obj(o) => o, _ => return Err(vm.raise_type("not class/module as owner of method object")) };
    let mid = match iv(vm, s, i.name) { Value::Sym(m) => m, _ => return Err(vm.raise_type("not a symbol")) };
    let recv = match recv { Some(r) => { bind_check(vm, r, owner)?; r } None => iv(vm, s, i.recv) };
    // the keyword Hash the native `call` received travels on to the method
    let kw = match (vm.pending_kw, a.last()) { (Some(k), Some(l)) if !k.is_nil() && k == *l => Some(k), _ => None };
    let pos: &[Value] = if kw.is_some() { &a[..a.len() - 1] } else { a };
    if iv(vm, s, i.missing).truthy() {
        let mm = vm.intern("method_missing");
        let mut args = vec![Value::Sym(mid)];
        args.extend_from_slice(a);
        return vm.send_in_frame(recv, mm, &args, kw, b);
    }
    match iv(vm, s, i.proc_) {
        // called by a SEND the method's body becomes the frame (`mcall` → `mrb_exec_irep`)
        Value::Obj(p) => vm.exec_method_proc(p, recv, pos, kw, b, Some(mid), owner),
        _ => match search(vm, owner, mid) {
            Some((Method::Native(f), _)) => vm.call_native(f, recv, a, b),
            Some((Method::Closure(f), _)) => vm.call_closure(&f, recv, a, b),
            Some((Method::AttrReader(ivn), _)) => Ok(recv.obj().map(|o| vm.heap.ivar_get(o, ivn)).unwrap_or(Value::Nil)),
            Some((Method::AttrWriter(ivn), _)) => { if a.len() != 1 { return Err(vm.argnum_error(a.len(), "1")); } if let Some(o) = recv.obj() { vm.heap.ivar_set(o, ivn, a[0]); } Ok(a[0]) }
            _ => vm.send_in_frame(recv, mid, a, kw, b),
        },
    }
}

/// Arity of a native by the function it is (so an alias of `upcase` answers as `upcase`).
/// A closure has no address to look up: it carries its own arity, which `Vm::define_fn` fills
/// in from the Rust signature and `Vm::define_closure` leaves at `-1`.
fn native_arity(vm: &mut Vm, owner: ObjId, name: Sym) -> i64 {
    match search(vm, owner, name) {
        Some((Method::Native(f), _)) => {
            for (g, a) in &vm.native_arity { if core::ptr::fn_addr_eq(f, *g) { return *a; } }
            -1
        }
        Some((Method::Closure(f), _)) => f.arity,
        _ => -1,
    }
}

fn method_arity(vm: &mut Vm, s: Value) -> i64 {
    let i = ivs(vm);
    if iv(vm, s, i.missing).truthy() { return -1; }
    match iv(vm, s, i.proc_) {
        Value::Obj(p) => { let (irep, strict) = { let pd = vm.heap.proc_data(p); (pd.irep, pd.strict) }; super::proc_::arity_of(&vm.ireps[irep], strict) }
        _ => { let owner = match iv(vm, s, i.owner) { Value::Obj(o) => o, _ => return -1 }; let name = match iv(vm, s, i.name) { Value::Sym(n) => n, _ => return -1 }; native_arity(vm, owner, name) }
    }
}

fn method_to_s(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let i = ivs(vm);
    let cn = { let c = vm.real_class_of(s); vm.class_name(c) };
    let owner = match iv(vm, s, i.owner) { Value::Obj(o) => o, _ => vm.core.object };
    let klass = match iv(vm, s, i.klass) { Value::Obj(o) => o, _ => owner };
    let name = match iv(vm, s, i.name) { Value::Sym(n) => vm.sym_name(n), _ => "?".into() };
    let mut out = format!("#<{cn}: ");
    let recv = iv(vm, s, i.recv);
    if vm.heap.class(owner).is_singleton && !recv.is_nil() {
        let r = vm.inspect_str(recv)?;
        out.push_str(&format!("{r}.{name}"));
    } else {
        let od = vm.inspect_str(Value::Obj(owner))?;
        let rk = class_real(vm, klass);
        if owner == klass || owner == rk { out.push_str(&format!("{od}#{name}")); }
        else { let kd = vm.inspect_str(Value::Obj(rk))?; out.push_str(&format!("{kd}({od})#{name}")); }
    }
    out.push('>');
    Ok(vm.str_from(out))
}

fn method_eql(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    if a.len() != 1 { return Err(vm.argnum_error(a.len(), "1")); }
    let other = a[0];
    let i = ivs(vm);
    let (Value::Obj(so), Value::Obj(oo)) = (s, other) else { return Ok(Value::False) };
    if vm.real_class_of(s) != vm.real_class_of(other) { return Ok(Value::False); }
    if !matches!(vm.heap.get(oo).kind, ObjKind::Object) { return Ok(Value::False); }
    for k in [i.owner, i.recv, i.name, i.proc_, i.klass] { if !vm.heap.get(oo).ivars.iter().any(|(x, _)| *x == k) { return Ok(Value::False); } }
    let _ = so;
    if iv(vm, s, i.owner) != iv(vm, other, i.owner) { return Ok(Value::False); }
    if iv(vm, s, i.recv) != iv(vm, other, i.recv) { return Ok(Value::False); }
    let (p1, p2) = (iv(vm, s, i.proc_), iv(vm, other, i.proc_));
    if p1.is_nil() && p2.is_nil() {
        // two natives are the same method when they are the same function
        // (the reference compares the cfunc procs); a `respond_to_missing?`
        // method is equal to one of the same name
        let (m1, m2) = (iv(vm, s, i.missing).truthy(), iv(vm, other, i.missing).truthy());
        if m1 || m2 { return Ok(Value::bool(m1 == m2 && iv(vm, s, i.name) == iv(vm, other, i.name))); }
        let (o1, n1) = match (iv(vm, s, i.owner), iv(vm, s, i.name)) { (Value::Obj(o), Value::Sym(n)) => (o, n), _ => return Ok(Value::False) };
        let (o2, n2) = match (iv(vm, other, i.owner), iv(vm, other, i.name)) { (Value::Obj(o), Value::Sym(n)) => (o, n), _ => return Ok(Value::False) };
        return Ok(Value::bool(match (search(vm, o1, n1), search(vm, o2, n2)) {
            (Some((Method::Native(f), _)), Some((Method::Native(g), _))) => core::ptr::fn_addr_eq(f, g),
            (Some((Method::Closure(f), _)), Some((Method::Closure(g), _))) => alloc::sync::Arc::ptr_eq(&f, &g),
            (Some((Method::AttrReader(x), _)), Some((Method::AttrReader(y), _))) => x == y,
            (Some((Method::AttrWriter(x), _)), Some((Method::AttrWriter(y), _))) => x == y,
            _ => false,
        }));
    }
    if p1.is_nil() || p2.is_nil() { return Ok(Value::False); }
    let eq = vm.intern("==");
    vm.funcall(p1, eq, &[p2], Value::Nil)
}

fn super_method(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let i = ivs(vm);
    let recv = iv(vm, s, i.recv);
    let klass = match iv(vm, s, i.klass) { Value::Obj(o) => o, _ => return Ok(Value::Nil) };
    let owner = match iv(vm, s, i.owner) { Value::Obj(o) => o, _ => return Ok(Value::Nil) };
    let name = match iv(vm, s, i.name) { Value::Sym(n) => n, _ => return Ok(Value::Nil) };
    let start = if vm.heap.class(owner).is_module {
        let mut r = vm.heap.class(klass).superclass;
        while let Some(x) = r { if vm.heap.class(x).iclass_of == Some(owner) { break; } r = vm.heap.class(x).superclass; }
        match r { Some(x) => vm.heap.class(x).superclass, None => return Ok(Value::Nil) }
    } else { vm.heap.class(owner).superclass };
    let start = match start { Some(x) => x, None => return Ok(Value::Nil) };
    let (m, found) = match search(vm, start, name) { Some(x) => x, None => return Ok(Value::Nil) };
    let sup = table_class(vm, found);
    let sup = if vm.heap.class(sup).is_module { sup } else { class_real(vm, sup) };
    let proc_ = match m { Method::Ruby(p) => Value::Obj(p), _ => Value::Nil };
    let cn = { let c = vm.real_class_of(s); vm.class_name(c) };
    Ok(new_method_obj(vm, &cn, sup, recv, name, proc_, sup, false))
}

fn parameters(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let i = ivs(vm);
    if iv(vm, s, i.missing).truthy() { let r = vm.intern("rest"); let e = vm.ary_new(vec![Value::Sym(r)]); return Ok(vm.ary_new(vec![e])); }
    match iv(vm, s, i.proc_) {
        Value::Obj(p) => Ok(super::ext_proc::parameters_of(vm, p)),
        _ => {
            let n = method_arity(vm, s);
            let (req, rest) = if n >= 0 { (n, false) } else { (-n - 1, true) };
            let (sr, srest) = (vm.intern("req"), vm.intern("rest"));
            let mut out = vec![];
            for _ in 0..req { let e = vm.ary_new(vec![Value::Sym(sr)]); out.push(e); }
            if rest { let e = vm.ary_new(vec![Value::Sym(srest)]); out.push(e); }
            Ok(vm.ary_new(out))
        }
    }
}

pub fn init(vm: &mut Vm) {
    let object = vm.core.object;
    let unbound = vm.define_class("UnboundMethod", object);
    let method = vm.define_class("Method", object);
    for c in [unbound, method] {
        let sc = vm.singleton_class(Value::Obj(c)).expect("singleton");
        let new = vm.intern("new");
        let _ = vm.undef_method(sc, new);
    }
    let shared: &[(&str, crate::object::NativeFn)] = &[
        ("==", method_eql), ("eql?", method_eql), ("to_s", method_to_s), ("inspect", method_to_s),
        ("super_method", super_method),
        ("arity", |vm, s, _a, _b| Ok(Value::Int(method_arity(vm, s)))),
        ("source_location", |vm, s, _a, _b| {
            // the Proc the Method wraps answers for it; a native (or a `method_missing` stand-in)
            // has no irep and answers nil, as the reference does
            let i = ivs(vm);
            if iv(vm, s, i.missing).truthy() { return Ok(Value::Nil); }
            match iv(vm, s, i.proc_) {
                Value::Obj(p) => Ok(super::ext_proc::source_location_of(vm, p)),
                _ => Ok(Value::Nil),
            }
        }),
        ("parameters", parameters),
        ("owner", |vm, s, _a, _b| { let i = ivs(vm); Ok(iv(vm, s, i.owner)) }),
        ("name", |vm, s, _a, _b| { let i = ivs(vm); Ok(iv(vm, s, i.name)) }),
    ];
    vm.define_methods(unbound, shared);
    vm.define_methods(method, shared);
    vm.define_methods(unbound, &[
        ("bind", |vm, s, a, _b| { if a.len() != 1 { return Err(vm.argnum_error(a.len(), "1")); } let i = ivs(vm); let owner = match iv(vm, s, i.owner) { Value::Obj(o) => o, _ => vm.core.object }; bind_check(vm, a[0], owner)?; let (name, proc_, klass, missing) = (iv(vm, s, i.name), iv(vm, s, i.proc_), iv(vm, s, i.klass), iv(vm, s, i.missing).truthy()); let name = match name { Value::Sym(n) => n, _ => return Err(vm.raise_type("not a symbol")) }; let klass = match klass { Value::Obj(k) => k, _ => owner }; Ok(new_method_obj(vm, "Method", owner, a[0], name, proc_, klass, missing)) }),
        ("bind_call", |vm, s, a, b| { if a.is_empty() { return Err(vm.argnum_error(0, "1+")); } mcall(vm, s, Some(a[0]), &a[1..], b) }),
    ]);
    vm.define_methods(method, &[
        ("call", |vm, s, a, b| mcall(vm, s, None, a, b)),
        ("[]", |vm, s, a, b| mcall(vm, s, None, a, b)),
        ("unbind", |vm, s, _a, _b| { let i = ivs(vm); let owner = match iv(vm, s, i.owner) { Value::Obj(o) => o, _ => vm.core.object }; let name = match iv(vm, s, i.name) { Value::Sym(n) => n, _ => return Err(vm.raise_type("not a symbol")) }; let klass = match iv(vm, s, i.klass) { Value::Obj(k) => k, _ => owner }; let (proc_, missing) = (iv(vm, s, i.proc_), iv(vm, s, i.missing).truthy()); Ok(new_method_obj(vm, "UnboundMethod", owner, Value::Nil, name, proc_, klass, missing)) }),
        ("receiver", |vm, s, _a, _b| { let i = ivs(vm); Ok(iv(vm, s, i.recv)) }),
    ]);
    let k = vm.core.kernel;
    vm.define_methods(k, &[
        ("method", |vm, s, a, _b| { if a.len() != 1 { return Err(vm.argnum_error(a.len(), "1")); } let n = sym_arg(vm, a[0])?; let c = vm.class_of(s); method_alloc(vm, c, s, n, false, false) }),
        ("singleton_method", |vm, s, a, _b| { if a.len() != 1 { return Err(vm.argnum_error(a.len(), "1")); } let n = sym_arg(vm, a[0])?; let c = vm.class_of(s); method_alloc(vm, c, s, n, false, true) }),
    ]);
    let m = vm.core.module;
    vm.define_method(m, "instance_method", |vm, s, a, _b| { if a.len() != 1 { return Err(vm.argnum_error(a.len(), "1")); } let n = sym_arg(vm, a[0])?; let c = s.obj().unwrap(); method_alloc(vm, c, s, n, true, false) });
    // Arity of the natives the reference declares with an argument spec
    // (`MRB_ARGS_*`); SabiRuby natives carry none, so the common ones are listed.
    let core = vm.core;
    let table: Vec<(ObjId, &str, i64)> = vec![
        (core.string, "upcase", 0), (core.string, "downcase", 0), (core.string, "size", 0), (core.string, "length", 0), (core.string, "to_s", 0), (core.string, "inspect", 0), (core.string, "+", 1),
        (core.integer, "+", 1), (core.integer, "-", 1), (core.integer, "*", 1), (core.integer, "/", 1), (core.integer, "to_s", -1), (core.integer, "chr", -1),
        (core.float, "+", 1), (core.float, "-", 1), (core.array, "push", -1), (core.array, "first", -1), (core.array, "size", 0), (core.array, "[]", -1), (core.array, "each", 0),
        (core.hash, "[]", 1), (core.basic_object, "__id__", 0), (core.basic_object, "__send__", -2), (core.basic_object, "==", 1), (core.basic_object, "!", 0), (core.basic_object, "instance_eval", -1),
        (core.kernel, "inspect", 0), (core.kernel, "to_s", 0), (core.kernel, "object_id", 0), (core.kernel, "eql?", 1), (core.kernel, "class", 0), (core.kernel, "nil?", 0), (core.kernel, "send", -2),
        (method, "==", 1), (method, "eql?", 1), (unbound, "==", 1), (unbound, "eql?", 1),
    ];
    for (c, n, ar) in table { let s = vm.intern(n); if let Some((Method::Native(f), _)) = search(vm, c, s) { vm.native_arity.push((f, ar)); } }
    let _ = Value::Nil;
}
