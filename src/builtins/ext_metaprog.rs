//! mruby-metaprog (`mrbgems/mruby-metaprog/src/metaprog.c`): the reflective
//! methods of Kernel and Module. Replaces the core-stage natives of the same
//! names with the reference's argument checks and listing rules.

use alloc::{format, vec, vec::Vec};

use crate::argc;
use crate::error::VmResult;
use crate::object::{Method, ObjKind, Vis};
use crate::symbol::Sym;
use crate::value::{ObjId, Slot, Value};
use crate::vm::Vm;

use super::object::sym_arg;

/// `mrb_ident_p`: letters, digits, `_` and any non-ASCII byte.
fn ident_p(s: &[u8]) -> bool { !s.is_empty() && s.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c >= 0x80) }

fn iv_name_p(name: &[u8]) -> bool { name.len() >= 2 && name[0] == b'@' && !name[1].is_ascii_digit() && ident_p(&name[1..]) }
fn cv_name_p(name: &[u8]) -> bool { name.len() > 2 && name[0] == b'@' && name[1] == b'@' && !name[2].is_ascii_digit() && ident_p(&name[2..]) }

fn iv_check(vm: &mut Vm, v: Value) -> VmResult<Sym> {
    let s = sym_arg(vm, v)?;
    if !iv_name_p(vm.syms.name(s)) { let n = vm.sym_name(s); return Err(vm.name_error(s, &format!("'{n}' is not allowed as an instance variable name"))); }
    Ok(s)
}
fn cv_check(vm: &mut Vm, v: Value) -> VmResult<Sym> {
    let s = sym_arg(vm, v)?;
    if !cv_name_p(vm.syms.name(s)) { let n = vm.sym_name(s); return Err(vm.name_error(s, &format!("'{n}' is not allowed as a class variable name"))); }
    Ok(s)
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Filt { NoPriv, Public, Private, Protected }

fn vis_ok(v: Vis, f: Filt) -> bool {
    match f { Filt::NoPriv => v != Vis::Private, Filt::Public => v == Vis::Public, Filt::Private => v == Vis::Private, Filt::Protected => v == Vis::Protected }
}

/// `method_entry_loop`: the table of `c` (its module's for an include class).
fn entry_loop(vm: &Vm, c: ObjId, seen: &mut Vec<(Sym, bool)>, f: Filt) {
    let owner = vm.table_owner(c);
    let t = vm.heap.class(owner);
    let mut names: Vec<&Sym> = t.methods.keys().collect();
    names.sort_by_key(|k| k.0);
    for k in names {
        let m = &t.methods[k];
        let vis = t.vis.get(k).copied().unwrap_or(Vis::Public);
        if !vis_ok(vis, f) { continue; }
        if seen.iter().any(|(s, _)| s == k) { continue; }
        seen.push((*k, !matches!(m, Method::Undef)));
    }
}

/// `mrb_class_instance_method_list`.
pub(crate) fn instance_method_list(vm: &Vm, recur: bool, klass: ObjId, f: Filt) -> Vec<Value> {
    let mut seen: Vec<(Sym, bool)> = vec![];
    if !recur {
        let c = vm.heap.class(klass).origin.unwrap_or(klass);
        entry_loop(vm, c, &mut seen, f);
    } else {
        let mut c = Some(klass);
        while let Some(x) = c { entry_loop(vm, x, &mut seen, f); c = vm.heap.class(x).superclass; }
    }
    seen.into_iter().filter(|(_, d)| *d).map(|(s, _)| Value::Sym(s)).collect()
}

fn regular_arg(a: &[Value]) -> bool { a.first().map(|v| v.truthy()).unwrap_or(true) }

fn is_iclass(vm: &Vm, c: ObjId) -> bool { let cd = vm.heap.class(c); cd.iclass_of.is_some() || cd.origin_of.is_some() }

fn const_at(vm: &Vm, c: ObjId, out: &mut Vec<Value>) {
    let owner = vm.heap.class(c).iclass_of.unwrap_or(c);
    let mut names: Vec<Sym> = vm.heap.class(owner).consts.keys().copied().collect();
    names.sort_by_key(|k| k.0);
    for k in names {
        let n = vm.syms.name(k);
        if n.first().map(|c| c.is_ascii_uppercase()).unwrap_or(false) && !out.iter().any(|v| *v == Value::Sym(k)) { out.push(Value::Sym(k)); }
    }
}

/// `mrb_mod_cv_defined`: any table on the chain.
fn cv_defined(vm: &Vm, c: ObjId, s: Sym) -> bool {
    let mut c = Some(c);
    while let Some(x) = c { let owner = vm.heap.class(x).iclass_of.unwrap_or(x); if vm.heap.class(owner).cvars.contains_key(&s) { return true; } c = vm.heap.class(x).superclass; }
    false
}

fn caller_proc(vm: &Vm) -> Option<ObjId> { vm.ci.last().map(|c| c.proc_) }

pub fn init(vm: &mut Vm) {
    let k = vm.core.kernel;
    vm.define_methods(k, &[
        ("instance_variable_defined?", |vm, s, a, _b| { argc!(vm, a, 1); let n = iv_check(vm, a[0])?; Ok(Value::bool(match s { Value::Obj(o) => vm.heap.get(o).ivars.iter().any(|(k, _)| *k == n), _ => false })) }),
        ("instance_variable_get", |vm, s, a, _b| { argc!(vm, a, 1); let n = iv_check(vm, a[0])?; Ok(match s { Value::Obj(o) => vm.heap.ivar_get(o, n), _ => Value::Nil }) }),
        ("instance_variable_set", |vm, s, a, _b| { argc!(vm, a, 2); let n = iv_check(vm, a[0])?; match s { Value::Obj(o) => { if vm.heap.get(o).frozen { return Err(vm.frozen_error(s)); } vm.heap.ivar_set(o, n, a[1]); Ok(a[1]) } _ => Err(vm.frozen_error(s)) } }),
        ("instance_variables", |vm, s, _a, _b| { let list: Vec<Value> = match s { Value::Obj(o) => vm.heap.get(o).ivars.iter().filter(|(k, _)| iv_name_p(vm.syms.name(*k))).map(|(k, _)| Value::Sym(*k)).collect(), _ => vec![] }; Ok(vm.ary_new(list)) }),
        ("methods", |vm, s, a, _b| { let c = vm.class_of(s); let l = instance_method_list(vm, regular_arg(a), c, Filt::NoPriv); Ok(vm.ary_new(l)) }),
        ("private_methods", |vm, s, a, _b| { let c = vm.class_of(s); let l = instance_method_list(vm, regular_arg(a), c, Filt::Private); Ok(vm.ary_new(l)) }),
        ("protected_methods", |vm, s, a, _b| { let c = vm.class_of(s); let l = instance_method_list(vm, regular_arg(a), c, Filt::Protected); Ok(vm.ary_new(l)) }),
        ("public_methods", |vm, s, a, _b| { let c = vm.class_of(s); let l = instance_method_list(vm, regular_arg(a), c, Filt::Public); Ok(vm.ary_new(l)) }),
        ("singleton_methods", |vm, s, a, _b| {
            let recur = regular_arg(a);
            let mut seen: Vec<(Sym, bool)> = vec![];
            let mut klass = Some(vm.class_of(s));
            if let Some(c) = klass { if vm.heap.class(c).is_singleton { entry_loop(vm, c, &mut seen, Filt::Public); klass = vm.heap.class(c).superclass; } }
            if recur { while let Some(c) = klass { if !(vm.heap.class(c).is_singleton || is_iclass(vm, c)) { break; } entry_loop(vm, c, &mut seen, Filt::Public); klass = vm.heap.class(c).superclass; } }
            let l: Vec<Value> = seen.into_iter().map(|(s, _)| Value::Sym(s)).collect();
            Ok(vm.ary_new(l))
        }),
        ("local_variables", |vm, _s, _a, _b| {
            // `mrb_proc_local_variables` of the caller's proc: its locals and its
            // blocks' up to the enclosing scope, without `*`/`&` placeholders
            let mut out: Vec<Value> = vec![];
            let mut p = caller_proc(vm);
            while let Some(pid) = p {
                let pd = vm.heap.proc_data(pid);
                let irep = &vm.ireps[pd.irep];
                for i in 0..irep.nlocals.saturating_sub(1) {
                    if let Some(Some(s)) = irep.lv.get(i) {
                        let n = vm.syms.name(*s);
                        if n.first().map(|c| *c == b'*' || *c == b'&').unwrap_or(true) { continue; }
                        if !out.iter().any(|v| *v == Value::Sym(*s)) { out.push(Value::Sym(*s)); }
                    }
                }
                if pd.scope { break; }
                p = pd.upper;
            }
            Ok(vm.ary_new(out))
        }),
        ("public_send", |vm, s, a, b| {
            if a.is_empty() { return Err(vm.argnum_error(0, "1+")); }
            let m = sym_arg(vm, a[0])?;
            if let Some((_, owner)) = vm.find_method(vm.class_of(s), m) {
                let vis = vm.method_vis(owner, m);
                if vis != Vis::Public { let name = vm.sym_name(m); let d = vm.describe_for_error(s); let v = if vis == Vis::Private { "private" } else { "protected" }; return Err(vm.no_method_error(m, s, &format!("{v} method '{name}' called for {d}"))); }
            }
            vm.funcall(s, m, &a[1..], b)
        }),
    ]);
    let m = vm.core.module;
    vm.define_methods(m, &[
        ("class_variables", |vm, s, a, _b| {
            let inherit = regular_arg(a);
            let mut out = vec![];
            let mut c = s.obj();
            while let Some(x) = c {
                let owner = vm.heap.class(x).iclass_of.unwrap_or(x);
                let mut names: Vec<Sym> = vm.heap.class(owner).cvars.keys().copied().collect();
                names.sort_by_key(|k| k.0);
                for k in names { if !out.iter().any(|v| *v == Value::Sym(k)) { out.push(Value::Sym(k)); } }
                if !inherit { break; }
                c = vm.heap.class(x).superclass;
            }
            Ok(vm.ary_new(out))
        }),
        ("remove_class_variable", |vm, s, a, _b| {
            argc!(vm, a, 1);
            let id = cv_check(vm, a[0])?;
            let c = s.obj().unwrap();
            if vm.heap.get(c).frozen { return Err(vm.frozen_error(s)); }
            if let Some(v) = vm.heap.class_mut(c).cvars.remove(&id) { return Ok(v.get()); }
            let n = vm.sym_name(id); let cn = vm.inspect_str(s)?;
            if cv_defined(vm, c, id) { return Err(vm.name_error(id, &format!("cannot remove {n} for {cn}"))); }
            Err(vm.name_error(id, &format!("class variable {n} not defined for {cn}")))
        }),
        ("class_variable_defined?", |vm, s, a, _b| { argc!(vm, a, 1); let id = cv_check(vm, a[0])?; Ok(Value::bool(cv_defined(vm, s.obj().unwrap(), id))) }),
        ("class_variable_get", |vm, s, a, _b| { argc!(vm, a, 1); let id = cv_check(vm, a[0])?; match vm.cvar_lookup(s.obj().unwrap(), id) { Some(v) => Ok(v), None => { let n = vm.sym_name(id); let cn = vm.class_name(s.obj().unwrap()); Err(vm.name_error(id, &format!("uninitialized class variable {n} in {cn}"))) } } }),
        ("class_variable_set", |vm, s, a, _b| { argc!(vm, a, 2); let id = cv_check(vm, a[0])?; vm.cvar_store(s.obj().unwrap(), id, a[1])?; Ok(a[1]) }),
        ("included_modules", |vm, s, _a, _b| {
            let me = s.obj().unwrap();
            let origin = vm.heap.class(me).origin.unwrap_or(me);
            let mut out = vec![];
            let mut c = Some(me);
            while let Some(x) = c {
                let cd = vm.heap.class(x);
                if x != origin && cd.origin_of.is_none() { if let Some(m) = cd.iclass_of { if vm.heap.class(m).is_module { out.push(Value::Obj(m)); } } }
                c = cd.superclass;
            }
            Ok(vm.ary_new(out))
        }),
        ("instance_methods", |vm, s, a, _b| { let l = instance_method_list(vm, regular_arg(a), s.obj().unwrap(), Filt::NoPriv); Ok(vm.ary_new(l)) }),
        ("public_instance_methods", |vm, s, a, _b| { let l = instance_method_list(vm, regular_arg(a), s.obj().unwrap(), Filt::Public); Ok(vm.ary_new(l)) }),
        ("private_instance_methods", |vm, s, a, _b| { let l = instance_method_list(vm, regular_arg(a), s.obj().unwrap(), Filt::Private); Ok(vm.ary_new(l)) }),
        ("protected_instance_methods", |vm, s, a, _b| { let l = instance_method_list(vm, regular_arg(a), s.obj().unwrap(), Filt::Protected); Ok(vm.ary_new(l)) }),
        ("undefined_instance_methods", |vm, s, a, _b| { argc!(vm, a, 0); let me = s.obj().unwrap(); let c = vm.heap.class(me).origin.unwrap_or(me); let owner = vm.table_owner(c); let mut names: Vec<Sym> = vm.heap.class(owner).methods.iter().filter(|(_, m)| matches!(m, Method::Undef)).map(|(k, _)| *k).collect(); names.sort_by_key(|k| k.0); let l: Vec<Value> = names.into_iter().map(Value::Sym).collect(); Ok(vm.ary_new(l)) }),
        ("remove_method", |vm, s, a, _b| {
            let me = s.obj().unwrap();
            if vm.heap.get(me).frozen { return Err(vm.frozen_error(s)); }
            for v in a {
                let mid = sym_arg(vm, *v)?;
                let c = vm.heap.class(me).origin.unwrap_or(me);
                let owner = vm.table_owner(c);
                if vm.heap.class_mut(owner).methods.remove(&mid).is_none() { let n = vm.sym_name(mid); let cn = vm.class_name(me); return Err(vm.name_error(mid, &format!("method '{n}' not defined in {cn}"))); }
                vm.heap.class_mut(owner).vis.remove(&mid);
                // the hook: `singleton_method_removed` on the attached object, else `method_removed`
                vm.method_removed_hook(me, mid)?;
            }
            Ok(s)
        }),
        ("method_removed", |_vm, _s, _a, _b| Ok(Value::Nil)),
        ("constants", |vm, s, a, _b| {
            let inherit = regular_arg(a);
            let mut out = vec![];
            let mut c = s.obj();
            while let Some(x) = c {
                const_at(vm, x, &mut out);
                if !inherit { break; }
                c = vm.heap.class(x).superclass;
                if c == Some(vm.core.object) { break; }
            }
            Ok(vm.ary_new(out))
        }),
    ]);
    let msc = vm.singleton_class(Value::Obj(m)).expect("Module singleton");
    vm.define_methods(msc, &[
        ("constants", |vm, s, a, _b| {
            // Module.constants with no argument: the constants visible from the caller
            if !a.is_empty() || s.obj() != Some(vm.core.module) {
                let inherit = regular_arg(a);
                let mut out = vec![];
                let mut c = s.obj();
                while let Some(x) = c { const_at(vm, x, &mut out); if !inherit { break; } c = vm.heap.class(x).superclass; if c == Some(vm.core.object) { break; } }
                return Ok(vm.ary_new(out));
            }
            let mut out = vec![];
            let mut p = caller_proc(vm);
            let first = p.map(|pid| vm.heap.proc_data(pid).target_class.unwrap_or(vm.core.object)).unwrap_or(vm.core.object);
            while let Some(pid) = p { let pd = vm.heap.proc_data(pid); let tc = pd.target_class.unwrap_or(vm.core.object); const_at(vm, tc, &mut out); p = pd.upper; }
            let mut c = Some(first);
            while let Some(x) = c { const_at(vm, x, &mut out); c = vm.heap.class(x).superclass; if c == Some(vm.core.object) { break; } }
            Ok(vm.ary_new(out))
        }),
        ("nesting", |vm, _s, _a, _b| {
            let mut out = vec![];
            let mut last: Option<ObjId> = None;
            let mut p = caller_proc(vm);
            while let Some(pid) = p {
                let pd = vm.heap.proc_data(pid);
                if pd.upper.is_none() { break; } // the top-level program has no lexical class
                if pd.scope { if let Some(tc) = pd.target_class { if Some(tc) != last { last = Some(tc); out.push(Value::Obj(tc)); } } }
                p = pd.upper;
            }
            Ok(vm.ary_new(out))
        }),
    ]);
    // The reference defines these on Kernel (`mrb_init_kernel`); SabiRuby's core
    // stage put its natives on Object. Move every Object native to Kernel so the
    // listings, owners and `Kernel.instance_method(:inspect)` match, and drop the
    // Class-level copies that would shadow the Module ones above.
    let object = vm.core.object;
    let names: Vec<Sym> = vm.heap.class(object).methods.keys().copied().collect();
    for n in names {
        if vm.heap.class(k).methods.contains_key(&n) { vm.heap.class_mut(object).methods.remove(&n); vm.heap.class_mut(object).vis.remove(&n); continue; }
        if let Some(m) = vm.heap.class_mut(object).methods.remove(&n) {
            let vis = vm.heap.class_mut(object).vis.remove(&n);
            vm.heap.class_mut(k).methods.insert(n, m);
            if let Some(v) = vis { vm.heap.class_mut(k).vis.insert(n, v); }
        }
    }
    let class = vm.core.class;
    for n in ["class_variable_get", "class_variable_set", "constants", "class_variables"] { let s = vm.intern(n); vm.heap.class_mut(class).methods.remove(&s); }
    // Both are module functions in the reference: `metaprog_krn_rom_entries` marks the
    // instance copies `MRB_MT_PRIVATE` and `metaprog_krn_module_function_entries` puts the
    // public ones on Kernel's singleton class. `global_variables` is written in `kernel.rs`
    // here, so this comes after the sweep above, when both sit on Kernel.
    for n in ["global_variables", "local_variables"] { let _ = vm.make_module_function(k, n); }
    let _ = Slot::NIL;
    let _ = ObjKind::Object;
}
