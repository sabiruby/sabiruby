//! Range (`each`, `to_a` etc. come from mrblib; the primitives are here).

use alloc::{vec, vec::Vec};

use crate::argc;
use crate::error::VmResult;
use crate::object::ObjKind;
use crate::value::{Slot, Value};
use crate::vm::Vm;

fn parts(vm: &Vm, v: Value) -> (Value, Value, bool) {
    match v.obj().map(|o| &vm.heap.get(o).kind) { Some(ObjKind::Range { begin, end, excl }) => (begin.get(), end.get(), *excl), _ => (Value::Nil, Value::Nil, false) }
}

pub fn range_inspect(vm: &mut Vm, v: Value, insp: bool) -> VmResult<Vec<u8>> {
    let (b, e, x) = parts(vm, v);
    let mut out = if b.is_nil() && insp { vec![] } else if insp { vm.inspect(b)? } else { vm.as_string(b)? };
    out.extend_from_slice(if x { b"..." } else { b".." });
    if !(e.is_nil() && insp) { out.extend(if insp { vm.inspect(e)? } else { vm.as_string(e)? }); }
    Ok(out)
}

fn cover(vm: &mut Vm, s: Value, v: Value) -> VmResult<bool> {
    let (b, e, x) = parts(vm, s);
    let cmp = vm.intern("<=>");
    if !b.is_nil() {
        let r = vm.funcall(b, cmp, &[v], Value::Nil)?;
        match r { Value::Int(i) if i <= 0 => {} _ => return Ok(false) }
    }
    if !e.is_nil() {
        let r = vm.funcall(v, cmp, &[e], Value::Nil)?;
        match r { Value::Int(i) if i < 0 || (i == 0 && !x) => {} _ => return Ok(false) }
    }
    Ok(true)
}

pub fn init(vm: &mut Vm) {
    let c = vm.core;
    let rsc = vm.singleton_class(Value::Obj(c.range)).unwrap();
    vm.define_method(rsc, "new", |vm, s, a, _b| {
        argc!(vm, a, 2, 3);
        let excl = a.len() == 3 && a[2].truthy();
        vm.check_range_ends(a[0], a[1])?;
        let r = vm.instance_alloc(s.obj().unwrap())?;
        if let Some(o) = r.obj() { vm.heap.get_mut(o).kind = ObjKind::Range { begin: Slot::from(a[0]), end: Slot::from(a[1]), excl }; }
        Ok(r)
    });
    vm.define_methods(c.range, &[
        // private (set below); `dup`/`clone` call it on the fresh copy
        ("initialize_copy", |vm, s, a, _b| { argc!(vm, a, 1); if s == a[0] { return Ok(s); } let cls = vm.real_class_of(s); if vm.real_class_of(a[0]) != cls { return Err(vm.raise_type("wrong argument class")); } if s.obj().map(|o| matches!(vm.heap.get(o).kind, ObjKind::Range { .. })).unwrap_or(false) { let n = vm.intern("initialize"); return Err(vm.name_error(n, "'initialize' called twice")); } let (b, e, x) = match a[0].obj().map(|o| &vm.heap.get(o).kind) { Some(ObjKind::Range { begin, end, excl }) => (*begin, *end, *excl), _ => return Err(vm.raise_type("wrong argument class")) }; if let Some(o) = s.obj() { vm.heap.get_mut(o).kind = ObjKind::Range { begin: b, end: e, excl: x }; } Ok(s) }),
        ("initialize", |vm, s, a, _b| { argc!(vm, a, 2, 3); if s.obj().map(|o| matches!(vm.heap.get(o).kind, ObjKind::Range { .. })).unwrap_or(false) { let n = vm.intern("initialize"); return Err(vm.name_error(n, "'initialize' called twice")); } let excl = a.len() == 3 && a[2].truthy(); vm.check_range_ends(a[0], a[1])?; if let Some(o) = s.obj() { vm.heap.get_mut(o).kind = ObjKind::Range { begin: Slot::from(a[0]), end: Slot::from(a[1]), excl }; } Ok(s) }),

        ("begin", |vm, s, _a, _b| Ok(parts(vm, s).0)),
        ("first", |vm, s, a, b| { argc!(vm, a, 0, 1); let (bg, _, _) = parts(vm, s); if a.is_empty() { if bg.is_nil() { return Err(vm.raise(vm.core.range_error, "cannot get the first element of beginless range")); } return Ok(bg); } let n = vm.expect_int(a[0], "argument")?; if n < 0 { return Err(vm.raise_arg("negative array size (or size too big)")); } let mut out = vec![]; let (bb, e, x) = parts(vm, s); if let (Value::Int(mut i), Value::Int(last)) = (bb, e) { while out.len() < n as usize && (i < last || (i == last && !x)) { out.push(Value::Int(i)); i += 1; } } else { let each = vm.s.each; let _ = b; if n == 0 { return Ok(vm.ary_new(vec![])); } let _ = each; return Err(vm.raise(vm.core.not_implemented_error, "Range#first(n) for non-integer ranges")); } Ok(vm.ary_new(out)) }),
        ("end", |vm, s, _a, _b| Ok(parts(vm, s).1)),
        ("last", |vm, s, a, _b| { argc!(vm, a, 0, 1); let (b, e, x) = parts(vm, s); if a.is_empty() { return Ok(e); } if e.is_nil() { return Err(vm.raise(vm.core.range_error, "cannot get the last element of endless range")); } let n = vm.expect_int(a[0], "argument")?; if n < 0 { return Err(vm.raise_arg("negative array size")); } if let (Value::Int(bb), Value::Int(ee)) = (b, e) { let last = if x { ee - 1 } else { ee }; let start = (last - n + 1).max(bb); let v: Vec<Value> = (start..=last).map(Value::Int).collect(); return Ok(vm.ary_new(v)); } Err(vm.raise(vm.core.not_implemented_error, "Range#last(n) for non-integer ranges")) }),
        ("exclude_end?", |vm, s, _a, _b| Ok(Value::bool(parts(vm, s).2))),
        ("==", |vm, s, a, _b| { argc!(vm, a, 1); if s == a[0] { return Ok(Value::True); } if !matches!(a[0].obj().map(|o| &vm.heap.get(o).kind), Some(ObjKind::Range { .. })) { return Ok(Value::False); } let (b1, e1, x1) = parts(vm, s); let (b2, e2, x2) = parts(vm, a[0]); Ok(Value::bool(x1 == x2 && vm.equal(b1, b2)? && vm.equal(e1, e2)?)) }),
        ("eql?", |vm, s, a, _b| { argc!(vm, a, 1); let (b1, e1, x1) = parts(vm, s); let (b2, e2, x2) = parts(vm, a[0]); Ok(Value::bool(matches!(a[0].obj().map(|o| &vm.heap.get(o).kind), Some(ObjKind::Range { .. })) && x1 == x2 && vm.eql(b1, b2) && vm.eql(e1, e2))) }),
        ("===", |vm, s, a, _b| { argc!(vm, a, 1); Ok(Value::bool(cover(vm, s, a[0])?)) }),
        ("include?", |vm, s, a, _b| { argc!(vm, a, 1); Ok(Value::bool(cover(vm, s, a[0])?)) }),
        ("member?", |vm, s, a, _b| { argc!(vm, a, 1); Ok(Value::bool(cover(vm, s, a[0])?)) }),
        ("cover?", |vm, s, a, _b| { argc!(vm, a, 1); Ok(Value::bool(cover(vm, s, a[0])?)) }),
        ("to_s", |vm, s, _a, _b| { let b = range_inspect(vm, s, false)?; Ok(vm.str_new(&b)) }),
        ("inspect", |vm, s, _a, _b| { let b = range_inspect(vm, s, true)?; Ok(vm.str_new(&b)) }),
        ("size", |vm, s, _a, _b| { let (b, e, x) = parts(vm, s); match (b, e) { (Value::Int(b), Value::Int(e)) => { let n = if x { e - b } else { e - b + 1 }; Ok(Value::Int(n.max(0))) } (Value::Int(_), Value::Nil) => Ok(Value::Float(f64::INFINITY)), _ => Ok(Value::Nil) } }),
        ("__num_to_a", |vm, s, _a, _b| { let (b, e, x) = parts(vm, s); match (b, e) { (Value::Int(b), Value::Int(e)) => { let last = if x { e - 1 } else { e }; let v: Vec<Value> = (b..=last).map(Value::Int).collect(); Ok(vm.ary_new(v)) } (Value::Int(_), Value::Nil) => Err(vm.raise(vm.core.range_error, "cannot convert endless range to an array")), _ => Ok(Value::Nil) } }),
        ("dup", |vm, s, _a, _b| { let (b, e, x) = parts(vm, s); Ok(vm.range_new(b, e, x)) }),
        ("hash", |vm, s, _a, _b| { let (b, e, x) = parts(vm, s); Ok(Value::Int(vm.value_hash(b).wrapping_mul(31).wrapping_add(vm.value_hash(e)).wrapping_add(x as i64))) }),
    ]);
    // `MRB_MT_PRIVATE` in `range_rom_entries` (src/range.c) for both
    vm.mark_private(c.range, &["initialize", "initialize_copy"]);
}
