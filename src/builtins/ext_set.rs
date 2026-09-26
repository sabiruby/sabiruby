//! mruby-set (`mrbgems/mruby-set/src/set.c`): `Set`. Its Ruby part (`initialize`, `merge`,
//! `replace`, `subtract`, the set operators over `__merge`/`__union`/..., `each`, `delete_if`,
//! `classify`, ...) is `src/mrblib/set.mrb`.
//!
//! A Set is a Hash-shaped object (`ObjKind::Hash`, element => true) with the class `Set`, so
//! membership follows the Hash's `hash`/`eql?` rule and the order of elements is insertion
//! order (the reference's khash walks its buckets). As Hash keys, unfrozen Strings are
//! stored as frozen copies. The reference's "uninitialized Set" state does not exist here.

use alloc::{format, vec, vec::Vec};

use crate::argc;
use crate::error::VmResult;
use crate::object::{InstanceKind, ObjKind};
use crate::value::{ObjId, Value};
use crate::vm::Vm;

use super::object::same_object;

const GOLDEN_RATIO_PRIME: u64 = 0x9e3779b97f4a7c15;

fn set_class(vm: &mut Vm) -> ObjId {
    let n = vm.intern("Set");
    match vm.const_get(vm.core.object, n) { Some(Value::Obj(c)) => c, _ => vm.core.object }
}

fn is_set(vm: &mut Vm, v: Value) -> bool {
    let c = set_class(vm);
    matches!(v, Value::Obj(o) if matches!(vm.heap.get(o).kind, ObjKind::Hash(_))) && vm.obj_is_kind_of(v, c)
}

fn check_set(vm: &mut Vm, v: Value) -> VmResult<()> {
    if !is_set(vm, v) { return Err(vm.raise_arg("value must be a set")); }
    Ok(())
}

fn elems(vm: &Vm, s: Value) -> Vec<Value> {
    match s.obj().map(|o| &vm.heap.get(o).kind) { Some(ObjKind::Hash(hd)) => hd.entries().iter().map(|(k, _)| k.get()).collect(), _ => Vec::new() }
}

fn size(vm: &Vm, s: Value) -> usize {
    match s.obj().map(|o| &vm.heap.get(o).kind) { Some(ObjKind::Hash(hd)) => hd.len(), _ => 0 }
}

/// Membership. What `hash`/`eql?` raise counts as "not the same element": mruby-set calls
/// `eql?` under `mrb_protect_error` and answers not-equal on an error (`kset_equal_value`),
/// unlike Hash, which raises it.
fn has(vm: &mut Vm, s: Value, v: Value) -> bool {
    matches!(vm.hash_get(s, v), Ok(Some(_)))
}

fn put(vm: &mut Vm, s: Value, v: Value) -> VmResult<()> {
    vm.hash_set(s, v, Value::True)
}

/// As [`has`]: an error in `hash`/`eql?` is "not there".
fn del(vm: &mut Vm, s: Value, v: Value) -> bool {
    matches!(vm.hash_delete(s, v), Ok(Some(_)))
}

fn clear(vm: &mut Vm, s: Value) {
    if let Some(ObjKind::Hash(hd)) = s.obj().map(|o| &mut vm.heap.get_mut(o).kind) { hd.clear(); }
}

/// a new empty set of the receiver's class (`mrb_obj_new(class, 0, NULL)`)
fn new_like(vm: &mut Vm, s: Value) -> VmResult<Value> {
    let c = vm.real_class_of(s);
    vm.class_new_instance(c, &[], Value::Nil)
}

fn dup(vm: &mut Vm, s: Value) -> VmResult<Value> {
    let d = vm.intern("dup");
    vm.funcall(s, d, &[], Value::Nil)
}

fn set_init_copy(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    if !is_set(vm, a[0]) { return Err(vm.raise_type("initialize_copy should take a Set object")); }
    if vm.real_class_of(s) != vm.real_class_of(a[0]) { return Err(vm.raise_type("initialize_copy should take same class object")); }
    clear(vm, s);
    for e in elems(vm, a[0]) { put(vm, s, e)?; }
    Ok(s)
}

fn set_include_p(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    Ok(Value::bool(has(vm, s, a[0])))
}

fn set_add(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    super::ext_array::check_frozen(vm, s)?;
    put(vm, s, a[0])?;
    Ok(s)
}

fn set_add_p(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    super::ext_array::check_frozen(vm, s)?;
    if has(vm, s, a[0]) { return Ok(Value::Nil); }
    put(vm, s, a[0])?;
    Ok(s)
}

fn set_delete(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    super::ext_array::check_frozen(vm, s)?;
    del(vm, s, a[0]);
    Ok(s)
}

fn set_delete_p(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    super::ext_array::check_frozen(vm, s)?;
    Ok(if del(vm, s, a[0]) { s } else { Value::Nil })
}

/// `__merge(other)`: true when `other` is a Set (merged in place), false otherwise
fn core_merge(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    if !is_set(vm, a[0]) { return Ok(Value::False); }
    for e in elems(vm, a[0]) { put(vm, s, e)?; }
    Ok(Value::True)
}

fn core_subtract(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    if !is_set(vm, a[0]) { return Ok(Value::False); }
    for e in elems(vm, a[0]) { del(vm, s, e); }
    Ok(Value::True)
}

/// `__union(other)`: a copy of self with `other`'s elements, nil unless `other` is a Set
fn core_union(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    if !is_set(vm, a[0]) { return Ok(Value::Nil); }
    let r = dup(vm, s)?;
    for e in elems(vm, a[0]) { put(vm, r, e)?; }
    Ok(r)
}

fn core_difference(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    if !is_set(vm, a[0]) { return Ok(Value::Nil); }
    let r = dup(vm, s)?;
    for e in elems(vm, a[0]) { del(vm, r, e); }
    Ok(r)
}

fn core_intersection(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    if !is_set(vm, a[0]) { return Ok(Value::Nil); }
    let r = new_like(vm, s)?;
    for e in elems(vm, a[0]) { if has(vm, s, e) { put(vm, r, e)?; } }
    Ok(r)
}

fn core_xor(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    if !is_set(vm, a[0]) { return Ok(Value::Nil); }
    let r = new_like(vm, s)?;
    for e in elems(vm, s) { if !has(vm, a[0], e) { put(vm, r, e)?; } }
    for e in elems(vm, a[0]) { if !has(vm, s, e) { put(vm, r, e)?; } }
    Ok(r)
}

fn set_equal(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    if same_object(s, a[0]) { return Ok(Value::True); }
    if !is_set(vm, a[0]) { return Ok(Value::False); }
    if size(vm, s) != size(vm, a[0]) { return Ok(Value::False); }
    for e in elems(vm, s) { if !has(vm, a[0], e) { return Ok(Value::False); } }
    Ok(Value::True)
}

/// order-independent: the size and each element's hash (32 bits, as `khint_t`) folded by xor
fn set_hash_m(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let n = size(vm, s) as u64;
    let mut h: u64 = n.wrapping_mul(GOLDEN_RATIO_PRIME);
    for e in elems(vm, s) {
        let eh = vm.value_hash(e) as u32 as u64;
        h ^= eh.wrapping_mul(GOLDEN_RATIO_PRIME);
    }
    h ^= h >> 32;
    Ok(Value::Int(h as i64))
}

/// every element of `sub` is in `sup`
fn all_in(vm: &mut Vm, sub: Value, sup: Value) -> bool {
    elems(vm, sub).into_iter().all(|e| has(vm, sup, e))
}

fn set_superset_p(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    check_set(vm, a[0])?;
    if size(vm, a[0]) == 0 { return Ok(Value::True); }
    if size(vm, s) < size(vm, a[0]) { return Ok(Value::False); }
    Ok(Value::bool(all_in(vm, a[0], s)))
}

fn set_proper_superset_p(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    check_set(vm, a[0])?;
    if size(vm, a[0]) == 0 { return Ok(Value::bool(size(vm, s) != 0)); }
    if size(vm, s) <= size(vm, a[0]) { return Ok(Value::False); }
    Ok(Value::bool(all_in(vm, a[0], s)))
}

fn set_subset_p(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    check_set(vm, a[0])?;
    if size(vm, s) == 0 { return Ok(Value::True); }
    if size(vm, a[0]) < size(vm, s) { return Ok(Value::False); }
    Ok(Value::bool(all_in(vm, s, a[0])))
}

fn set_proper_subset_p(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    check_set(vm, a[0])?;
    if size(vm, s) == 0 { return Ok(Value::bool(size(vm, a[0]) != 0)); }
    if size(vm, a[0]) <= size(vm, s) { return Ok(Value::False); }
    Ok(Value::bool(all_in(vm, s, a[0])))
}

fn set_intersect_p(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    check_set(vm, a[0])?;
    if size(vm, s) == 0 || size(vm, a[0]) == 0 { return Ok(Value::False); }
    let (small, big) = if size(vm, s) < size(vm, a[0]) { (s, a[0]) } else { (a[0], s) };
    Ok(Value::bool(elems(vm, small).into_iter().any(|e| has(vm, big, e))))
}

fn set_disjoint_p(vm: &mut Vm, s: Value, a: &[Value], b: Value) -> VmResult<Value> {
    let r = set_intersect_p(vm, s, a, b)?;
    Ok(Value::bool(!r.truthy()))
}

/// `<=>`: -1/0/1 for proper subset/equal/proper superset, nil when neither
fn set_cmp(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    if !is_set(vm, a[0]) { return Ok(Value::Nil); }
    let (ns, no) = (size(vm, s), size(vm, a[0]));
    if ns == 0 { return Ok(Value::Int(if no == 0 { 0 } else { -1 })); }
    if no == 0 { return Ok(Value::Int(1)); }
    if ns < no { return Ok(if all_in(vm, s, a[0]) { Value::Int(-1) } else { Value::Nil }); }
    if ns > no { return Ok(if all_in(vm, a[0], s) { Value::Int(1) } else { Value::Nil }); }
    Ok(if all_in(vm, s, a[0]) { Value::Int(0) } else { Value::Nil })
}

fn set_join(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 0, 1);
    let sep = match a.first() { Some(v) => Some(vm.expect_str(*v, "separator")?), None => None };
    let mut out: Vec<u8> = Vec::new();
    for (i, e) in elems(vm, s).into_iter().enumerate() {
        if i > 0 { if let Some(sp) = &sep { out.extend_from_slice(sp); } }
        let t = vm.as_string(e)?;
        out.extend_from_slice(&t);
    }
    Ok(vm.str_new(&out))
}

fn set_inspect(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let c = vm.real_class_of(s);
    let name = vm.class_name(c);
    let o = s.obj().unwrap();
    if size(vm, s) == 0 { return Ok(vm.str_from(format!("{name}[]"))); }
    if vm.inspect_guard.contains(&o) { return Ok(vm.str_from(format!("{name}[...]"))); }
    vm.inspect_guard.push(o);
    let mut out = format!("{name}[");
    let mut r = Ok(());
    for (i, e) in elems(vm, s).into_iter().enumerate() {
        if i > 0 { out.push_str(", "); }
        match vm.inspect_str(e) { Ok(t) => out.push_str(&t), Err(e) => { r = Err(e); break; } }
    }
    vm.inspect_guard.pop();
    r?;
    out.push(']');
    Ok(vm.str_from(out))
}

fn set_reset(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    super::ext_array::check_frozen(vm, s)?;
    Ok(s)
}

fn set_add_all(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    for v in a { put(vm, s, *v)?; }
    Ok(s)
}

fn set_delete_all(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    for v in a { del(vm, s, *v); }
    Ok(s)
}

fn set_include_all_p(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    Ok(Value::bool(a.iter().all(|v| has(vm, s, *v))))
}

fn set_include_any_p(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    if size(vm, s) == 0 { return Ok(Value::False); }
    Ok(Value::bool(a.iter().any(|v| has(vm, s, *v))))
}

const MAX_NESTED_DEPTH: usize = 16;

/// nested sets are flattened into `target`; deeper than 16 levels (a cycle) is an error
fn flatten_into(vm: &mut Vm, target: Value, source: Value, depth: &mut usize) -> VmResult<bool> {
    if *depth >= MAX_NESTED_DEPTH { return Ok(false); }
    for e in elems(vm, source) {
        if is_set(vm, e) {
            *depth += 1;
            if !flatten_into(vm, target, e, depth)? { return Ok(false); }
            *depth -= 1;
        } else {
            put(vm, target, e)?;
        }
    }
    Ok(true)
}

fn has_nested(vm: &mut Vm, s: Value) -> bool {
    elems(vm, s).into_iter().any(|e| is_set(vm, e))
}

fn do_flatten(vm: &mut Vm, target: Value, source: Value) -> VmResult<()> {
    let mut depth = 0;
    if !flatten_into(vm, target, source, &mut depth)? { return Err(vm.raise_arg("flatten recursion depth too deep")); }
    Ok(())
}

fn set_flatten(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    if size(vm, s) == 0 { return new_like(vm, s); }
    if !has_nested(vm, s) { return dup(vm, s); }
    let r = new_like(vm, s)?;
    do_flatten(vm, r, s)?;
    Ok(r)
}

fn set_flatten_bang(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    super::ext_array::check_frozen(vm, s)?;
    if size(vm, s) == 0 || !has_nested(vm, s) { return Ok(Value::Nil); }
    let tmp = new_like(vm, s)?;
    do_flatten(vm, tmp, s)?;
    clear(vm, s);
    for e in elems(vm, tmp) { put(vm, s, e)?; }
    Ok(s)
}

/// `Set[*elements]`
fn s_create(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    let set = vm.class_new_instance(s.obj().unwrap(), &[], Value::Nil)?;
    for v in a { put(vm, set, *v)?; }
    Ok(set)
}

pub fn init(vm: &mut Vm) {
    let set = vm.define_class("Set", vm.core.object);
    vm.heap.class_mut(set).instance_kind = Some(InstanceKind::Hash);
    let en = vm.intern("Enumerable");
    if let Some(Value::Obj(m)) = vm.const_get(vm.core.object, en) { vm.include_module(set, m); }
    let sc = vm.singleton_class(Value::Obj(set)).expect("Set singleton");
    vm.define_method(sc, "[]", s_create);
    vm.define_methods(set, &[
        ("size", |vm, s, _a, _b| Ok(Value::Int(size(vm, s) as i64))),
        ("length", |vm, s, _a, _b| Ok(Value::Int(size(vm, s) as i64))),
        ("empty?", |vm, s, _a, _b| Ok(Value::bool(size(vm, s) == 0))),
        ("clear", |vm, s, _a, _b| { super::ext_array::check_frozen(vm, s)?; clear(vm, s); Ok(s) }),
        ("to_a", |vm, s, _a, _b| { let e = elems(vm, s); Ok(vm.ary_new(e)) }),
        ("include?", set_include_p),
        ("member?", set_include_p),
        ("===", set_include_p),
        ("add", set_add),
        ("<<", set_add),
        ("add?", set_add_p),
        ("delete", set_delete),
        ("delete?", set_delete_p),
        ("__init", |_vm, s, _a, _b| Ok(s)),
        ("__merge", core_merge),
        ("__subtract", core_subtract),
        ("__union", core_union),
        ("__difference", core_difference),
        ("__intersection", core_intersection),
        ("__xor", core_xor),
        ("==", set_equal),
        ("eql?", set_equal),
        ("hash", set_hash_m),
        ("join", set_join),
        ("inspect", set_inspect),
        ("to_s", set_inspect),
        ("reset", set_reset),
        ("add_all", set_add_all),
        ("delete_all", set_delete_all),
        ("include_all?", set_include_all_p),
        ("include_any?", set_include_any_p),
        ("superset?", set_superset_p),
        (">=", set_superset_p),
        ("proper_superset?", set_proper_superset_p),
        (">", set_proper_superset_p),
        ("subset?", set_subset_p),
        ("<=", set_subset_p),
        ("proper_subset?", set_proper_subset_p),
        ("<", set_proper_subset_p),
        ("intersect?", set_intersect_p),
        ("disjoint?", set_disjoint_p),
        ("<=>", set_cmp),
        ("flatten", set_flatten),
        ("flatten!", set_flatten_bang),
        ("initialize_copy", set_init_copy),
    ]);
    let ic = vm.intern("initialize_copy");
    let _ = vm.set_visibility(set, ic, crate::object::Vis::Private);
    let _ = vec![0u8];
}
