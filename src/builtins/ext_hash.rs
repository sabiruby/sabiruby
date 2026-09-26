//! mruby-hash-ext (`mrbgems/mruby-hash-ext/src/hash_ext.c`) plus the core
//! `__compact` its Ruby part (`src/mrblib/hash-ext.mrb`) relies on.

use alloc::{format, vec, vec::Vec};

use crate::argc;
use crate::error::VmResult;
use crate::object::ObjKind;
use crate::value::{Slot, Value};
use crate::vm::Vm;

fn entries(vm: &Vm, v: Value) -> Vec<(Value, Value)> {
    match v.obj().map(|o| &vm.heap.get(o).kind) { Some(ObjKind::Hash(h)) => h.entries().iter().map(|(k, v)| (k.get(), v.get())).collect(), _ => vec![] }
}
fn is_hash(vm: &Vm, v: Value) -> bool { matches!(v.obj().map(|o| &vm.heap.get(o).kind), Some(ObjKind::Hash(_))) }
fn check_frozen(vm: &mut Vm, v: Value) -> VmResult<()> {
    if v.obj().map(|o| vm.heap.get(o).frozen).unwrap_or(false) { return Err(vm.frozen_error(v)); }
    Ok(())
}
fn set_class(vm: &mut Vm, h: Value, klass: Value) {
    if let (Value::Obj(o), Value::Obj(c)) = (h, klass) { if c != vm.core.hash { vm.heap.get_mut(o).class = c; } }
}

pub fn init(vm: &mut Vm) {
    let hash = vm.core.hash;
    vm.define_methods(hash, &[
        ("values_at", |vm, s, a, _b| { let mut out = Vec::with_capacity(a.len()); for k in a { out.push(super::hash::hash_value(vm, s, *k)?); } Ok(vm.ary_new(out)) }),
        ("slice", |vm, s, a, _b| { let h = vm.hash_new(); for k in a { if let Some(v) = vm.hash_get(s, *k) { vm.hash_set(h, *k, v)?; } } Ok(h) }),
        ("slice!", |vm, s, a, _b| {
            check_frozen(vm, s)?;
            let mut remove = vec![];
            for (k, _) in entries(vm, s) { let mut keep = false; for x in a { if vm.key_eql(*x, k)? { keep = true; break; } } if !keep { remove.push(k); } }
            let removed = vm.hash_new();
            for k in remove { let v = vm.hash_delete(s, k).unwrap_or(Value::Nil); vm.hash_set(removed, k, v)?; }
            Ok(removed)
        }),
        ("except", |vm, s, a, _b| { let h = vm.hash_new(); for (k, v) in entries(vm, s) { vm.hash_set(h, k, v)?; } for k in a { vm.hash_delete(h, *k); } Ok(h) }),
        ("key", |vm, s, a, _b| { argc!(vm, a, 1); for (k, v) in entries(vm, s) { if vm.equal(v, a[0])? { return Ok(k); } } Ok(Value::Nil) }),
        ("__merge", |vm, s, a, _b| {
            if a.is_empty() { return Err(vm.argnum_error(0, "1+")); }
            for other in a { if !is_hash(vm, *other) { let c = vm.real_class_of(*other); let cn = vm.class_name(c); return Err(vm.raise_type(&format!("no implicit conversion of {cn} into Hash"))); } }
            check_frozen(vm, s)?;
            for other in a { for (k, v) in entries(vm, *other) { vm.hash_set(s, k, v)?; } }
            Ok(s)
        }),
        // core hash.c: implementation of Hash#compact!
        ("__compact", |vm, s, _a, _b| {
            check_frozen(vm, s)?;
            let o = match s.obj() { Some(o) => o, None => return Ok(Value::Nil) };
            let removed = match &mut vm.heap.get_mut(o).kind {
                ObjKind::Hash(hd) => { let n = hd.len(); let stale = hd.hashes_stale(); let mut hs = Vec::with_capacity(n); let mut es: Vec<(Slot, Slot)> = Vec::with_capacity(n); for (i, e) in hd.entries().iter().enumerate() { if !e.1.get().is_nil() { es.push(*e); if let Some(h) = hd.hash_at(i) { hs.push(h); } } } let removed = n - es.len(); if stale { hd.set_entries(es); } else { hd.set_entries_with_hashes(es, hs); } removed }
                _ => 0,
            };
            Ok(if removed == 0 { Value::Nil } else { s })
        }),
    ]);
    let sc = vm.singleton_class(Value::Obj(hash)).expect("Hash singleton");
    vm.define_method(sc, "[]", |vm, s, a, _b| {
        if a.len() == 1 {
            let obj = a[0];
            if is_hash(vm, obj) { let h = vm.hash_new(); for (k, v) in entries(vm, obj) { vm.hash_set(h, k, v)?; } set_class(vm, h, s); return Ok(h); }
            if let Some(list) = vm.ary_vals(obj) {
                let h = vm.hash_new();
                for elem in list {
                    let pair = match vm.ary_vals(elem) { Some(p) => p, None => { let c = vm.real_class_of(elem); let cn = vm.class_name(c); return Err(vm.raise_arg(&format!("wrong element type {cn} (expected array)"))); } };
                    let (k, v) = match pair.len() { 2 => (pair[0], pair[1]), 1 => (pair[0], Value::Nil), n => return Err(vm.raise_arg(&format!("invalid number of elements ({n} for 1..2)"))) };
                    vm.hash_set(h, k, v)?;
                }
                set_class(vm, h, s);
                return Ok(h);
            }
        }
        if a.len() % 2 != 0 { return Err(vm.raise_arg("odd number of arguments for Hash")); }
        let h = vm.hash_new();
        for pair in a.chunks(2) { vm.hash_set(h, pair[0], pair[1])?; }
        set_class(vm, h, s);
        Ok(h)
    });
}
