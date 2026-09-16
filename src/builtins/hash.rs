//! Hash (insertion-ordered, linear lookup with `eql?` semantics).

use alloc::{format, vec, vec::Vec};

use crate::argc;
use crate::error::VmResult;
use crate::object::ObjKind;
use crate::value::{Slot, Value};
use crate::vm::Vm;

fn entries(vm: &Vm, v: Value) -> Vec<(Value, Value)> {
    match v.obj().map(|o| &vm.heap.get(o).kind) { Some(ObjKind::Hash(h)) => h.entries().iter().map(|(k, v)| (k.get(), v.get())).collect(), _ => vec![] }
}
fn to_slots(e: Vec<(Value, Value)>) -> Vec<(Slot, Slot)> { e.into_iter().map(|(k, v)| (Slot::from(k), Slot::from(v))).collect() }
fn hash_len(vm: &Vm, v: Value) -> usize {
    match v.obj().map(|o| &vm.heap.get(o).kind) { Some(ObjKind::Hash(h)) => h.len(), _ => 0 }
}
fn default_of(vm: &Vm, v: Value) -> Value {
    match v.obj().map(|o| &vm.heap.get(o).kind) { Some(ObjKind::Hash(h)) => h.default.get(), _ => Value::Nil }
}
fn with_mut<R>(vm: &mut Vm, v: Value, f: impl FnOnce(&mut crate::object::HashData) -> R) -> VmResult<R> {
    match v.obj() {
        Some(o) => {
            if vm.heap.get(o).frozen { return Err(vm.raise(vm.core.frozen_error, "can't modify frozen Hash")); }
            match &mut vm.heap.get_mut(o).kind { ObjKind::Hash(h) => Ok(f(h)), _ => Err(vm.raise_type("not a hash")) }
        }
        None => Err(vm.raise_type("not a hash")),
    }
}
fn default_proc_ivar(vm: &mut Vm) -> crate::symbol::Sym { vm.intern("__default_proc") }

pub fn hash_inspect(vm: &mut Vm, v: Value) -> VmResult<Vec<u8>> {
    let list = entries(vm, v);
    if list.is_empty() { return Ok(b"{}".to_vec()); }
    let mut out = b"{".to_vec();
    for (i, (k, val)) in list.iter().enumerate() {
        if i > 0 { out.extend_from_slice(b", "); }
        match k {
            Value::Sym(s) => {
                // `key: value` form when the symbol is simple
                let name = vm.syms.name(*s).to_vec();
                let insp = super::symbol::sym_inspect(&name);
                if insp.len() == name.len() + 1 && !name.ends_with(b"=") { out.extend(name); out.extend_from_slice(b": "); }
                else { out.extend(insp); out.extend_from_slice(b" => "); }
            }
            _ => { if *k == v { out.extend_from_slice(b"{...}"); } else { out.extend(vm.inspect(*k)?); } out.extend_from_slice(b" => "); }
        }
        if *val == v { out.extend_from_slice(b"{...}"); } else { out.extend(vm.inspect(*val)?); }
    }
    out.push(b'}');
    Ok(out)
}

pub(crate) fn hash_aref(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    if let Some(v) = vm.hash_get(s, a[0]) { return Ok(v); }
    // a redefined `default` is honoured (#3272)
    let dm = vm.intern("default");
    if let Some((crate::object::Method::Ruby(_) | crate::object::Method::Closure(_), _)) = vm.find_method(vm.class_of(s), dm) { return vm.funcall(s, dm, &[a[0]], Value::Nil); }
    let dp = default_proc_ivar(vm);
    let proc_ = s.obj().map(|o| vm.heap.ivar_get(o, dp)).unwrap_or(Value::Nil);
    if !proc_.is_nil() { return vm.call_block(proc_, &[s, a[0]]); }
    Ok(default_of(vm, s))
}

/// `Hash#default_proc=` (`mrb_hash_set_default_proc`, src/hash.c). Assigning one replaces the
/// plain default, as it does there: mruby keeps both in the one `ifnone` ivar and the two flags
/// say which it is, so writing either clears the other. `nil` takes the default away altogether.
///
/// The argument must be a Proc, and a **lambda** must be one that could be called with the two
/// arguments the lookup passes (`hash_set_default_proc`): an arity of 2, or a negative one no
/// smaller than -3 (`|*a|`, `|a, *b|`, `|a, b, *c|`). A plain proc is not checked, because a
/// proc takes whatever it is given.
fn default_proc_set(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    let dp = default_proc_ivar(vm);
    let o = match s.obj() { Some(o) => o, None => return Err(vm.raise_type("not a hash")) };
    if vm.heap.get(o).frozen { return Err(vm.raise(vm.core.frozen_error, "can't modify frozen Hash")); }
    if a[0].is_nil() {
        vm.heap.ivar_set(o, dp, Value::Nil);
        with_mut(vm, s, |h| h.default = Slot::NIL)?;
        return Ok(Value::Nil);
    }
    let po = match a[0].obj() {
        Some(p) if matches!(vm.heap.get(p).kind, ObjKind::Proc(_)) => p,
        _ => { let d = vm.describe_for_type_error(a[0]); return Err(vm.raise_type(&format!("wrong argument type {d} (expected Proc)"))); }
    };
    let pd = vm.heap.proc_data(po);
    if pd.strict {
        let n = super::proc_::arity_of(&vm.ireps[pd.irep], true);
        if n != 2 && (n >= 0 || n < -3) {
            let n = if n < 0 { -n - 1 } else { n };
            return Err(vm.raise_type(&format!("default_proc takes two arguments (2 for {n})")));
        }
    }
    vm.heap.ivar_set(o, dp, a[0]);
    with_mut(vm, s, |h| h.default = Slot::NIL)?;
    Ok(a[0])
}

/// `Hash#[]=`. Named, not a closure in the table, because `OP_SETIDX` records it as the
/// implementation it stands in for and calls it directly (`Vm::op_setidx`).
pub(crate) fn hash_aset(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 2);
    if s.obj().map(|o| vm.heap.get(o).frozen).unwrap_or(false) { return Err(vm.raise(vm.core.frozen_error, "can't modify frozen Hash")); }
    vm.hash_set(s, a[0], a[1])?;
    Ok(a[1])
}

pub fn init(vm: &mut Vm) {
    let c = vm.core;
    // Hash[...]: pairs, a flat key/value list, or another Hash
    let sc = vm.singleton_class(Value::Obj(c.hash)).unwrap();
    vm.define_method(sc, "[]", |vm, s, a, _b| {
        let h = vm.instance_alloc(s.obj().unwrap())?;
        if a.len() == 1 {
            if let Some(src) = a[0].obj().and_then(|o| match &vm.heap.get(o).kind { ObjKind::Hash(hd) => Some(hd.entries().to_vec()), _ => None }) { for (k, v) in src { vm.hash_set(h, k.get(), v.get())?; } return Ok(h); }
            if let Some(list) = vm.ary_vals(a[0]) { for pair in list { match vm.ary_vals(pair) { Some(p) if p.len() == 2 => vm.hash_set(h, p[0], p[1])?, Some(p) if p.len() == 1 => vm.hash_set(h, p[0], Value::Nil)?, _ => { let d = vm.inspect_str(pair)?; return Err(vm.raise_arg(&format!("wrong element type {d} (expected array)"))); } } } return Ok(h); }
        }
        if a.len() % 2 != 0 { return Err(vm.raise_arg("odd number of arguments for Hash")); }
        for p in a.chunks(2) { vm.hash_set(h, p[0], p[1])?; }
        Ok(h)
    });
    vm.define_methods(c.hash, &[
        ("initialize", |vm, s, a, b| { argc!(vm, a, 0, 1); if !b.is_nil() { if !a.is_empty() { return Err(vm.argnum_error(1, "0")); } let dp = default_proc_ivar(vm); vm.heap.ivar_set(s.obj().unwrap(), dp, b); return Ok(s); } if let Some(d) = a.first() { with_mut(vm, s, |h| h.default = Slot::from(*d))?; } Ok(s) }),
        ("assoc", |vm, s, a, _b| { argc!(vm, a, 1); for (k, v) in entries(vm, s) { if vm.equal(k, a[0])? { return Ok(vm.ary_new(vec![k, v])); } } Ok(Value::Nil) }),
        ("rassoc", |vm, s, a, _b| { argc!(vm, a, 1); for (k, v) in entries(vm, s) { if vm.equal(v, a[0])? { return Ok(vm.ary_new(vec![k, v])); } } Ok(Value::Nil) }),
        ("__pat_values", |vm, s, a, _b| { argc!(vm, a, 1); let keys = vm.ary_vals(a[0]).unwrap_or_default(); let mut out = vec![]; for k in keys { match vm.hash_get(s, k) { Some(v) => out.push(v), None => return Ok(Value::False) } } Ok(vm.ary_new(out)) }),
        ("__except", |vm, s, a, _b| { argc!(vm, a, 1); let keys = vm.ary_vals(a[0]).unwrap_or_default(); let h = vm.hash_new(); for (k, v) in entries(vm, s) { if !keys.iter().any(|x| vm.eql(*x, k)) { vm.hash_set(h, k, v)?; } } Ok(h) }),
        ("initialize_copy", |vm, s, a, _b| { argc!(vm, a, 1); let e = entries(vm, a[0]); let d = default_of(vm, a[0]); with_mut(vm, s, |h| { h.set_entries(to_slots(e)); h.default = Slot::from(d); })?; Ok(s) }),
        ("replace", |vm, s, a, _b| { argc!(vm, a, 1); if !matches!(a[0].obj().map(|o| &vm.heap.get(o).kind), Some(ObjKind::Hash(_))) { let d = vm.describe_for_type_error(a[0]); return Err(vm.raise_type(&format!("{d} cannot be converted to Hash"))); } let e = entries(vm, a[0]); let d = default_of(vm, a[0]); with_mut(vm, s, |h| { h.set_entries(to_slots(e)); h.default = Slot::from(d); })?; let dp = default_proc_ivar(vm); let p = a[0].obj().map(|o| vm.heap.ivar_get(o, dp)).unwrap_or(Value::Nil); vm.heap.ivar_set(s.obj().unwrap(), dp, p); Ok(s) }),
        ("[]", hash_aref),
        ("[]=", hash_aset),
        ("store", |vm, s, a, _b| { argc!(vm, a, 2); vm.hash_set(s, a[0], a[1])?; Ok(a[1]) }),
        ("fetch", |vm, s, a, b| { argc!(vm, a, 1, 2); if let Some(v) = vm.hash_get(s, a[0]) { return Ok(v); } if !b.is_nil() { return vm.call_block(b, &[a[0]]); } if a.len() == 2 { return Ok(a[1]); } let k = vm.inspect_str(a[0])?; Err(vm.raise(vm.core.key_error, &format!("Key not found: {k}"))) }),
        ("dig", |vm, s, a, _b| { let mut cur = s; let aref = vm.s.aref; for k in a { if cur.is_nil() { return Ok(Value::Nil); } cur = vm.funcall(cur, aref, &[*k], Value::Nil)?; } Ok(cur) }),
        ("size", |vm, s, _a, _b| Ok(Value::Int(hash_len(vm, s) as i64))),
        ("length", |vm, s, _a, _b| Ok(Value::Int(hash_len(vm, s) as i64))),
        ("empty?", |vm, s, _a, _b| Ok(Value::bool(hash_len(vm, s) == 0))),
        ("keys", |vm, s, _a, _b| { let k: Vec<Value> = entries(vm, s).iter().map(|(k, _)| *k).collect(); Ok(vm.ary_new(k)) }),
        ("values", |vm, s, _a, _b| { let v: Vec<Value> = entries(vm, s).iter().map(|(_, v)| *v).collect(); Ok(vm.ary_new(v)) }),
        ("values_at", |vm, s, a, _b| { let mut out = vec![]; for k in a { out.push(hash_aref(vm, s, &[*k], Value::Nil)?); } Ok(vm.ary_new(out)) }),
        ("has_key?", |vm, s, a, _b| { argc!(vm, a, 1); Ok(Value::bool(vm.hash_get(s, a[0]).is_some())) }),
        ("key?", |vm, s, a, _b| { argc!(vm, a, 1); Ok(Value::bool(vm.hash_get(s, a[0]).is_some())) }),
        ("include?", |vm, s, a, _b| { argc!(vm, a, 1); Ok(Value::bool(vm.hash_get(s, a[0]).is_some())) }),
        ("member?", |vm, s, a, _b| { argc!(vm, a, 1); Ok(Value::bool(vm.hash_get(s, a[0]).is_some())) }),
        ("has_value?", |vm, s, a, _b| { argc!(vm, a, 1); for (_, v) in entries(vm, s) { if vm.equal(a[0], v)? { return Ok(Value::True); } } Ok(Value::False) }),
        ("value?", |vm, s, a, _b| { argc!(vm, a, 1); for (_, v) in entries(vm, s) { if vm.equal(a[0], v)? { return Ok(Value::True); } } Ok(Value::False) }),
        ("key", |vm, s, a, _b| { argc!(vm, a, 1); for (k, v) in entries(vm, s) { if vm.equal(v, a[0])? { return Ok(k); } } Ok(Value::Nil) }),
        ("delete", |vm, s, a, b| { argc!(vm, a, 1); if s.obj().map(|o| vm.heap.get(o).frozen).unwrap_or(false) { return Err(vm.frozen_error(s)); } match vm.hash_delete(s, a[0]) { Some(v) => Ok(v), None => if b.is_nil() { Ok(Value::Nil) } else { vm.call_block(b, &[a[0]]) } } }),
        ("clear", |vm, s, _a, _b| { with_mut(vm, s, |h| h.clear())?; Ok(s) }),
        ("shift", |vm, s, _a, _b| { let first = with_mut(vm, s, |h| if h.is_empty() { None } else { Some(h.remove_entry(0)) })?; match first { Some((k, v)) => Ok(vm.ary_new(vec![k.get(), v.get()])), None => Ok(Value::Nil) } }),
        ("default", |vm, s, a, _b| { argc!(vm, a, 0, 1); let dp = default_proc_ivar(vm); let proc_ = s.obj().map(|o| vm.heap.ivar_get(o, dp)).unwrap_or(Value::Nil); if !proc_.is_nil() { if let Some(k) = a.first() { return vm.call_block(proc_, &[s, *k]); } return Ok(Value::Nil); } Ok(default_of(vm, s)) }),
        ("rehash", |vm, s, _a, _b| {
            // rebuild: keys are re-hashed and compared with eql?; a later duplicate replaces the earlier
            if s.obj().map(|o| vm.heap.get(o).frozen).unwrap_or(false) { return Err(vm.frozen_error(s)); }
            let old = entries(vm, s);
            let mut out: Vec<(Value, Value)> = Vec::with_capacity(old.len());
            let mut hs: Vec<i64> = Vec::with_capacity(old.len());
            for (k, v) in old {
                let kh = vm.key_hash(k)?;
                let mut pos = None;
                for (i, (ek, _)) in out.iter().enumerate() { if hs[i] == kh && vm.key_eql(k, *ek)? { pos = Some(i); break; } }
                match pos { Some(i) => out[i].1 = v, None => { out.push((k, v)); hs.push(kh); } }
            }
            with_mut(vm, s, |h| h.set_entries_with_hashes(to_slots(out), hs))?;
            Ok(s)
        }),
        // `mrb_hash_set_default`: writing the plain default clears MRB_HASH_PROC_DEFAULT, so a
        // Hash made with a block answers with the plain default from then on
        ("default=", |vm, s, a, _b| { argc!(vm, a, 1); with_mut(vm, s, |h| h.default = Slot::from(a[0]))?; let dp = default_proc_ivar(vm); if let Some(o) = s.obj() { vm.heap.ivar_set(o, dp, Value::Nil); } Ok(a[0]) }),
        ("default_proc", |vm, s, _a, _b| { let dp = default_proc_ivar(vm); Ok(s.obj().map(|o| vm.heap.ivar_get(o, dp)).unwrap_or(Value::Nil)) }),
        ("default_proc=", default_proc_set),
        ("to_h", |_vm, s, _a, _b| Ok(s)),
        ("to_hash", |_vm, s, _a, _b| Ok(s)),
        ("to_a", |vm, s, _a, _b| { let pairs: Vec<Value> = entries(vm, s).into_iter().map(|(k, v)| vm.ary_new(vec![k, v])).collect(); Ok(vm.ary_new(pairs)) }),
        ("inspect", |vm, s, _a, _b| { let b = hash_inspect(vm, s)?; Ok(vm.str_new(&b)) }),
        ("to_s", |vm, s, _a, _b| { let b = hash_inspect(vm, s)?; Ok(vm.str_new(&b)) }),
        ("==", hash_eq),
        ("eql?", |vm, s, a, _b| { argc!(vm, a, 1); if s == a[0] { return Ok(Value::True); } let y = match a[0].obj().map(|o| &vm.heap.get(o).kind) { Some(ObjKind::Hash(h)) => h.entries().iter().map(|(k, v)| (k.get(), v.get())).collect::<Vec<_>>(), _ => return Ok(Value::False) }; let x = entries(vm, s); if x.len() != y.len() { return Ok(Value::False); } let pair = (s.obj().unwrap(), a[0].obj().unwrap()); if vm.eq_guard.contains(&pair) { return Ok(Value::True); } vm.eq_guard.push(pair); let eql = vm.s.eql; let mut res = Ok(Value::True); for (k, v) in x { match vm.hash_get(a[0], k) { Some(w) => match vm.funcall(v, eql, &[w], Value::Nil) { Ok(t) if t.truthy() => {} Ok(_) => { res = Ok(Value::False); break; } Err(e) => { res = Err(e); break; } }, None => { res = Ok(Value::False); break; } } } vm.eq_guard.pop(); res }),
        ("hash", |vm, s, _a, _b| { let mut h: i64 = 0; let hs = vm.s.hash; for (k, v) in entries(vm, s) { let x = match vm.funcall(k, hs, &[], Value::Nil)? { Value::Int(i) => i, _ => 0 }; let y = match vm.funcall(v, hs, &[], Value::Nil)? { Value::Int(i) => i, _ => 0 }; h = h.wrapping_add(x.wrapping_mul(31).wrapping_add(y)); } Ok(Value::Int(h)) }),
        ("merge", |vm, s, a, b| { for other in a { if !matches!(other.obj().map(|o| &vm.heap.get(o).kind), Some(ObjKind::Hash(_))) { let d = vm.describe_for_type_error(*other); return Err(vm.raise_type(&format!("{d} cannot be converted to Hash"))); } } let mut out = entries(vm, s); let d = default_of(vm, s); for other in a { for (k, v) in entries(vm, *other) { match out.iter().position(|(ek, _)| vm.eql(*ek, k)) { Some(i) => { out[i].1 = if b.is_nil() { v } else { vm.call_block(b, &[k, out[i].1, v])? }; } None => out.push((k, v)) } } } let cls = vm.real_class_of(s); let h = vm.instance_alloc(cls)?; let ivars = s.obj().map(|o| vm.heap.get(o).ivars.clone()).unwrap_or_default(); vm.heap.get_mut(h.obj().unwrap()).ivars = ivars; with_mut(vm, h, |hd| { hd.set_entries(to_slots(out)); hd.default = Slot::from(d); })?; Ok(h) }),
        ("merge!", |vm, s, a, b| { for other in a { for (k, v) in entries(vm, *other) { let cur = vm.hash_get(s, k); let nv = match cur { Some(c) if !b.is_nil() => vm.call_block(b, &[k, c, v])?, _ => v }; vm.hash_set(s, k, nv)?; } } Ok(s) }),
        ("update", |vm, s, a, b| { for other in a { for (k, v) in entries(vm, *other) { let cur = vm.hash_get(s, k); let nv = match cur { Some(c) if !b.is_nil() => vm.call_block(b, &[k, c, v])?, _ => v }; vm.hash_set(s, k, nv)?; } } Ok(s) }),
        ("__delete", |vm, s, a, _b| { argc!(vm, a, 1); if s.obj().map(|o| vm.heap.get(o).frozen).unwrap_or(false) { return Err(vm.frozen_error(s)); } Ok(vm.hash_delete(s, a[0]).unwrap_or(Value::Nil)) }),
        ("__update", |vm, s, a, _b| { argc!(vm, a, 1); for (k, v) in entries(vm, a[0]) { vm.hash_set(s, k, v)?; } Ok(s) }),
        ("__merge", |vm, s, a, _b| { for other in a { if !matches!(other.obj().map(|o| &vm.heap.get(o).kind), Some(ObjKind::Hash(_))) { let d = vm.describe_for_type_error(*other); return Err(vm.raise_type(&format!("{d} cannot be converted to Hash"))); } for (k, v) in entries(vm, *other) { vm.hash_set(s, k, v)?; } } Ok(s) }),
        ("dup", |vm, s, _a, _b| { let e = entries(vm, s); let d = default_of(vm, s); let c = vm.real_class_of(s); let ivars = s.obj().map(|o| vm.heap.get(o).ivars.clone()).unwrap_or_default(); let n = vm.heap.alloc(c, ObjKind::Hash(crate::object::HashData::from_entries(to_slots(e), Slot::from(d)))); vm.heap.get_mut(n).ivars = ivars; Ok(Value::Obj(n)) }),
        ("freeze", |vm, s, _a, _b| { if let Some(o) = s.obj() { vm.heap.get_mut(o).frozen = true; } Ok(s) }),
        ("frozen?", |vm, s, _a, _b| Ok(Value::bool(s.obj().map(|o| vm.heap.get(o).frozen).unwrap_or(true)))),
        ("invert", |vm, s, _a, _b| { let h = vm.hash_new(); for (k, v) in entries(vm, s) { vm.hash_set(h, v, k)?; } Ok(h) }),
    ]);
    // `MRB_MT_PRIVATE` in the reference's ROM table for this class (src/hash.c)
    vm.mark_private(c.hash, &["initialize", "initialize_copy"]);
}

fn hash_eq(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    if s == a[0] { return Ok(Value::True); }
    if let (Value::Obj(x), Value::Obj(y)) = (s, a[0]) {
        if vm.eq_guard.contains(&(x, y)) { return Ok(Value::True); }
        vm.eq_guard.push((x, y));
        let r = hash_eq_inner(vm, s, a);
        vm.eq_guard.pop();
        return r;
    }
    hash_eq_inner(vm, s, a)
}

fn hash_eq_inner(vm: &mut Vm, s: Value, a: &[Value]) -> VmResult<Value> {
    let (x, y) = (entries(vm, s), match a[0].obj().map(|o| &vm.heap.get(o).kind) { Some(ObjKind::Hash(h)) => h.entries().to_vec(), _ => return Ok(Value::False) });
    if x.len() != y.len() { return Ok(Value::False); }
    for (k, v) in x {
        match vm.hash_get(a[0], k) { Some(w) => { if !vm.equal(v, w)? { return Ok(Value::False); } } None => return Ok(Value::False) }
    }
    let _ = y;
    Ok(Value::True)
}
