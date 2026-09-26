//! Array.

use alloc::{format, vec, vec::Vec};

use crate::argc;
use crate::error::VmResult;
use crate::object::{ArrayData, ObjKind};
use crate::value::{slots_of, values_of, Slot, Value};
use crate::vm::Vm;

fn items(vm: &Vm, v: Value) -> Vec<Value> {
    vm.ary_vals(v).unwrap_or_default()
}
/// How many elements, without copying them out (`items(vm, v).len()` allocates).
fn ary_len(vm: &Vm, v: Value) -> usize {
    vm.ary(v).map(|l| l.len()).unwrap_or(0)
}
/// The elements as they lie in the heap, for the natives that only read a few of them.
/// `items` copies the whole array to answer; this borrows it, so the borrow has to end
/// before anything touches `vm` again (nothing here calls Ruby while holding it).
fn slots(vm: &Vm, v: Value) -> &[Slot] {
    vm.ary(v).unwrap_or(&[])
}
fn with_mut<R>(vm: &mut Vm, v: Value, f: impl FnOnce(&mut ArrayData) -> R) -> VmResult<R> {
    match v.obj() {
        Some(o) => {
            if vm.heap.get(o).frozen { return Err(vm.raise(vm.core.frozen_error, "can't modify frozen Array")); }
            match &mut vm.heap.get_mut(o).kind { ObjKind::Array(a) => Ok(f(a)), _ => Err(vm.raise_type("not an array")) }
        }
        None => Err(vm.raise_type("not an array")),
    }
}

pub fn ary_inspect(vm: &mut Vm, v: Value) -> VmResult<Vec<u8>> {
    let list = items(vm, v);
    let mut out = b"[".to_vec();
    for (i, it) in list.iter().enumerate() {
        if i > 0 { out.extend_from_slice(b", "); }
        if *it == v { out.extend_from_slice(b"[...]"); } else { out.extend(vm.inspect(*it)?); }
    }
    out.push(b']');
    Ok(out)
}

fn ary_eq(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    if s == a[0] { return Ok(Value::True); }
    if let (Value::Obj(x), Value::Obj(y)) = (s, a[0]) {
        if vm.eq_guard.contains(&(x, y)) { return Ok(Value::True); }
        vm.eq_guard.push((x, y));
        let r = ary_eq_inner(vm, s, a);
        vm.eq_guard.pop();
        return r;
    }
    ary_eq_inner(vm, s, a)
}

fn ary_eq_inner(vm: &mut Vm, s: Value, a: &[Value]) -> VmResult<Value> {
    let (x, y) = (items(vm, s), match vm.ary_vals(a[0]) { Some(y) => y, None => return Ok(Value::False) });
    if x.len() != y.len() { return Ok(Value::False); }
    for (p, q) in x.iter().zip(y.iter()) { if !vm.equal(*p, *q)? { return Ok(Value::False); } }
    Ok(Value::True)
}

fn ary_cmp(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    let (x, y) = (items(vm, s), match vm.ary_vals(a[0]) { Some(y) => y, None => return Ok(Value::Nil) });
    let cmp = vm.intern("<=>");
    for (p, q) in x.iter().zip(y.iter()) {
        let r = vm.funcall(*p, cmp, &[*q], Value::Nil)?;
        match r { Value::Int(0) => {} Value::Int(_) => return Ok(r), _ => return Ok(Value::Nil) }
    }
    Ok(Value::Int(x.len().cmp(&y.len()) as i64))
}

/// What a comparison of `a` with `b` answered (`<=>` or a sort block), as an order: an
/// Integer by its sign, nil is an error, and anything else is asked `> 0` then `< 0` (a NaN is
/// a tie).
fn sort_order(vm: &mut Vm, v: Value, a: Value, b: Value) -> VmResult<core::cmp::Ordering> {
    match v {
        Value::Int(i) => Ok(i.cmp(&0)),
        Value::Nil => { let x = vm.describe_for_error(a); let y = vm.describe_for_error(b); Err(vm.raise_arg(&format!("comparison of {x} with {y} failed"))) }
        v => {
            let (gt, lt) = (vm.s.gt, vm.s.lt);
            if vm.funcall(v, gt, &[Value::Int(0)], Value::Nil)?.truthy() { return Ok(core::cmp::Ordering::Greater); }
            if vm.funcall(v, lt, &[Value::Int(0)], Value::Nil)?.truthy() { return Ok(core::cmp::Ordering::Less); }
            Ok(core::cmp::Ordering::Equal)
        }
    }
}

/// `__sort_cmp(answer, a, b)`: [`sort_order`] for the Ruby loop of `sort! { }`
/// (`src/mrblib/block-frames.rb`), as -1, 0 or 1.
fn sort_cmp(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 3);
    Ok(Value::Int(sort_order(vm, a[0], a[1], a[2])? as i64))
}

fn sort_values(vm: &mut Vm, list: &mut Vec<Value>, blk: Value) -> VmResult<()> {
    // insertion-free merge sort via a comparator that may raise: collect errors.
    let cmp = vm.intern("<=>");
    let mut err = None;
    let mut compare = |vm: &mut Vm, a: Value, b: Value| -> core::cmp::Ordering {
        if err.is_some() { return core::cmp::Ordering::Equal; }
        let r = if blk.is_nil() {
            match (a, b) {
                (Value::Int(p), Value::Int(q)) => return p.cmp(&q),
                _ => vm.funcall(a, cmp, &[b], Value::Nil),
            }
        } else { vm.call_block(blk, &[a, b]) };
        match r.and_then(|v| sort_order(vm, v, a, b)) {
            Ok(o) => o,
            Err(e) => { err = Some(e); core::cmp::Ordering::Equal }
        }
    };
    // simple merge sort (stable) driving the comparator
    let n = list.len();
    let mut buf = list.clone();
    let mut width = 1;
    while width < n {
        let mut i = 0;
        while i < n {
            let mid = (i + width).min(n); let hi = (i + 2 * width).min(n);
            let (mut l, mut r, mut k) = (i, mid, i);
            while l < mid && r < hi {
                if compare(vm, list[r], list[l]) == core::cmp::Ordering::Less { buf[k] = list[r]; r += 1; } else { buf[k] = list[l]; l += 1; }
                k += 1;
            }
            while l < mid { buf[k] = list[l]; l += 1; k += 1; }
            while r < hi { buf[k] = list[r]; r += 1; k += 1; }
            i += 2 * width;
        }
        core::mem::swap(list, &mut buf);
        width *= 2;
    }
    match err { Some(e) => Err(e), None => Ok(()) }
}

fn flatten(vm: &mut Vm, v: Value, depth: i64, out: &mut Vec<Value>) {
    for it in items(vm, v) {
        if depth != 0 && vm.ary(it).is_some() { flatten(vm, it, depth - 1, out); } else { out.push(it); }
    }
}

pub fn init(vm: &mut Vm) {
    let c = vm.core;
    let sc = vm.singleton_class(Value::Obj(c.array)).unwrap();
    vm.define_method(sc, "[]", |vm, s, a, _b| { let v = vm.instance_alloc(s.obj().unwrap())?; with_mut(vm, v, |arr| arr.extend(slots_of(a)))?; Ok(v) });
    vm.define_methods(c.array, &[
        ("initialize", |vm, s, a, b| {
            argc!(vm, a, 0, 2);
            let v = match a.first() {
                None => vec![],
                Some(Value::Int(n)) => { if *n < 0 { return Err(vm.raise_arg("negative array size")); } if !b.is_nil() && vm.in_frame() { let m = vm.intern("__init_by"); return vm.send_in_frame(s, m, &[Value::Int(*n)], None, b); } let n = *n as usize; if !b.is_nil() { let mut v = Vec::with_capacity(n); for i in 0..n { v.push(vm.call_block(b, &[Value::Int(i as i64)])?); } v } else { vec![a.get(1).copied().unwrap_or(Value::Nil); n] } }
                Some(x) => match vm.ary_vals(*x) { Some(v) => v, None => return Err(vm.raise_type("no implicit conversion into Integer")) },
            };
            with_mut(vm, s, |arr| *arr = slots_of(&v).into())?; Ok(s)
        }),
        ("initialize_copy", |vm, s, a, _b| { argc!(vm, a, 1); let v: Vec<Slot> = slots(vm, a[0]).to_vec(); with_mut(vm, s, |arr| *arr = v.into())?; Ok(s) }),
        ("replace", |vm, s, a, _b| { argc!(vm, a, 1); let v: Vec<Slot> = slots(vm, a[0]).to_vec(); with_mut(vm, s, |arr| *arr = v.into())?; Ok(s) }),
        ("size", |vm, s, _a, _b| Ok(Value::Int(vm.ary(s).map(|v| v.len()).unwrap_or(0) as i64))),
        ("length", |vm, s, _a, _b| Ok(Value::Int(vm.ary(s).map(|v| v.len()).unwrap_or(0) as i64))),
        ("count", |vm, s, a, b| { if a.is_empty() && b.is_nil() { return Ok(Value::Int(ary_len(vm, s) as i64)); } let list = items(vm, s); if let Some(x) = a.first() { let mut n = 0; for it in list { if vm.equal(it, *x)? { n += 1; } } return Ok(Value::Int(n)); } if !b.is_nil() { let mut n = 0; for it in list { if vm.call_block(b, &[it])?.truthy() { n += 1; } } return Ok(Value::Int(n)); } Ok(Value::Int(list.len() as i64)) }),
        ("empty?", |vm, s, _a, _b| Ok(Value::bool(vm.ary(s).map(|v| v.is_empty()).unwrap_or(true)))),
        ("[]", ary_aref),
        ("slice", ary_aref),
        ("[]=", ary_aset),
        ("at", |vm, s, a, _b| { argc!(vm, a, 1); ary_aref(vm, s, a, Value::Nil) }),
        ("fetch", |vm, s, a, b| { argc!(vm, a, 1, 2); let i = vm.expect_int(a[0], "index")?; let len = ary_len(vm, s) as i64; let idx = if i < 0 { i + len } else { i }; if idx >= 0 && idx < len { return Ok(slots(vm, s)[idx as usize].get()); } if !b.is_nil() { return vm.call_block(b, &[a[0]]); } if a.len() == 2 { return Ok(a[1]); } Err(vm.raise(vm.core.index_error, &format!("index {i} outside of array bounds: {}...{len}", -len))) }),
        ("dig", |vm, s, a, _b| { let mut cur = s; let aref = vm.s.aref; for k in a { if cur.is_nil() { return Ok(Value::Nil); } cur = vm.funcall(cur, aref, &[*k], Value::Nil)?; } Ok(cur) }),
        ("first", |vm, s, a, _b| { argc!(vm, a, 0, 1); if a.is_empty() { return Ok(vm.ary(s).and_then(|v| v.first().map(|x| x.get())).unwrap_or(Value::Nil)); } let n = vm.expect_int(a[0], "argument")?; if n < 0 { return Err(vm.raise_arg("negative array size")); } let v: Vec<Value> = slots(vm, s).iter().take(n as usize).map(|x| x.get()).collect(); Ok(vm.ary_new(v)) }),
        ("last", |vm, s, a, _b| { argc!(vm, a, 0, 1); if a.is_empty() { return Ok(vm.ary(s).and_then(|v| v.last().map(|x| x.get())).unwrap_or(Value::Nil)); } let n = vm.expect_int(a[0], "argument")?; if n < 0 { return Err(vm.raise_arg("negative array size")); } let l = slots(vm, s); let n = (n as usize).min(l.len()); let v: Vec<Value> = l[l.len() - n..].iter().map(|x| x.get()).collect(); Ok(vm.ary_new(v)) }),
        ("<<", |vm, s, a, _b| { argc!(vm, a, 1); with_mut(vm, s, |arr| arr.push(Slot::from(a[0])))?; Ok(s) }),
        ("push", |vm, s, a, _b| { with_mut(vm, s, |arr| arr.extend(slots_of(a)))?; Ok(s) }),
        ("append", |vm, s, a, _b| { with_mut(vm, s, |arr| arr.extend(slots_of(a)))?; Ok(s) }),
        ("pop", |vm, s, a, _b| { argc!(vm, a, 0, 1); if a.is_empty() { return with_mut(vm, s, |arr| arr.pop().map(|x| x.get()).unwrap_or(Value::Nil)); } let n = vm.expect_int(a[0], "argument")? as usize; let v = with_mut(vm, s, |arr| { let k = arr.len().saturating_sub(n); values_of(&arr.split_off(k)) })?; Ok(vm.ary_new(v)) }),
        ("shift", |vm, s, a, _b| { argc!(vm, a, 0, 1); if a.is_empty() { return with_mut(vm, s, |arr| arr.shift().map(|x| x.get()).unwrap_or(Value::Nil)); } let n = vm.expect_int(a[0], "argument")? as usize; let v = with_mut(vm, s, |arr| values_of(&arr.shift_n(n)))?; Ok(vm.ary_new(v)) }),
        ("unshift", |vm, s, a, _b| { with_mut(vm, s, |arr| arr.unshift(slots_of(a)))?; Ok(s) }),
        ("prepend", |vm, s, a, _b| { with_mut(vm, s, |arr| arr.unshift(slots_of(a)))?; Ok(s) }),
        ("insert", |vm, s, a, _b| { if a.is_empty() { return Err(vm.argnum_error(0, "1+")); } let i = vm.expect_int(a[0], "index")?; let len = ary_len(vm, s) as i64; let i = if i < 0 { i + len + 1 } else { i }; if i < 0 { return Err(vm.raise(vm.core.index_error, &format!("index {} too small for array", i - len - 1))); } let rest = a[1..].to_vec(); with_mut(vm, s, |arr| { let i = i as usize; if arr.len() < i { arr.resize(i, Slot::NIL); } arr.splice_range(i, i, slots_of(&rest)); })?; Ok(s) }),
        ("concat", |vm, s, a, _b| { let mut all = vec![]; for x in a { match vm.ary(*x) { Some(v) => all.extend(v.iter().map(|x| x.get())), None => return Err(vm.raise_type("no implicit conversion into Array")) } } with_mut(vm, s, |arr| arr.extend(slots_of(&all)))?; Ok(s) }),
        ("+", |vm, s, a, _b| { argc!(vm, a, 1); let mut v = items(vm, s); match vm.ary_vals(a[0]) { Some(o) => v.extend(o), None => return Err(vm.raise_type("no implicit conversion into Array")) } Ok(vm.ary_new(v)) }),
        ("-", |vm, s, a, _b| { argc!(vm, a, 1); let v = items(vm, s); let o = match vm.ary_vals(a[0]) { Some(o) => o, None => return Err(vm.raise_type("no implicit conversion into Array")) }; let mut out = vec![]; for it in v { let mut found = false; for x in &o { if vm.equal(it, *x)? { found = true; break; } } if !found { out.push(it); } } Ok(vm.ary_new(out)) }),
        ("*", |vm, s, a, _b| { argc!(vm, a, 1); match a[0] { Value::Int(n) => { if n < 0 { return Err(vm.raise_arg("negative argument")); } let v = items(vm, s).repeat(n as usize); Ok(vm.ary_new(v)) } v => { let sep = vm.expect_str(v, "argument")?; ary_join(vm, s, &sep) } } }),
        ("&", |vm, s, a, _b| { argc!(vm, a, 1); let v = items(vm, s); let o = items(vm, a[0]); let mut out: Vec<Value> = vec![]; for it in v { let mut hit = false; for x in &o { if vm.equal(it, *x)? { hit = true; break; } } if hit { let mut dup = false; for x in &out { if vm.equal(it, *x)? { dup = true; break; } } if !dup { out.push(it); } } } Ok(vm.ary_new(out)) }),
        ("|", |vm, s, a, _b| { argc!(vm, a, 1); let mut v = items(vm, s); v.extend(items(vm, a[0])); let mut out: Vec<Value> = vec![]; for it in v { let mut dup = false; for x in &out { if vm.equal(it, *x)? { dup = true; break; } } if !dup { out.push(it); } } Ok(vm.ary_new(out)) }),
        ("==", ary_eq),
        ("eql?", |vm, s, a, _b| { argc!(vm, a, 1); if s == a[0] { return Ok(Value::True); } let (x, y) = (items(vm, s), match vm.ary_vals(a[0]) { Some(y) => y, None => return Ok(Value::False) }); if x.len() != y.len() { return Ok(Value::False); } let pair = (s.obj().unwrap(), a[0].obj().unwrap()); if vm.eq_guard.contains(&pair) { return Ok(Value::True); } vm.eq_guard.push(pair); let eql = vm.s.eql; let mut res = Ok(Value::True); for (p, q) in x.iter().zip(y.iter()) { match vm.funcall(*p, eql, &[*q], Value::Nil) { Ok(t) if t.truthy() => {} Ok(_) => { res = Ok(Value::False); break; } Err(e) => { res = Err(e); break; } } } vm.eq_guard.pop(); res }),
        ("<=>", ary_cmp),
        ("__ary_eq", ary_eq),
        ("__ary_cmp", ary_cmp),
        ("__ary_index", |vm, s, a, _b| { argc!(vm, a, 1); let mut i = 0; loop { let it = match slots(vm, s).get(i).map(|x| x.get()) { Some(v) => v, None => break }; if vm.equal(it, a[0])? { return Ok(Value::Int(i as i64)); } i += 1; } Ok(Value::Nil) }),
        ("__svalue", |vm, s, _a, _b| { let l = slots(vm, s); Ok(if l.len() == 1 { l[0].get() } else if l.is_empty() { Value::Nil } else { s }) }),
        ("hash", |vm, s, _a, _b| { let o = s.obj().unwrap(); if vm.inspect_guard.contains(&o) { return Ok(Value::Int(0)); } vm.inspect_guard.push(o); let list = items(vm, s); let mut h: i64 = list.len() as i64; let hs = vm.s.hash; let mut err = None; for it in list { match vm.funcall(it, hs, &[], Value::Nil) { Ok(Value::Int(i)) => h = h.wrapping_mul(31).wrapping_add(i), Ok(_) => {} Err(e) => { err = Some(e); break; } } } vm.inspect_guard.pop(); if let Some(e) = err { return Err(e); } Ok(Value::Int(h)) }),
        ("inspect", |vm, s, _a, _b| { let b = ary_inspect(vm, s)?; Ok(vm.str_new(&b)) }),
        ("to_s", |vm, s, _a, _b| { let b = ary_inspect(vm, s)?; Ok(vm.str_new(&b)) }),
        ("to_a", |_vm, s, _a, _b| Ok(s)),
        ("entries", |_vm, s, _a, _b| Ok(s)),
        ("to_ary", |_vm, s, _a, _b| Ok(s)),
        // to_h / zip are mruby-array-ext (gem) methods, provided natively here.
        ("to_h", |vm, s, _a, b| { let h = vm.hash_new(); for it in items(vm, s) { let it = if b.is_nil() { it } else { vm.call_block(b, &[it])? }; match vm.ary_vals(it) { Some(p) if p.len() == 2 => vm.hash_set(h, p[0], p[1])?, _ => { let d = vm.describe_for_error(it); return Err(vm.raise_type(&format!("wrong element type {d} (expected array)"))); } } } Ok(h) }),
        ("join", |vm, s, a, _b| { argc!(vm, a, 0, 1); let sep = match a.first() { Some(v) if !v.is_nil() => vm.expect_str(*v, "separator")?, _ => vec![] }; ary_join(vm, s, &sep) }),
        ("reverse", |vm, s, _a, _b| { let mut v = items(vm, s); v.reverse(); Ok(vm.ary_new(v)) }),
        ("reverse!", |vm, s, _a, _b| { with_mut(vm, s, |arr| arr.reverse())?; Ok(s) }),
        ("rotate", |vm, s, a, _b| { argc!(vm, a, 0, 1); let n = if a.is_empty() { 1 } else { vm.expect_int(a[0], "count")? }; let mut v = items(vm, s); if !v.is_empty() { let k = n.rem_euclid(v.len() as i64) as usize; v.rotate_left(k); } Ok(vm.ary_new(v)) }),
        // with a block and called by a SEND, the loop is the Ruby one in `block-frames.rb`, so
        // the block runs in an ordinary frame (`docs/design/wait-anywhere.md`)
        ("index", |vm, s, a, b| { if a.is_empty() && !b.is_nil() && vm.in_frame() { let m = vm.intern("__index_by"); return vm.send_in_frame(s, m, &[], None, b); } let mut i = 0; loop { let it = match vm.ary(s).and_then(|v| v.get(i).map(|x| x.get())) { Some(v) => v, None => return Ok(Value::Nil) }; let hit = if let Some(x) = a.first() { vm.equal(it, *x)? } else { vm.call_block(b, &[it])?.truthy() }; if hit { return Ok(Value::Int(i as i64)); } i += 1; } }),
        ("rindex", |vm, s, a, b| {
            if a.is_empty() && !b.is_nil() && vm.in_frame() { let m = vm.intern("__rindex_by"); return vm.send_in_frame(s, m, &[], None, b); }
            // the array is re-read every step: `==` or the block may shrink or replace it
            let mut i = vm.ary(s).map(|v| v.len()).unwrap_or(0);
            while i > 0 {
                i -= 1;
                let len = vm.ary(s).map(|v| v.len()).unwrap_or(0);
                if i >= len { i = len; continue; }
                let it = vm.ary(s).map(|v| v[i].get()).unwrap_or(Value::Nil);
                let hit = if let Some(x) = a.first() { vm.equal(it, *x)? } else { vm.call_block(b, &[it])?.truthy() };
                if hit { return Ok(Value::Int(i as i64)); }
            }
            Ok(Value::Nil)
        }),
        // as `index` above: one element at a time, without copying the array out
        ("include?", |vm, s, a, _b| { argc!(vm, a, 1); let mut i = 0; loop { let it = match vm.ary(s).and_then(|l| l.get(i).map(|x| x.get())) { Some(v) => v, None => break }; if vm.equal(it, a[0])? { return Ok(Value::True); } i += 1; } Ok(Value::False) }),
        ("member?", |vm, s, a, _b| { argc!(vm, a, 1); let mut i = 0; loop { let it = match vm.ary(s).and_then(|l| l.get(i).map(|x| x.get())) { Some(v) => v, None => break }; if vm.equal(it, a[0])? { return Ok(Value::True); } i += 1; } Ok(Value::False) }),
        ("clear", |vm, s, _a, _b| { with_mut(vm, s, |arr| arr.clear())?; Ok(s) }),
        ("delete_at", |vm, s, a, _b| { argc!(vm, a, 1); let i = vm.expect_int(a[0], "index")?; with_mut(vm, s, |arr| { let i = if i < 0 { i + arr.len() as i64 } else { i }; if i < 0 || i as usize >= arr.len() { Value::Nil } else { arr.remove(i as usize).get() } }) }),
        ("delete", |vm, s, a, b| { argc!(vm, a, 1); let list = items(vm, s); let mut keep = vec![]; let mut found = None; for it in list { if vm.equal(it, a[0])? { found = Some(it); } else { keep.push(it); } } with_mut(vm, s, |arr| *arr = slots_of(&keep).into())?; match found { Some(v) => Ok(v), None => if b.is_nil() { Ok(Value::Nil) } else { vm.exec_block(b, &[a[0]]) } } }),
        ("delete_if", |vm, s, _a, b| { let list = items(vm, s); let mut keep = vec![]; for it in list { if !vm.call_block(b, &[it])?.truthy() { keep.push(it); } } with_mut(vm, s, |arr| *arr = slots_of(&keep).into())?; Ok(s) }),
        ("reject!", |vm, s, _a, b| { let list = items(vm, s); let n = list.len(); let mut keep = vec![]; for it in list { if !vm.call_block(b, &[it])?.truthy() { keep.push(it); } } let changed = keep.len() != n; with_mut(vm, s, |arr| *arr = slots_of(&keep).into())?; Ok(if changed { s } else { Value::Nil }) }),
        ("select!", |vm, s, _a, b| { let list = items(vm, s); let n = list.len(); let mut keep = vec![]; for it in list { if vm.call_block(b, &[it])?.truthy() { keep.push(it); } } let changed = keep.len() != n; with_mut(vm, s, |arr| *arr = slots_of(&keep).into())?; Ok(if changed { s } else { Value::Nil }) }),
        ("keep_if", |vm, s, _a, b| { let list = items(vm, s); let mut keep = vec![]; for it in list { if vm.call_block(b, &[it])?.truthy() { keep.push(it); } } with_mut(vm, s, |arr| *arr = slots_of(&keep).into())?; Ok(s) }),
        ("compact", |vm, s, _a, _b| { let v: Vec<Value> = items(vm, s).into_iter().filter(|x| !x.is_nil()).collect(); Ok(vm.ary_new(v)) }),
        ("compact!", |vm, s, _a, _b| { let n = ary_len(vm, s); let v: Vec<Value> = items(vm, s).into_iter().filter(|x| !x.is_nil()).collect(); let changed = v.len() != n; with_mut(vm, s, |arr| *arr = slots_of(&v).into())?; Ok(if changed { s } else { Value::Nil }) }),
        ("flatten", |vm, s, a, _b| { argc!(vm, a, 0, 1); let d = if a.is_empty() { -1 } else { vm.expect_int(a[0], "depth")? }; let mut out = vec![]; flatten(vm, s, d, &mut out); Ok(vm.ary_new(out)) }),
        ("flatten!", |vm, s, a, _b| { argc!(vm, a, 0, 1); let d = if a.is_empty() { -1 } else { vm.expect_int(a[0], "depth")? }; let mut out = vec![]; flatten(vm, s, d, &mut out); with_mut(vm, s, |arr| *arr = slots_of(&out).into())?; Ok(s) }),
        ("uniq", |vm, s, _a, _b| { let list = items(vm, s); let mut out: Vec<Value> = vec![]; for it in list { let mut dup = false; for x in &out { if vm.eql(it, *x) || vm.equal(it, *x)? { dup = true; break; } } if !dup { out.push(it); } } Ok(vm.ary_new(out)) }),
        ("uniq!", |vm, s, _a, _b| { let list = items(vm, s); let n = list.len(); let mut out: Vec<Value> = vec![]; for it in list { let mut dup = false; for x in &out { if vm.eql(it, *x) || vm.equal(it, *x)? { dup = true; break; } } if !dup { out.push(it); } } let changed = out.len() != n; with_mut(vm, s, |arr| *arr = slots_of(&out).into())?; Ok(if changed { s } else { Value::Nil }) }),
        ("sort", |vm, s, _a, b| { let mut v = items(vm, s); sort_values(vm, &mut v, b)?; Ok(vm.ary_new(v)) }),
        ("sort!", |vm, s, _a, b| { if !b.is_nil() && vm.in_frame() { let m = vm.intern("__sort_by_block!"); return vm.send_in_frame(s, m, &[], None, b); } let mut v = items(vm, s); sort_values(vm, &mut v, b)?; with_mut(vm, s, |arr| *arr = slots_of(&v).into())?; Ok(s) }),
        ("sort_by", |vm, s, _a, b| { let v = items(vm, s); let mut keyed: Vec<(Value, Value)> = vec![]; for it in v { keyed.push((vm.call_block(b, &[it])?, it)); } let mut keys: Vec<Value> = keyed.iter().map(|(k, _)| *k).collect(); let idx: Vec<usize> = (0..keys.len()).collect(); let mut order: Vec<Value> = idx.iter().map(|i| Value::Int(*i as i64)).collect(); let _ = &mut keys; let cmp = vm.intern("<=>"); let mut err = None; order.sort_by(|x, y| { if err.is_some() { return core::cmp::Ordering::Equal; } let (i, j) = (match x { Value::Int(i) => *i as usize, _ => 0 }, match y { Value::Int(j) => *j as usize, _ => 0 }); match vm.funcall(keyed[i].0, cmp, &[keyed[j].0], Value::Nil) { Ok(Value::Int(r)) => r.cmp(&0), Ok(_) => { err = Some(vm.raise_arg("comparison failed")); core::cmp::Ordering::Equal } Err(e) => { err = Some(e); core::cmp::Ordering::Equal } } }); if let Some(e) = err { return Err(e); } let out: Vec<Value> = order.iter().map(|x| match x { Value::Int(i) => keyed[*i as usize].1, _ => Value::Nil }).collect(); Ok(vm.ary_new(out)) }),
        ("sum", |vm, s, a, _b| { let v = items(vm, s); let mut acc = a.first().copied().unwrap_or(Value::Int(0)); let plus = vm.s.plus; for it in v { acc = vm.funcall(acc, plus, &[it], Value::Nil)?; } Ok(acc) }),
        ("take", |vm, s, a, _b| { argc!(vm, a, 1); let n = vm.expect_int(a[0], "argument")?; if n < 0 { return Err(vm.raise_arg("attempt to take negative size")); } let v: Vec<Value> = slots(vm, s).iter().take(n as usize).map(|x| x.get()).collect(); Ok(vm.ary_new(v)) }),
        ("drop", |vm, s, a, _b| { argc!(vm, a, 1); let n = vm.expect_int(a[0], "argument")?; if n < 0 { return Err(vm.raise_arg("attempt to drop negative size")); } let v: Vec<Value> = slots(vm, s).iter().skip(n as usize).map(|x| x.get()).collect(); Ok(vm.ary_new(v)) }),
        ("slice!", |vm, s, a, _b| { let len = ary_len(vm, s); let r = ary_aref(vm, s, a, Value::Nil)?; if r.is_nil() { return Ok(r); } match super::string::index_args(vm, len, a)? { Some((i, n)) => { with_mut(vm, s, |arr| { arr.drain_range(i, i + n); })?; Ok(r) } None => Ok(Value::Nil) } }),
        ("fill", |vm, s, a, _b| { argc!(vm, a, 1, 3); let v = a[0]; with_mut(vm, s, |arr| for x in arr.iter_mut() { *x = Slot::from(v); })?; Ok(s) }),
        ("assoc", |vm, s, a, _b| { argc!(vm, a, 1); let mut i = 0; loop { let it = match slots(vm, s).get(i).map(|x| x.get()) { Some(v) => v, None => break }; if let Some(k) = vm.ary(it).and_then(|p| p.first().map(|x| x.get())) { if vm.equal(k, a[0])? { return Ok(it); } } i += 1; } Ok(Value::Nil) }),
        ("rassoc", |vm, s, a, _b| { argc!(vm, a, 1); let mut i = 0; loop { let it = match slots(vm, s).get(i).map(|x| x.get()) { Some(v) => v, None => break }; if let Some(k) = vm.ary(it).and_then(|p| p.get(1).map(|x| x.get())) { if vm.equal(k, a[0])? { return Ok(it); } } i += 1; } Ok(Value::Nil) }),
        ("values_at", |vm, s, a, _b| { let len = ary_len(vm, s) as i64; let mut out = vec![]; for i in a { let i = vm.expect_int(*i, "index")?; let i = if i < 0 { i + len } else { i }; out.push(if i < 0 { Value::Nil } else { slots(vm, s).get(i as usize).map(|x| x.get()).unwrap_or(Value::Nil) }); } Ok(vm.ary_new(out)) }),
        ("transpose", |vm, s, _a, _b| { let rows = items(vm, s); if rows.is_empty() { return Ok(vm.ary_new(vec![])); } let cols: Vec<Vec<Value>> = rows.iter().map(|r| items(vm, *r)).collect(); let n = cols[0].len(); if cols.iter().any(|c| c.len() != n) { return Err(vm.raise(vm.core.index_error, "element size differs")); } let mut out = vec![]; for j in 0..n { let col: Vec<Value> = cols.iter().map(|c| c[j]).collect(); out.push(vm.ary_new(col)); } Ok(vm.ary_new(out)) }),
        ("product", |vm, s, a, _b| { let mut lists = vec![items(vm, s)]; for x in a { lists.push(items(vm, *x)); } let mut out: Vec<Vec<Value>> = vec![vec![]]; for l in lists { let mut next = vec![]; for prefix in &out { for it in &l { let mut p = prefix.clone(); p.push(*it); next.push(p); } } out = next; } let items_: Vec<Value> = out.into_iter().map(|p| vm.ary_new(p)).collect(); Ok(vm.ary_new(items_)) }),
        ("freeze", |vm, s, _a, _b| { if let Some(o) = s.obj() { vm.heap.get_mut(o).frozen = true; } Ok(s) }),
        ("frozen?", |vm, s, _a, _b| Ok(Value::bool(s.obj().map(|o| vm.heap.get(o).frozen).unwrap_or(true)))),
        ("dup", |vm, s, _a, _b| { let v: Vec<Slot> = slots(vm, s).to_vec(); let c = vm.real_class_of(s); Ok(Value::Obj(vm.heap.alloc(c, ObjKind::Array(v.into())))) }),
    ]);
    // `MRB_MT_PRIVATE` in the reference's ROM table for this class (src/array.c)
    vm.mark_private(c.array, &["initialize", "initialize_copy"]);
    vm.define_private_method(c.array, "__sort_cmp", sort_cmp);
}

fn ary_join(vm: &mut Vm, s: Value, sep: &[u8]) -> VmResult<Value> {
    fn rec(vm: &mut Vm, v: Value, sep: &[u8], out: &mut Vec<u8>, first: &mut bool, root: Value) -> VmResult<()> {
        for it in items(vm, v) {
            if it == root { return Err(vm.raise_arg("recursive array join")); }
            if vm.ary(it).is_some() { rec(vm, it, sep, out, first, root)?; continue; }
            if !*first { out.extend_from_slice(sep); }
            *first = false;
            out.extend(vm.as_string(it)?);
        }
        Ok(())
    }
    let mut out = vec![];
    let mut first = true;
    rec(vm, s, sep, &mut out, &mut first, s)?;
    Ok(vm.str_new(&out))
}

fn ary_aref(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1, 2);
    let a: Vec<Value> = a.iter().map(|v| match v { Value::Float(f) => Value::Int(*f as i64), v => *v }).collect();
    let a = &a[..];
    if let [Value::Int(i)] = a {
        // hot path (`each` in mrblib indexes with `self[idx]`): no copy of the array
        let list = match vm.ary(s) { Some(l) => l, None => return Ok(Value::Nil) };
        let i = if *i < 0 { *i + list.len() as i64 } else { *i };
        return Ok(if i < 0 { Value::Nil } else { list.get(i as usize).map(|x| x.get()).unwrap_or(Value::Nil) });
    }
    // `index_args` only reads Integers and a Range of Integers (a Bignum there raises), so it
    // cannot run Ruby and cannot have moved the array under us: the borrow below is safe.
    let len = ary_len(vm, s);
    match super::string::index_args(vm, len, a)? {
        Some((i, n)) => { let v: Vec<Value> = slots(vm, s)[i..i + n].iter().map(|x| x.get()).collect(); Ok(vm.ary_new(v)) }
        None => Ok(Value::Nil),
    }
}

/// `Array#[]=`. `pub(crate)` because `OP_SETIDX` records it as the implementation it stands
/// in for and calls it directly for an Integer index (`Vm::op_setidx`).
pub(crate) fn ary_aset(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 2, 3);
    let val = a[a.len() - 1];
    let len = ary_len(vm, s);
    if a.len() == 3 { let n = vm.expect_int(a[1], "length")?; if n < 0 { return Err(vm.raise(vm.core.index_error, &format!("negative length ({n})"))); } }
    if let [Value::Int(i), _] = a {
        let i = if *i < 0 { *i + len as i64 } else { *i };
        if i < 0 { return Err(vm.raise(vm.core.index_error, &format!("index {} too small for array; minimum: -{}", i - len as i64, len))); }
        with_mut(vm, s, |arr| { let i = i as usize; if arr.len() <= i { arr.resize(i + 1, Slot::NIL); } arr[i] = Slot::from(val); })?;
        return Ok(val);
    }
    let (i, n) = match super::string::index_args(vm, len, &a[..a.len() - 1])? {
        Some(x) => x,
        None => {
            // start beyond the end: pad with nil
            match a[0] { Value::Int(i) if i >= 0 => (i as usize, 0), _ => return Err(vm.raise(vm.core.index_error, "index out of array")) }
        }
    };
    let rep = match vm.ary_vals(val) { Some(v) => v, None => vec![val] };
    with_mut(vm, s, |arr| { if arr.len() < i { arr.resize(i, Slot::NIL); } let end = (i + n).min(arr.len()); arr.splice_range(i, end, slots_of(&rep)); })?;
    Ok(val)
}
