//! mruby-struct (`mrbgems/mruby-struct/src/struct.c`): `Struct`. Its Ruby part (`each`,
//! `each_pair`, `select`, `dig`, `to_s`, `__struct_init_fwd`) is `src/mrblib/struct.mrb`.
//!
//! A struct instance is the reference's `MRB_TT_STRUCT`, an array-shaped object whose class
//! is the struct class: here an `ObjKind::Array` with that class. The member accessors are
//! one native each way that reads the name it was called by (`Vm::native_mid`) instead of
//! the reference's per-member C procs with an index in their environment.

use alloc::{format, vec, vec::Vec};

use crate::argc;
use crate::error::{VmError, VmResult};
use crate::object::{ClassData, Method, ObjKind};
use crate::symbol::Sym;
use crate::value::{ObjId, Slot, Value};
use crate::vm::Vm;

use super::object::{same_object, sym_arg};

fn struct_class(vm: &mut Vm) -> ObjId {
    let n = vm.intern("Struct");
    match vm.const_get(vm.core.object, n) { Some(Value::Obj(c)) => c, _ => vm.core.object }
}

pub(crate) fn corrupted(vm: &mut Vm, what: &str) -> VmError {
    vm.raise_type(&format!("corrupted {what}"))
}

/// `struct_s_members`: the `__members__` of the nearest class that has them.
pub(crate) fn s_members(vm: &mut Vm, mut c: ObjId, root: ObjId, what: &str) -> VmResult<Vec<Sym>> {
    let key = vm.intern("__members__");
    loop {
        match vm.heap.ivar_get(c, key) {
            Value::Nil => {}
            v => {
                let vals = match vm.ary_vals(v) { Some(x) => x, None => return Err(corrupted(vm, what)) };
                let mut out = Vec::with_capacity(vals.len());
                for x in vals { match x { Value::Sym(s) => out.push(s), _ => return Err(corrupted(vm, what)) } }
                return Ok(out);
            }
        }
        // the singleton chain and include classes are skipped: the ivar sits on the class
        c = match vm.heap.class(c).superclass { Some(s) if s != root => s, _ => return Err(vm.raise_type(&format!("uninitialized {what}"))) };
    }
}

pub(crate) fn values(vm: &Vm, s: Value) -> Vec<Value> {
    vm.ary_vals(s).unwrap_or_default()
}

pub(crate) fn is_struct(vm: &Vm, s: Value) -> bool {
    matches!(s, Value::Obj(o) if matches!(vm.heap.get(o).kind, ObjKind::Array(_)))
}

/// `struct_members`: the members of the instance's class, sized like the instance.
fn members(vm: &mut Vm, s: Value) -> VmResult<Vec<Sym>> {
    if !is_struct(vm, s) { return Err(corrupted(vm, "struct")); }
    let root = struct_class(vm);
    let c = vm.real_class_of(s);
    let m = s_members(vm, c, root, "struct")?;
    let len = values(vm, s).len();
    if len > 0 && len != m.len() { return Err(vm.raise_type(&format!("struct size differs ({} required {} given)", m.len(), len))); }
    Ok(m)
}

/// `mrb_ary_set` on the instance: frozen check, growth with nil.
pub(crate) fn ary_set(vm: &mut Vm, s: Value, i: usize, v: Value) -> VmResult<()> {
    let o = s.obj().unwrap();
    if vm.heap.get(o).frozen { return Err(vm.frozen_error(s)); }
    if let ObjKind::Array(a) = &mut vm.heap.get_mut(o).kind {
        if a.len() <= i { a.resize(i + 1, Slot::NIL); }
        a[i] = Slot::from(v);
    }
    Ok(())
}

pub(crate) fn ary_replace(vm: &mut Vm, dst: Value, src: Value) -> VmResult<()> {
    let vals: Vec<Slot> = values(vm, src).into_iter().map(Slot::from).collect();
    let o = dst.obj().unwrap();
    if vm.heap.get(o).frozen { return Err(vm.frozen_error(dst)); }
    if let ObjKind::Array(a) = &mut vm.heap.get_mut(o).kind { *a = vals.into(); }
    Ok(())
}

fn keyword_init(vm: &mut Vm, mut c: ObjId) -> Value {
    let root = struct_class(vm);
    let key = vm.intern("@__keyword_init__");
    while c != root {
        let v = vm.heap.ivar_get(c, key);
        if !v.is_nil() { return v; }
        c = match vm.heap.class(c).superclass { Some(s) => s, None => break };
    }
    Value::Nil
}

fn s_keyword_init_p(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    Ok(keyword_init(vm, s.obj().unwrap()))
}

fn s_members_m(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let root = struct_class(vm);
    let m = s_members(vm, s.obj().unwrap(), root, "struct")?;
    Ok(vm.ary_new(m.into_iter().map(Value::Sym).collect()))
}

fn m_members(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let c = vm.real_class_of(s);
    s_members_m(vm, Value::Obj(c), &[], Value::Nil)
}

/// the index of the member the accessor was called as (`name` or `name=`)
fn member_index(vm: &mut Vm, s: Value, setter: bool) -> VmResult<usize> {
    let mid = vm.native_mid.ok_or_else(|| corrupted(vm, "struct"))?;
    let mut name = vm.sym_name(mid);
    if setter { name.pop(); }
    let m = members(vm, s)?;
    let want = vm.intern(&name);
    m.iter().position(|x| *x == want).ok_or_else(|| corrupted(vm, "struct"))
}

fn struct_ref(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    if !a.is_empty() { return Err(vm.argnum_error(a.len(), "0")); }
    let i = member_index(vm, s, false)?;
    Ok(values(vm, s).get(i).copied().unwrap_or(Value::Nil))
}

fn struct_set(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    let i = member_index(vm, s, true)?;
    ary_set(vm, s, i, a[0])?;
    Ok(a[0])
}

pub(crate) fn define_accessors(vm: &mut Vm, c: ObjId, members: &[Sym], writers: bool) {
    for m in members {
        let name = vm.sym_name(*m);
        vm.define_method(c, &name, struct_ref);
        if writers { vm.define_method(c, &format!("{name}="), struct_set); }
    }
}

/// `mrb_const_name_p`
fn const_name_p(name: &[u8]) -> bool {
    match name.first() {
        Some(c) if c.is_ascii_uppercase() => name[1..].iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c >= 0x80),
        _ => false,
    }
}

/// `mrb_class_new(klass)`: an anonymous subclass
pub(crate) fn class_new(vm: &mut Vm, sup: ObjId) -> VmResult<ObjId> {
    let c = vm.heap.alloc(vm.core.class, ObjKind::Class(ClassData { superclass: Some(sup), ..Default::default() }));
    vm.singleton_class(Value::Obj(c))?;
    Ok(c)
}

fn make_struct(vm: &mut Vm, name: Value, members: &[Sym], klass: ObjId) -> VmResult<ObjId> {
    let c = if name.is_nil() {
        class_new(vm, klass)?
    } else {
        let bytes = match vm.str_bytes(name) { Some(b) => b.to_vec(), None => { let d = vm.describe_for_type_error(name); return Err(vm.raise_type(&format!("{d} cannot be converted to String"))); } };
        let id = vm.intern(&alloc::string::String::from_utf8_lossy(&bytes));
        if !const_name_p(&bytes) {
            let d = vm.inspect_str(name)?;
            return Err(vm.name_error(id, &format!("identifier {d} needs to be constant")));
        }
        // the reference warns "redefining constant Struct::<name>" and removes it
        vm.heap.class_mut(klass).consts.remove(&id);
        let c = vm.heap.alloc(vm.core.class, ObjKind::Class(ClassData { name: Some(id), superclass: Some(klass), outer: Some(klass), ..Default::default() }));
        vm.singleton_class(Value::Obj(c))?;
        vm.heap.class_mut(klass).consts.insert(id, Slot::from(Value::Obj(c)));
        c
    };
    let key = vm.intern("__members__");
    let ary = vm.ary_new(members.iter().map(|m| Value::Sym(*m)).collect());
    vm.heap.ivar_set(c, key, ary);
    let sc = vm.singleton_class(Value::Obj(c))?;
    vm.define_methods(sc, &[("new", struct_new), ("[]", struct_new), ("members", s_members_m), ("keyword_init?", s_keyword_init_p)]);
    define_accessors(vm, c, members, true);
    Ok(c)
}

/// `Struct.new(name = nil, *members, keyword_init: nil) { block }`: a new struct class
fn s_def(vm: &mut Vm, s: Value, a: &[Value], b: Value) -> VmResult<Value> {
    let klass = s.obj().unwrap();
    let mut args = a.to_vec();
    let mut keyword_init_val = Value::Nil;
    // a trailing Hash with `keyword_init:` is the option, keyword or not
    if let Some(last) = args.last().copied() {
        if matches!(last.obj().map(|o| &vm.heap.get(o).kind), Some(ObjKind::Hash(_))) {
            let k = vm.intern("keyword_init");
            if let Some(v) = vm.hash_get(last, Value::Sym(k)) {
                keyword_init_val = if v.is_nil() { Value::Nil } else { Value::bool(v.truthy()) };
                args.pop();
            }
        }
    }
    let mut name = Value::Nil;
    let mut rest: &[Value] = &args;
    if !rest.is_empty() && !matches!(rest[0], Value::Sym(_)) {
        name = rest[0];
        rest = &rest[1..];
    }
    let mut members: Vec<Sym> = Vec::with_capacity(rest.len());
    for v in rest { members.push(sym_arg(vm, *v)?); }
    for i in 0..members.len() {
        for j in i + 1..members.len() {
            if members[i] == members[j] { let n = vm.sym_name(members[i]); return Err(vm.raise_arg(&format!("duplicate member: {n}"))); }
        }
    }
    let st = make_struct(vm, name, &members, klass)?;
    let kk = vm.intern("@__keyword_init__");
    vm.heap.ivar_set(st, kk, keyword_init_val);
    // called by a SEND the block runs in a frame of its own
    if !b.is_nil() { return vm.exec_block_with_self_then(b, Value::Obj(st), &[Value::Obj(st)], Value::Obj(st)); }
    Ok(Value::Obj(st))
}

fn init_with_args(vm: &mut Vm, s: Value, argv: &[Value]) -> VmResult<Value> {
    let n = members(vm, s)?.len();
    if n < argv.len() { return Err(vm.raise_arg("struct size differs")); }
    for (i, v) in argv.iter().enumerate() { ary_set(vm, s, i, *v)?; }
    for i in argv.len()..n { ary_set(vm, s, i, Value::Nil)?; }
    Ok(s)
}

fn init_with_keywords(vm: &mut Vm, s: Value, hash: Value) -> VmResult<Value> {
    let m = members(vm, s)?;
    for (i, mem) in m.iter().enumerate() {
        let v = vm.hash_get(hash, Value::Sym(*mem)).unwrap_or(Value::Nil);
        ary_set(vm, s, i, v)?;
    }
    let keys: Vec<Value> = match hash.obj().map(|o| &vm.heap.get(o).kind) { Some(ObjKind::Hash(hd)) => hd.entries().iter().map(|(k, _)| k.get()).collect(), _ => Vec::new() };
    let mut invalid: Vec<alloc::string::String> = Vec::new();
    for k in keys {
        if !matches!(k, Value::Sym(x) if m.contains(&x)) { let b = vm.as_string(k)?; invalid.push(alloc::string::String::from_utf8_lossy(&b).into_owned()); }
    }
    if !invalid.is_empty() { return Err(vm.raise_arg(&format!("unknown keywords: {}", invalid.join(", ")))); }
    Ok(s)
}

/// `struct_init_body`: fill `s` from the constructor's arguments
fn init_body(vm: &mut Vm, s: Value, a: &[Value]) -> VmResult<Value> {
    // keywords were given when the trailing Hash is the pending kdict and not empty
    let kw_given = match (vm.pending_kw, a.last()) { (Some(k), Some(l)) if !k.is_nil() && k == *l => matches!(k.obj().map(|o| &vm.heap.get(o).kind), Some(ObjKind::Hash(hd)) if !hd.is_empty()), _ => false };
    let c = vm.real_class_of(s);
    let ki = keyword_init(vm, c);
    let is_hash = |vm: &Vm, v: Value| matches!(v.obj().map(|o| &vm.heap.get(o).kind), Some(ObjKind::Hash(_)));
    if ki.truthy() {
        if a.len() > 1 || (a.len() == 1 && !is_hash(vm, a[0])) { return Err(vm.argnum_error(a.len(), "0")); }
        let hash = if a.len() == 1 { a[0] } else { vm.hash_new() };
        return init_with_keywords(vm, s, hash);
    }
    if ki.is_nil() && kw_given && a.len() == 1 && is_hash(vm, a[0]) {
        return init_with_keywords(vm, s, a[0]);
    }
    init_with_args(vm, s, a)
}

fn struct_initialize(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    init_body(vm, s, a)
}

/// `mrb_func_basic_p(obj, :initialize, f)`
pub(crate) fn initialize_is(vm: &Vm, class: ObjId, f: crate::object::NativeFn) -> bool {
    let init = vm.s.initialize;
    matches!(vm.find_method(class, init), Some((Method::Native(g), _)) if core::ptr::fn_addr_eq(g, f))
}

/// the singleton `new`/`[]` of a struct class
fn struct_new(vm: &mut Vm, s: Value, a: &[Value], b: Value) -> VmResult<Value> {
    let klass = s.obj().unwrap();
    let obj = Value::Obj(vm.heap.alloc(klass, ObjKind::Array(Default::default())));
    if !initialize_is(vm, klass, struct_initialize) {
        // an overridden initialize takes the arguments; keywords go through the Ruby bridge
        let kw = match (vm.pending_kw, a.last()) { (Some(k), Some(l)) if !k.is_nil() && k == *l => Some(k), _ => None };
        match kw {
            Some(k) => {
                let pos = vm.ary_new(a[..a.len() - 1].to_vec());
                let fwd = vm.intern("__struct_init_fwd");
                vm.funcall(obj, fwd, &[pos, k], b)?;
            }
            None => {
                let init = vm.s.initialize;
                vm.funcall(obj, init, a, b)?;
            }
        }
        return Ok(obj);
    }
    init_body(vm, obj, a)?;
    Ok(obj)
}

fn init_copy(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    if same_object(s, a[0]) { return Ok(s); }
    if vm.real_class_of(a[0]) != vm.real_class_of(s) { return Err(vm.raise_type("wrong argument class")); }
    if !is_struct(vm, a[0]) { return Err(corrupted(vm, "struct")); }
    ary_replace(vm, s, a[0])?;
    Ok(s)
}

fn aref_sym(vm: &mut Vm, s: Value, id: Sym) -> VmResult<Value> {
    let m = members(vm, s)?;
    match m.iter().position(|x| *x == id) {
        Some(i) => Ok(values(vm, s).get(i).copied().unwrap_or(Value::Nil)),
        None => { let n = vm.sym_name(id); Err(vm.name_error(id, &format!("no member '{n}' in struct"))) }
    }
}

fn struct_index(vm: &mut Vm, i: i64, len: usize) -> VmResult<usize> {
    let idx = if i < 0 { len as i64 + i } else { i };
    if idx < 0 { return Err(vm.raise(vm.core.index_error, &format!("offset {i} too small for struct(size:{len})"))); }
    if idx >= len as i64 { return Err(vm.raise(vm.core.index_error, &format!("offset {i} too large for struct(size:{len})"))); }
    Ok(idx as usize)
}

fn aref_int(vm: &mut Vm, s: Value, i: i64) -> VmResult<Value> {
    let n = members(vm, s)?.len();
    let idx = struct_index(vm, i, n)?;
    Ok(values(vm, s).get(idx).copied().unwrap_or(Value::Nil))
}

fn key_sym(vm: &mut Vm, idx: Value) -> Option<Sym> {
    match idx {
        Value::Sym(s) => Some(s),
        v => vm.str_bytes(v).map(|b| b.to_vec()).map(|b| vm.intern(&alloc::string::String::from_utf8_lossy(&b))),
    }
}

fn struct_aref(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    if let Some(id) = key_sym(vm, a[0]) { return aref_sym(vm, s, id); }
    let i = vm.expect_int(a[0], "index")?;
    aref_int(vm, s, i)
}

fn struct_aset(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 2);
    if let Some(id) = key_sym(vm, a[0]) {
        let m = members(vm, s)?;
        return match m.iter().position(|x| *x == id) {
            Some(i) => { ary_set(vm, s, i, a[1])?; Ok(a[1]) }
            None => { let n = vm.sym_name(id); Err(vm.name_error(id, &format!("no member '{n}' in struct"))) }
        };
    }
    let i = vm.expect_int(a[0], "index")?;
    let n = members(vm, s)?.len();
    let idx = struct_index(vm, i, n)?;
    ary_set(vm, s, idx, a[1])?;
    Ok(a[1])
}

/// `==`/`eql?` member by member; a pair already being compared counts as equal, and a struct
/// whose storage changed shape under a member's comparison is no longer equal.
pub(crate) fn compare(vm: &mut Vm, s: Value, o: Value, eql: bool) -> VmResult<Value> {
    if same_object(s, o) { return Ok(Value::True); }
    if vm.real_class_of(s) != vm.real_class_of(o) { return Ok(Value::False); }
    let (x, y) = (s.obj().unwrap(), o.obj().unwrap());
    let len = values(vm, s).len();
    if len != values(vm, o).len() { return Ok(Value::False); }
    if vm.eq_guard.contains(&(x, y)) { return Ok(Value::True); }
    vm.eq_guard.push((x, y));
    let mut result = Ok(Value::True);
    let m = vm.intern(if eql { "eql?" } else { "==" });
    for i in 0..len {
        let (vs, vo) = (values(vm, s), values(vm, o));
        if vs.len() != len || vo.len() != len { result = Ok(Value::False); break; }
        let r = if eql { vm.funcall(vs[i], m, &[vo[i]], Value::Nil).map(|v| v.truthy()) } else { vm.equal(vs[i], vo[i]) };
        match r {
            Ok(true) => {}
            Ok(false) => { result = Ok(Value::False); break; }
            Err(e) => { result = Err(e); break; }
        }
    }
    vm.eq_guard.pop();
    result
}

/// `#<struct Name a=1, b=2>` (`#<struct ...>` inside its own inspect)
fn to_s(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let o = s.obj().unwrap();
    let mut out = alloc::string::String::from("#<struct ");
    let c = vm.real_class_of(s);
    if vm.heap.class(c).name.is_some() { out.push_str(&vm.class_name(c)); out.push(' '); }
    if vm.inspect_guard.contains(&o) { out.push_str("...>"); return Ok(vm.str_from(out)); }
    let m = members(vm, s)?;
    vm.inspect_guard.push(o);
    let mut r = Ok(());
    for (i, mem) in m.iter().enumerate() {
        if i > 0 { out.push_str(", "); }
        out.push_str(&vm.sym_name(*mem));
        out.push('=');
        let v = values(vm, s).get(i).copied().unwrap_or(Value::Nil);
        match vm.inspect_str(v) { Ok(t) => out.push_str(&t), Err(e) => { r = Err(e); break; } }
    }
    vm.inspect_guard.pop();
    r?;
    out.push('>');
    Ok(vm.str_from(out))
}

fn values_at(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    let len = values(vm, s).len() as i64;
    let mut out = Vec::new();
    for v in a {
        if matches!(v.obj().map(|o| &vm.heap.get(o).kind), Some(ObjKind::Range { .. })) {
            if let Some((beg, n)) = super::ext_array::range_beg_len(vm, *v, len, false)? {
                for j in 0..n { out.push(if beg + j < len { aref_int(vm, s, beg + j)? } else { Value::Nil }); }
            }
            continue;
        }
        let i = vm.expect_int(*v, "index")?;
        out.push(aref_int(vm, s, i)?);
    }
    Ok(vm.ary_new(out))
}

pub fn init(vm: &mut Vm) {
    let st = vm.define_class("Struct", vm.core.object);
    let sc = vm.singleton_class(Value::Obj(st)).expect("Struct singleton");
    vm.define_method(sc, "new", s_def);
    vm.define_methods(st, &[
        ("==", |vm, s, a, _b| { argc!(vm, a, 1); compare(vm, s, a[0], false) }),
        ("[]", struct_aref),
        ("[]=", struct_aset),
        ("members", m_members),
        ("initialize", struct_initialize),
        ("initialize_copy", init_copy),
        ("eql?", |vm, s, a, _b| { argc!(vm, a, 1); compare(vm, s, a[0], true) }),
        ("to_s", to_s),
        ("inspect", to_s),
        ("size", |vm, s, _a, _b| Ok(Value::Int(values(vm, s).len() as i64))),
        ("length", |vm, s, _a, _b| Ok(Value::Int(values(vm, s).len() as i64))),
        ("to_a", |vm, s, _a, _b| { let v = values(vm, s); Ok(vm.ary_new(v)) }),
        ("values", |vm, s, _a, _b| { let v = values(vm, s); Ok(vm.ary_new(v)) }),
        ("to_h", |vm, s, _a, _b| { let m = members(vm, s)?; let v = values(vm, s); let h = vm.hash_new(); for (i, mem) in m.iter().enumerate() { vm.hash_set(h, Value::Sym(*mem), v.get(i).copied().unwrap_or(Value::Nil))?; } Ok(h) }),
        ("values_at", values_at),
    ]);
    let ic = vm.intern("initialize_copy");
    let _ = vm.set_visibility(st, ic, crate::object::Vis::Private);
    let _ = vec![0u8];
}
