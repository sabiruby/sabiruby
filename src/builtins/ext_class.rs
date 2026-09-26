//! mruby-class-ext (`mrbgems/mruby-class-ext/src/class.c`): `Module#<`, `<=`, `<=>`, `>`, `>=`,
//! `class_exec`/`module_exec`, `name`, `singleton_class?`; `Class#attached_object`,
//! `subclasses`. No Ruby part.

use alloc::vec::Vec;

use crate::argc;
use crate::error::VmResult;
use crate::value::{ObjId, Value};
use crate::vm::Vm;

use super::object::class_arg;

/// `is_ancestor`: `sup` is on `klass`'s superclass chain (an include class stands for its module).
fn is_ancestor(vm: &Vm, klass: ObjId, sup: ObjId) -> bool {
    let mut c = Some(klass);
    while let Some(x) = c {
        let cd = vm.heap.class(x);
        if x == sup || cd.iclass_of == Some(sup) { return true; }
        c = cd.superclass;
    }
    false
}

/// `mod_compare_hierarchy`: true when `s` descends from `o`, false the other way round, nil
/// when unrelated; a TypeError unless both are classes or modules.
fn compare_hierarchy(vm: &mut Vm, s: Value, o: Value) -> VmResult<Value> {
    let (sc, oc) = match (s, o) {
        (Value::Obj(a), Value::Obj(b)) if vm.heap.is_class(a) && vm.heap.is_class(b) => (a, b),
        _ => return Err(vm.raise_type("compared with non class/module")),
    };
    if is_ancestor(vm, sc, oc) { return Ok(Value::True); }
    if is_ancestor(vm, oc, sc) { return Ok(Value::False); }
    Ok(Value::Nil)
}

fn mod_lt(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    if s == a[0] { return Ok(Value::False); }
    compare_hierarchy(vm, s, a[0])
}
fn mod_le(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    compare_hierarchy(vm, s, a[0])
}
fn mod_gt(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    if s == a[0] { return Ok(Value::False); }
    compare_hierarchy(vm, a[0], s)
}
fn mod_ge(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    compare_hierarchy(vm, a[0], s)
}
fn mod_cmp(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    if s == a[0] { return Ok(Value::Int(0)); }
    if !matches!(a[0], Value::Obj(o) if vm.heap.is_class(o)) { return Ok(Value::Nil); }
    Ok(match compare_hierarchy(vm, s, a[0])? { Value::True => Value::Int(-1), Value::False => Value::Int(1), _ => Value::Nil })
}

/// `class_exec`/`module_exec`: the block runs with the module as self and definition target;
/// the arguments (keywords included) go to the block.
fn mod_module_exec(vm: &mut Vm, s: Value, a: &[Value], b: Value) -> VmResult<Value> {
    if b.is_nil() { return Err(vm.raise_arg("no block given")); }
    let kw = match (vm.pending_kw, a.last()) { (Some(k), Some(l)) if !k.is_nil() && k == *l => Some(k), _ => None };
    let pos = if kw.is_some() { &a[..a.len() - 1] } else { a };
    // called by a SEND the block becomes the frame (`mod_module_exec` → `mrb_object_exec`)
    vm.exec_block_with_self(b, s, pos, kw)
}

/// `Module#name`: the class path as a frozen String; nil for an anonymous or singleton class.
fn mod_name(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let o = s.obj().unwrap();
    let cd = vm.heap.class(o);
    if cd.name.is_none() || cd.is_singleton { return Ok(Value::Nil); }
    let n = vm.class_name(o);
    let v = vm.str_from(n);
    if let Some(so) = v.obj() { vm.heap.get_mut(so).frozen = true; }
    Ok(v)
}

/// `mrb_class_real`: the nearest superclass that is neither a singleton nor an include class.
fn class_real(vm: &Vm, mut c: Option<ObjId>) -> Option<ObjId> {
    while let Some(x) = c {
        let cd = vm.heap.class(x);
        if !cd.is_singleton && cd.iclass_of.is_none() && cd.origin_of.is_none() { return Some(x); }
        c = cd.superclass;
    }
    None
}

/// `Class#subclasses`: every class whose real superclass is the receiver (a heap walk, as the
/// reference's `mrb_objspace_each_objects`); singleton classes are left out, the order is the
/// heap's.
fn class_subclasses(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let me = s.obj().unwrap();
    let mut out: Vec<Value> = Vec::new();
    for id in vm.heap.ids() {
        if !vm.heap.is_class(id) { continue; }
        let cd = vm.heap.class(id);
        if cd.is_module || cd.is_singleton || cd.iclass_of.is_some() || cd.origin_of.is_some() { continue; }
        if class_real(vm, cd.superclass) == Some(me) { out.push(Value::Obj(id)); }
    }
    Ok(vm.ary_new(out))
}

pub fn init(vm: &mut Vm) {
    let c = vm.core;
    vm.define_methods(c.module, &[
        ("<", mod_lt),
        ("<=", mod_le),
        ("<=>", mod_cmp),
        (">", mod_gt),
        (">=", mod_ge),
        ("class_exec", mod_module_exec),
        ("module_exec", mod_module_exec),
        ("name", mod_name),
        ("singleton_class?", |vm, s, _a, _b| Ok(Value::bool(vm.heap.class(s.obj().unwrap()).is_singleton))),
    ]);
    vm.define_methods(c.class, &[
        ("attached_object", |vm, s, _a, _b| {
            let cd = vm.heap.class(s.obj().unwrap());
            if !cd.is_singleton { return Err(vm.raise_type("not a singleton class")); }
            Ok(cd.attached.map(|x| x.get()).unwrap_or(Value::Nil))
        }),
        ("subclasses", class_subclasses),
    ]);
    let _ = class_arg;
}
