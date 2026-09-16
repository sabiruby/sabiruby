//! BasicObject / Object / Module / Class / NilClass / TrueClass / FalseClass.

use alloc::{format, string::String, vec, vec::Vec};

use crate::argc;
use crate::error::VmResult;
use crate::object::{Method, ObjKind, Vis};
use crate::value::{Slot, Value};
use crate::vm::Vm;

/// `Class#allocate`, the one `Class#new` skips the dispatch for.
fn default_allocate(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    match s.obj() { Some(c) => vm.instance_alloc(c), None => Err(vm.raise_type("not a class")) }
}

pub fn init(vm: &mut Vm) {
    let c = vm.core;
    // `bob_rom_entries` of src/class.c marks these `MRB_MT_PRIVATE`. `initialize` is there
    // too but needs no marking: `Vm::def_method_raw` makes it private wherever it is defined.
    vm.define_private_methods(c.basic_object, &[
        // the three hooks a change to an object's singleton class fires, as no-ops
        // (`mrb_do_nothing` for all three). The definition being the one here is what lets
        // `Vm::method_table_hook` skip the call entirely.
        ("singleton_method_added", |_vm, _s, _a, _b| Ok(Value::Nil)),
        ("singleton_method_removed", |_vm, _s, _a, _b| Ok(Value::Nil)),
        ("singleton_method_undefined", |_vm, _s, _a, _b| Ok(Value::Nil)),
        ("method_missing", method_missing),
    ]);
    vm.define_methods(c.basic_object, &[
        // `MRB_MT_PRIVATE` in `bob_rom_entries`; marked below, where the table is complete
        ("initialize", |_vm, _s, _a, _b| Ok(Value::Nil)),
        ("==", |vm, s, a, _b| { Ok(Value::bool(a.first().map(|x| vm.same_value(*x, s)).unwrap_or(false))) }),
        ("equal?", |_vm, s, a, _b| Ok(Value::bool(a.first().map(|x| same_object(*x, s)).unwrap_or(false)))),
        ("!", |_vm, s, _a, _b| Ok(Value::bool(!s.truthy()))),
        ("!=", |vm, s, a, _b| { argc!(vm, a, 1); let r = vm.equal(s, a[0])?; Ok(Value::bool(!r)) }),
        ("__id__", |vm, s, a, _b| { argc!(vm, a, 0); Ok(object_id(s)) }),
        ("__send__", send),
        ("instance_eval", |vm, s, _a, b| { if b.is_nil() { return Err(vm.raise_arg("no block given")); } vm.call_block_with_self(b, s, &[s]) }),
    ]);
    vm.mark_private(c.basic_object, &["initialize"]);
    vm.define_methods(c.object, &[
        ("class", |vm, s, _a, _b| Ok(Value::Obj(vm.real_class_of(s)))),
        ("singleton_class", |vm, s, _a, _b| Ok(Value::Obj(vm.singleton_class(s)?))),
        ("object_id", |_vm, s, _a, _b| Ok(object_id(s))),
        ("hash", |vm, s, _a, _b| Ok(Value::Int(vm.value_hash(s)))),
        ("eql?", |vm, s, a, _b| Ok(Value::bool(a.first().map(|x| vm.same_value(*x, s)).unwrap_or(false)))),
        ("===", |vm, s, a, _b| { argc!(vm, a, 1); Ok(Value::bool(vm.equal(s, a[0])?)) }),

        ("nil?", |_vm, s, _a, _b| Ok(Value::bool(s.is_nil()))),
        ("to_s", |vm, s, _a, _b| { let t = any_to_s(vm, s); Ok(vm.str_from(t)) }),
        ("inspect", obj_inspect),
        ("is_a?", is_a),
        ("kind_of?", is_a),
        ("instance_of?", |vm, s, a, _b| { argc!(vm, a, 1); let c = class_arg(vm, a[0])?; Ok(Value::bool(vm.real_class_of(s) == c)) }),
        ("respond_to?", |vm, s, a, _b| { argc!(vm, a, 1, 2); let m = sym_arg(vm, a[0])?; let include_private = a.get(1).map(|v| v.truthy()).unwrap_or(false); if let Some((mth, owner)) = vm.find_method(vm.class_of(s), m) { if let Method::Native(f) = mth { if vm.notimpl_fns.iter().any(|g| core::ptr::fn_addr_eq(*g, f)) { return Ok(Value::False); } } let vis = vm.method_vis(owner, m); if vis == Vis::Public || include_private { return Ok(Value::True); } return Ok(Value::False); } let rtm = vm.intern("respond_to_missing?"); let priv_ = a.get(1).copied().unwrap_or(Value::False); let r = vm.funcall(s, rtm, &[Value::Sym(m), priv_], Value::Nil)?; Ok(Value::bool(r.truthy())) }),
        ("respond_to_missing?", |_vm, _s, _a, _b| Ok(Value::False)),
        ("remove_instance_variable", |vm, s, a, _b| { argc!(vm, a, 1); let n = sym_arg(vm, a[0])?; match s { Value::Obj(o) => { if vm.heap.get(o).frozen { return Err(vm.frozen_error(s)); } let pos = vm.heap.get(o).ivars.iter().position(|(k, _)| *k == n); match pos { Some(i) => Ok(vm.heap.get_mut(o).ivars.remove(i).1.get()), None => { let nn = vm.sym_name(n); Err(vm.raise(vm.core.name_error, &format!("instance variable {nn} not defined"))) } } } _ => { let nn = vm.sym_name(n); Err(vm.raise(vm.core.name_error, &format!("instance variable {nn} not defined"))) } } }),
        ("send", send),
        ("public_send", |vm, s, a, b| { if a.is_empty() { return Err(vm.raise_arg("no method name given")); } let m = sym_arg(vm, a[0])?; if let Some((_, owner)) = vm.find_method(vm.class_of(s), m) { if vm.method_vis(owner, m) != Vis::Public { let name = vm.sym_name(m); let d = vm.describe_for_error(s); let v = if vm.method_vis(owner, m) == Vis::Private { "private" } else { "protected" }; return Err(vm.no_method_error(m, s, &format!("{v} method '{name}' called for {d}"))); } } vm.funcall(s, m, &a[1..], b) }),
        ("methods", |vm, s, a, _b| { let all = a.first().map(|v| v.truthy()).unwrap_or(true); let list = method_list(vm, vm.class_of(s), Some(Vis::Public), all); Ok(vm.ary_new(list)) }),
        ("public_methods", |vm, s, _a, _b| { let list = method_list(vm, vm.class_of(s), Some(Vis::Public), true); Ok(vm.ary_new(list)) }),
        ("private_methods", |vm, s, _a, _b| { let list = method_list(vm, vm.class_of(s), Some(Vis::Private), true); Ok(vm.ary_new(list)) }),
        ("protected_methods", |vm, s, _a, _b| { let list = method_list(vm, vm.class_of(s), Some(Vis::Protected), true); Ok(vm.ary_new(list)) }),
        ("dup", dup),
        ("clone", clone),
        ("freeze", |vm, s, _a, _b| { if let Value::Obj(o) = s { vm.heap.get_mut(o).frozen = true; let c = vm.heap.get(o).class; if vm.heap.class(c).is_singleton { vm.heap.get_mut(c).frozen = true; } } Ok(s) }),
        ("frozen?", |vm, s, _a, _b| Ok(Value::bool(match s { Value::Obj(o) => vm.heap.get(o).frozen, _ => true }))),
        ("instance_variable_get", |vm, s, a, _b| { argc!(vm, a, 1); let n = sym_arg(vm, a[0])?; Ok(match s { Value::Obj(o) => vm.heap.ivar_get(o, n), _ => Value::Nil }) }),
        ("instance_variable_set", |vm, s, a, _b| { argc!(vm, a, 2); let n = sym_arg(vm, a[0])?; match s { Value::Obj(o) => { vm.heap.ivar_set(o, n, a[1]); Ok(a[1]) } _ => Err(vm.raise_type("can't set instance variable")) } }),
        ("instance_variable_defined?", |vm, s, a, _b| { argc!(vm, a, 1); let n = sym_arg(vm, a[0])?; Ok(Value::bool(match s { Value::Obj(o) => vm.heap.get(o).ivars.iter().any(|(k, _)| *k == n), _ => false })) }),
        ("instance_variables", |vm, s, _a, _b| { let names: Vec<Value> = match s { Value::Obj(o) => vm.heap.get(o).ivars.iter().map(|(k, _)| Value::Sym(*k)).collect(), _ => vec![] }; Ok(vm.ary_new(names)) }),
        ("define_singleton_method", |vm, s, a, b| { argc!(vm, a, 1, 2); let n = sym_arg(vm, a[0])?; let body = if a.len() == 2 { a[1] } else { b }; let sc = vm.singleton_class(s)?; match body { Value::Obj(p) if matches!(vm.heap.get(p).kind, ObjKind::Proc(_)) => { if let ObjKind::Proc(pd) = &mut vm.heap.get_mut(p).kind { pd.target_class = Some(sc); } vm.def_method(sc, n, Method::Ruby(p), Vis::Public)?; Ok(Value::Sym(n)) } _ => Err(vm.raise_arg("tried to create Proc object without a block")) } }),
        ("extend", |vm, s, a, _b| { if let Value::Obj(o) = s { if vm.heap.get(o).frozen { return Err(vm.frozen_error(s)); } } for m in a.iter().rev() { let m = class_arg(vm, *m)?; let sc = vm.singleton_class(s)?; vm.include_module(sc, m); let hook = vm.intern("extended"); vm.funcall(Value::Obj(m), hook, &[s], Value::Nil)?; } Ok(s) }),
        ("singleton_methods", |vm, s, _a, _b| { let c = vm.class_of(s); let mut list = vec![]; if vm.heap.class(c).is_singleton { for (k, m) in &vm.heap.class(c).methods { if !matches!(m, Method::Undef) { list.push(Value::Sym(*k)); } } } Ok(vm.ary_new(list)) }),
    ]);
    // `MRB_MT_PRIVATE` in `krn_rom_entries` (src/kernel.c): these natives sit on Object
    // here and `ext_metaprog.rs` moves them to Kernel with their visibility
    vm.mark_private(c.object, &["respond_to_missing?"]);

    // Module
    vm.define_methods(c.module, &[
        // name, <, <=, <=>, >, >= are mruby-class-ext (`ext_class.rs`)
        ("to_s", mod_to_s),
        ("inspect", mod_to_s),
        ("===", |vm, s, a, _b| { argc!(vm, a, 1); Ok(Value::bool(vm.obj_is_kind_of(a[0], s.obj().unwrap()))) }),
        ("==", |_vm, s, a, _b| Ok(Value::bool(a.first().map(|x| *x == s).unwrap_or(false)))),
        ("include", |vm, s, a, _b| { let cls = s.obj().unwrap(); if vm.heap.get(cls).frozen { return Err(vm.frozen_error(s)); } for mv in a.iter().rev() { let m = class_arg(vm, *mv)?; if !vm.heap.class(m).is_module { let d = vm.inspect_str(*mv)?; return Err(vm.raise_type(&format!("wrong argument type {d} (expected Module)"))); } vm.include_module(cls, m); let hook = vm.intern("included"); vm.funcall(Value::Obj(m), hook, &[s], Value::Nil)?; } Ok(s) }),
        ("const_missing", |vm, s, a, _b| { argc!(vm, a, 1); let n = sym_arg(vm, a[0])?; let nn = vm.sym_name(n); let o = s.obj().unwrap(); let msg = if o == vm.core.object { format!("uninitialized constant {nn}") } else { let cn = vm.class_name(o); format!("uninitialized constant {cn}::{nn}") }; Err(vm.name_error(n, &msg)) }),
        ("initialize_copy", |vm, s, a, _b| { argc!(vm, a, 1); let (dst, src) = (s.obj().unwrap(), class_arg(vm, a[0])?); let (methods, consts, cvars, sup, is_module) = { let c = vm.heap.class(src); (c.methods.clone(), c.consts.clone(), c.cvars.clone(), c.superclass, c.is_module) }; let d = vm.heap.class_mut(dst); d.methods = methods; d.consts = consts; d.cvars = cvars; d.superclass = sup; d.is_module = is_module; d.name = None; Ok(s) }),
        ("include?", |vm, s, a, _b| { argc!(vm, a, 1); let m = class_arg(vm, a[0])?; let mut c = vm.heap.class(s.obj().unwrap()).superclass; while let Some(x) = c { if vm.heap.class(x).iclass_of == Some(m) { return Ok(Value::True); } c = vm.heap.class(x).superclass; } Ok(Value::False) }),
        ("ancestors", |vm, s, _a, _b| { let mut list = vec![]; let mut c = Some(s.obj().unwrap()); while let Some(x) = c { let cd = vm.heap.class(x); if cd.origin.is_some() { c = cd.superclass; continue; } let shown = cd.iclass_of.or(cd.origin_of).unwrap_or(x); list.push(Value::Obj(shown)); c = cd.superclass; } Ok(vm.ary_new(list)) }),
        ("attr_reader", |vm, s, a, _b| { attr(vm, s, a, true, false) }),
        ("attr_writer", |vm, s, a, _b| { attr(vm, s, a, false, true) }),
        ("attr_accessor", |vm, s, a, _b| { attr(vm, s, a, true, true) }),
        ("attr", |vm, s, a, _b| { attr(vm, s, a, true, false) }),
        ("private_class_method", |vm, s, a, _b| { let sc = vm.singleton_class(s)?; for n in flat_syms(vm, a)? { vm.set_visibility(sc, n, Vis::Private)?; } Ok(Value::Nil) }),
        ("public_class_method", |vm, s, a, _b| { let sc = vm.singleton_class(s)?; for n in flat_syms(vm, a)? { vm.set_visibility(sc, n, Vis::Public)?; } Ok(Value::Nil) }),
        ("alias_method", |vm, s, a, _b| { argc!(vm, a, 2); let m = s.obj().unwrap(); let (new, old) = (sym_arg(vm, a[0])?, sym_arg(vm, a[1])?); vm.alias_method(m, new, old)?; Ok(s) }),
        ("undef_method", |vm, s, a, _b| { let m = s.obj().unwrap(); for n in a { let n = sym_arg(vm, *n)?; vm.undef_method(m, n)?; } Ok(s) }),
        ("remove_method", |vm, s, a, _b| { let m = s.obj().unwrap(); if vm.heap.get(m).frozen { return Err(vm.frozen_error(s)); } let t = vm.def_target(m); for n in a { let n = sym_arg(vm, *n)?; if vm.heap.class_mut(t).methods.remove(&n).is_none() { let nn = vm.sym_name(n); let cn = vm.class_name(m); return Err(vm.raise(vm.core.name_error, &format!("method '{nn}' not defined in {cn}"))); } } Ok(s) }),
        ("define_method", |vm, s, a, b| { argc!(vm, a, 1, 2); let m = s.obj().unwrap(); let n = sym_arg(vm, a[0])?; let body = if a.len() == 2 { a[1] } else { b }; match body { Value::Obj(p) if matches!(vm.heap.get(p).kind, ObjKind::Proc(_)) => { if let ObjKind::Proc(pd) = &mut vm.heap.get_mut(p).kind { pd.target_class = Some(m); } let (vis, _) = vm.current_def_vis(m); vm.def_method(m, n, Method::Ruby(p), vis)?; Ok(Value::Sym(n)) } Value::Nil if a.len() == 1 => Err(vm.raise_arg("tried to create Proc object without a block")), v => { let d = vm.describe_for_type_error(v); Err(vm.raise_type(&format!("wrong argument type {d} (expected Proc)"))) } } }),
        ("method_defined?", |vm, s, a, _b| { argc!(vm, a, 1, 2); let n = sym_arg(vm, a[0])?; let m = s.obj().unwrap(); Ok(Value::bool(match vm.find_method(m, n) { Some((Method::Native(f), _)) if vm.notimpl_fns.iter().any(|g| core::ptr::fn_addr_eq(*g, f)) => false, Some((_, owner)) => vm.method_vis(owner, n) != Vis::Private, None => false })) }),
        ("public_method_defined?", |vm, s, a, _b| { argc!(vm, a, 1, 2); let n = sym_arg(vm, a[0])?; let m = s.obj().unwrap(); Ok(Value::bool(match vm.find_method(m, n) { Some((_, owner)) => vm.method_vis(owner, n) == Vis::Public, None => false })) }),
        ("private_method_defined?", |vm, s, a, _b| { argc!(vm, a, 1, 2); let n = sym_arg(vm, a[0])?; let m = s.obj().unwrap(); Ok(Value::bool(match vm.find_method(m, n) { Some((_, owner)) => vm.method_vis(owner, n) == Vis::Private, None => false })) }),
        ("protected_method_defined?", |vm, s, a, _b| { argc!(vm, a, 1, 2); let n = sym_arg(vm, a[0])?; let m = s.obj().unwrap(); Ok(Value::bool(match vm.find_method(m, n) { Some((_, owner)) => vm.method_vis(owner, n) == Vis::Protected, None => false })) }),
        ("instance_methods", |vm, s, a, _b| { let all = a.first().map(|v| v.truthy()).unwrap_or(true); let list = method_list(vm, s.obj().unwrap(), Some(Vis::Public), all); Ok(vm.ary_new(list)) }),
        ("public_instance_methods", |vm, s, a, _b| { let all = a.first().map(|v| v.truthy()).unwrap_or(true); let list = method_list(vm, s.obj().unwrap(), Some(Vis::Public), all); Ok(vm.ary_new(list)) }),
        ("private_instance_methods", |vm, s, a, _b| { let all = a.first().map(|v| v.truthy()).unwrap_or(true); let list = method_list(vm, s.obj().unwrap(), Some(Vis::Private), all); Ok(vm.ary_new(list)) }),
        ("protected_instance_methods", |vm, s, a, _b| { let all = a.first().map(|v| v.truthy()).unwrap_or(true); let list = method_list(vm, s.obj().unwrap(), Some(Vis::Protected), all); Ok(vm.ary_new(list)) }),
        ("prepend", |vm, s, a, _b| { let cls = s.obj().unwrap(); for mv in a.iter().rev() { let m = class_arg(vm, *mv)?; if !vm.heap.class(m).is_module { let d = vm.inspect_str(*mv)?; return Err(vm.raise_type(&format!("wrong argument type {d} (expected Module)"))); } vm.prepend_module(cls, m)?; let hook = vm.intern("prepended"); vm.funcall(Value::Obj(m), hook, &[s], Value::Nil)?; } Ok(s) }),
        ("instance_method", |vm, _s, _a, _b| Err(vm.raise(vm.core.not_implemented_error, "instance_method is not implemented"))),
        ("const_get", |vm, s, a, _b| {
            argc!(vm, a, 1);
            // "A::B" paths are resolved segment by segment
            if let Some(path) = vm.str_bytes(a[0]).map(|b| String::from_utf8_lossy(b).into_owned()) {
                if path.contains("::") {
                    let mut cur = s.obj().unwrap();
                    for seg in path.split("::") {
                        if seg.is_empty() { return Err(vm.raise(vm.core.name_error, &format!("wrong constant name '{path}'"))); }
                        let n = vm.intern(seg);
                        check_const_name(vm, n)?;
                        match vm.const_get(cur, n) { Some(Value::Obj(o)) if vm.heap.is_class(o) => cur = o, Some(v) => { if seg == path.split("::").last().unwrap() { return Ok(v); } let d = vm.inspect_str(v)?; return Err(vm.raise_type(&format!("{d} is not a class/module"))); } None => { let cm = vm.intern("const_missing"); return vm.funcall(Value::Obj(cur), cm, &[Value::Sym(n)], Value::Nil); } }
                    }
                    return Ok(Value::Obj(cur));
                }
            }
            let n = sym_arg(vm, a[0])?;
            check_const_name(vm, n)?;
            match vm.const_get(s.obj().unwrap(), n) { Some(v) => Ok(v), None => { let cm = vm.intern("const_missing"); vm.funcall(s, cm, &[Value::Sym(n)], Value::Nil) } }
        }),
        ("const_set", |vm, s, a, _b| { argc!(vm, a, 2); let n = sym_arg(vm, a[0])?; check_const_name(vm, n)?; let m = s.obj().unwrap(); if vm.heap.get(m).frozen { return Err(vm.frozen_error(s)); } vm.heap.class_mut(m).consts.insert(n, Slot::from(a[1])); if let Value::Obj(o) = a[1] { if vm.heap.is_class(o) && vm.heap.class(o).name.is_none() { vm.heap.class_mut(o).name = Some(n); vm.heap.class_mut(o).outer = Some(m); } } vm.const_added(m, n)?; Ok(a[1]) }),
        ("const_defined?", |vm, s, a, _b| { argc!(vm, a, 1, 2); let n = sym_arg(vm, a[0])?; check_const_name(vm, n)?; Ok(Value::bool(vm.const_get(s.obj().unwrap(), n).is_some())) }),
        ("constants", |vm, s, _a, _b| { let list: Vec<Value> = vm.heap.class(s.obj().unwrap()).consts.keys().map(|k| Value::Sym(*k)).collect(); Ok(vm.ary_new(list)) }),
        ("class_variable_get", |vm, s, a, _b| { argc!(vm, a, 1); let n = sym_arg(vm, a[0])?; Ok(vm.heap.class(s.obj().unwrap()).cvars.get(&n).map(|s| s.get()).unwrap_or(Value::Nil)) }),
        ("class_variable_set", |vm, s, a, _b| { argc!(vm, a, 2); let n = sym_arg(vm, a[0])?; vm.heap.class_mut(s.obj().unwrap()).cvars.insert(n, Slot::from(a[1])); Ok(a[1]) }),
        ("module_eval", |vm, s, _a, b| { if b.is_nil() { return Err(vm.raise_arg("no block given")); } vm.call_block_with_self(b, s, &[s]) }),
        ("class_eval", |vm, s, _a, b| { if b.is_nil() { return Err(vm.raise_arg("no block given")); } vm.call_block_with_self(b, s, &[s]) }),
    ]);
    // The `MRB_MT_PRIVATE` half of `mod_rom_entries` (src/class.c). `method_removed` is *not*
    // among them here although the core table marks it private: mruby-metaprog defines it a
    // second time (`metaprog_mod_rom_entries`, `mrb_f_nil`) without the flag, and that second
    // definition is the one the reference ends up with. It is in `ext_metaprog.rs`, public,
    // next to the `remove_method` that fires it.
    vm.define_private_methods(c.module, &[
        ("included", |_vm, _s, _a, _b| Ok(Value::Nil)),
        ("extended", |_vm, _s, _a, _b| Ok(Value::Nil)),
        ("prepended", |_vm, _s, _a, _b| Ok(Value::Nil)),
        ("method_added", |_vm, _s, _a, _b| Ok(Value::Nil)),
        ("method_undefined", |_vm, _s, _a, _b| Ok(Value::Nil)),
        ("const_added", |_vm, _s, _a, _b| Ok(Value::Nil)),
        ("public", |vm, s, a, _b| set_vis(vm, s, a, Vis::Public)),
        ("private", |vm, s, a, _b| set_vis(vm, s, a, Vis::Private)),
        ("protected", |vm, s, a, _b| set_vis(vm, s, a, Vis::Protected)),
        ("remove_const", |vm, s, a, _b| { argc!(vm, a, 1); let n = sym_arg(vm, a[0])?; check_const_name(vm, n)?; let m = s.obj().unwrap(); if vm.heap.get(m).frozen { return Err(vm.frozen_error(s)); } match vm.heap.class_mut(m).consts.remove(&n) { Some(v) => Ok(v.get()), None => { let nn = vm.sym_name(n); Err(vm.raise(vm.core.name_error, &format!("constant {nn} not defined"))) } } }),
        ("module_function", |vm, s, a, _b| {
            let m = s.obj().unwrap();
            if a.is_empty() { vm.set_scope_vis(Vis::Private, true); return Ok(Value::Nil); }
            let names = flat_syms(vm, a)?;
            for n in &names {
                match vm.find_method(m, *n) {
                    Some((mth, _)) => { let sc = vm.singleton_class(s)?; vm.def_method(sc, *n, mth, Vis::Public)?; vm.set_visibility(m, *n, Vis::Private)?; }
                    None => { let nn = vm.sym_name(*n); let cn = vm.class_name(m); return Err(vm.raise(vm.core.name_error, &format!("undefined method '{nn}' for class '{cn}'"))); }
                }
            }
            Ok(if names.len() == 1 { Value::Sym(names[0]) } else { let v: Vec<Value> = names.iter().map(|n| Value::Sym(*n)).collect(); vm.ary_new(v) })
        }),
    ]);
    // Class
    vm.define_methods(c.class, &[
        // `Class#new` is bytecode in the reference (`new_iseq` of src/class.c): it sends
        // `allocate` and then `initialize`, so a class that redefines `allocate` decides
        // what its `new` makes
        ("new", |vm, s, a, b| {
            let c = match s.obj() { Some(c) => c, None => return Err(vm.raise_type("not a class")) };
            let alloc = vm.intern("allocate");
            let obj = match vm.find_method(vm.class_of(s), alloc) {
                // the built-in allocator: no need to go through a dispatch
                Some((crate::object::Method::Native(f), _)) if core::ptr::fn_addr_eq(f, default_allocate as crate::object::NativeFn) => vm.instance_alloc(c)?,
                Some(_) => vm.funcall(s, alloc, &[], Value::Nil)?,
                None => vm.instance_alloc(c)?,
            };
            let init = vm.s.initialize;
            if vm.respond_to(obj, init) {
                vm.funcall(obj, init, a, b)?;
            } else if !a.is_empty() {
                return Err(vm.argnum_error(a.len(), "0"));
            }
            Ok(obj)
        }),
        ("allocate", default_allocate),
        ("superclass", |vm, s, _a, _b| { let mut c = vm.heap.class(s.obj().unwrap()).superclass; while let Some(x) = c { let cd = vm.heap.class(x); if cd.iclass_of.is_none() && !cd.is_singleton { return Ok(Value::Obj(x)); } c = cd.superclass; } Ok(Value::Nil) }),
        ("class_variable_get", |vm, s, a, _b| { argc!(vm, a, 1); let n = sym_arg(vm, a[0])?; Ok(vm.heap.class(s.obj().unwrap()).cvars.get(&n).map(|s| s.get()).unwrap_or(Value::Nil)) }),
    ]);
    // `cls_rom_entries` (src/class.c) marks `inherited` `MRB_MT_PRIVATE`
    vm.define_private_methods(c.class, &[
        ("inherited", |_vm, _s, _a, _b| Ok(Value::Nil)),
    ]);
    // Module.new / Class.new (with optional superclass) on their singleton classes
    let sc = vm.singleton_class(Value::Obj(c.class)).unwrap();
    vm.define_method(sc, "new", |vm, _s, a, b| {
        argc!(vm, a, 0, 1);
        let sup = if a.is_empty() { vm.core.object } else {
            match a[0] { Value::Obj(o) if vm.heap.is_class(o) && !vm.heap.class(o).is_module && !vm.heap.class(o).is_singleton => o, v => { let d = vm.inspect_str(v)?; return Err(vm.raise_type(&format!("superclass must be a Class ({d} given)"))); } }
        };
        let cls = vm.heap.alloc(vm.core.class, ObjKind::Class(crate::object::ClassData { superclass: Some(sup), ..Default::default() }));
        vm.singleton_class(Value::Obj(cls))?;
        vm.call_inherited(sup, cls)?;
        if !b.is_nil() { vm.call_block_with_self(b, Value::Obj(cls), &[Value::Obj(cls)])?; }
        Ok(Value::Obj(cls))
    });
    let sm = vm.singleton_class(Value::Obj(c.module)).unwrap();
    vm.define_method(sm, "new", |vm, _s, _a, b| {
        let m = vm.heap.alloc(vm.core.module, ObjKind::Class(crate::object::ClassData { is_module: true, ..Default::default() }));
        if !b.is_nil() { vm.call_block_with_self(b, Value::Obj(m), &[Value::Obj(m)])?; }
        Ok(Value::Obj(m))
    });

    // main (the top-level self) prints as "main"
    let ts = Value::Obj(vm.top_self);
    let msc = vm.singleton_class(ts).unwrap();
    vm.define_methods(msc, &[("to_s", |vm, _s, _a, _b| Ok(vm.str_new(b"main"))), ("inspect", |vm, _s, _a, _b| Ok(vm.str_new(b"main")))]);
    // Version constants (src/version.c). RUBY_ENGINE stays "mruby": the reference's own tests
    // branch on it (test/assert.rb, mruby-fiber's fiber2.rb) to tell mruby from CRuby, and so
    // may any script written for mruby. What tells SabiRuby apart is MRUBY_PLATFORM, and
    // SABIRUBY_VERSION (this crate's version), which the reference does not define at all —
    // `defined?(SABIRUBY_VERSION)` is the one-line "am I on SabiRuby?" (docs/design/gems.md).
    for (name, v) in [("RUBY_VERSION", "4.1"), ("RUBY_ENGINE", "mruby"), ("RUBY_ENGINE_VERSION", "4.1.0"), ("MRUBY_VERSION", "4.1.0"),
                      ("MRUBY_PLATFORM", "rust-sabiruby"), ("SABIRUBY_VERSION", env!("CARGO_PKG_VERSION")), ("MRUBY_RELEASE_DATE", "2026-09-04"),
                      // the commit this VM was built from (`build.rs`); `HEAD` where there was no git,
                      // which is the reference's own default too
                      ("MRUBY_REVISION", crate::REVISION),
                      ("MRUBY_DESCRIPTION", "mruby 4.1.0RC (2026-09-04)"), ("MRUBY_COPYRIGHT", "mruby - Copyright (c) 2010-2026 mruby developers")] {
        let s = vm.str_new(v.as_bytes());
        if let Some(o) = s.obj() { vm.heap.get_mut(o).frozen = true; }
        let n = vm.intern(name);
        vm.heap.class_mut(c.object).consts.insert(n, Slot::from(s));
    }
    // GC module (docs/design/gc.md): `start`, `enable`/`disable`, `interval_ratio` and `malloc_threshold` drive the
    // collector; the incremental/generational knobs are kept as values only.
    let gc = vm.define_module("GC");
    let gsc = vm.singleton_class(Value::Obj(gc)).unwrap();
    vm.define_methods(gsc, &[
        ("start", |vm, _s, _a, _b| { vm.gc_start(); Ok(Value::Nil) }),
        ("enable", |vm, _s, _a, _b| { let old = vm.gc_disabled; vm.gc_disabled = false; Ok(Value::bool(old)) }),
        ("disable", |vm, _s, _a, _b| { let old = vm.gc_disabled; vm.gc_disabled = true; Ok(Value::bool(old)) }),
        ("interval_ratio", |vm, _s, _a, _b| Ok(Value::Int(vm.gc_interval_ratio))),
        ("interval_ratio=", |vm, _s, a, _b| { argc!(vm, a, 1); let r = vm.expect_int(a[0], "ratio")?; vm.gc_interval_ratio = r; Ok(Value::Int(r)) }),
        ("step_ratio", |_vm, _s, _a, _b| Ok(Value::Int(200))),
        ("step_ratio=", |vm, _s, a, _b| { argc!(vm, a, 1); let r = vm.expect_int(a[0], "ratio")?; if r <= 0 { return Err(vm.raise_arg("step_ratio must be positive")); } Ok(Value::Nil) }),
        ("step_limit", |vm, _s, _a, _b| Ok(Value::Int(vm.gc_step_limit))),
        ("step_limit=", |vm, _s, a, _b| { argc!(vm, a, 1); let r = vm.expect_int(a[0], "limit")?; if r < 0 { return Err(vm.raise_arg("step_limit must be non-negative")); } vm.gc_step_limit = r; Ok(Value::Int(r)) }),
        ("generational_mode", |_vm, _s, _a, _b| Ok(Value::False)),
        ("generational_mode=", |vm, _s, a, _b| {
            // mruby-task: a generational minor cycle is one atomic step, which would defeat a
            // collector the scheduler drives (`gc_scheduler_driven_set`)
            if vm.task.gc_driven && a.first().map(|v| v.truthy()).unwrap_or(false) {
                return Err(vm.raise(vm.core.runtime_error, "generational mode cannot be enabled while GC is scheduler-driven"));
            }
            Ok(a.first().copied().unwrap_or(Value::Nil))
        }),
        // mruby-task's `GC.scheduler_driven` family (`mrbgems/mruby-task/src/gc.c`): the
        // scheduler collects from its idle points instead of the allocation path
        ("scheduler_driven", |vm, _s, _a, _b| Ok(Value::bool(vm.task.gc_driven))),
        ("scheduler_driven=", |vm, _s, a, _b| {
            argc!(vm, a, 1);
            let on = a[0].truthy();
            if on && vm.gc_disabled { return Err(vm.raise(vm.core.runtime_error, "cannot enable scheduler-driven GC while GC is disabled")); }
            vm.task.gc_driven = on;
            Ok(Value::bool(on))
        }),
        ("debt_limit", |vm, _s, _a, _b| Ok(Value::Int(vm.task.gc_debt_limit))),
        ("debt_limit=", |vm, _s, a, _b| { argc!(vm, a, 1); let n = vm.expect_int(a[0], "limit")?; if n < 0 { return Err(vm.raise_arg("debt_limit must be non-negative")); } vm.task.gc_debt_limit = n; Ok(Value::Int(n)) }),
        ("malloc_threshold", |vm, _s, _a, _b| Ok(Value::Int(vm.heap.malloc_threshold as i64))),
        ("malloc_threshold=", |vm, _s, a, _b| { argc!(vm, a, 1); let r = vm.expect_int(a[0], "threshold")?; if r < 0 { return Err(vm.raise_arg("malloc_threshold must be non-negative")); } vm.heap.malloc_threshold = r as usize; Ok(Value::Int(r)) }),
        ("stat", |vm, _s, _a, _b| { let h = vm.hash_new(); let live = vm.heap.live_count() as i64; let (mi, mt) = (vm.heap.malloc_increase as i64, vm.heap.malloc_threshold as i64); for (k, v) in [("live", live), ("debt", 0), ("state", 0), ("generational", 0), ("full", 0), ("step_limit", vm.gc_step_limit), ("malloc_increase", mi), ("malloc_threshold", mt), ("symbol_count", vm.syms.len() as i64), ("dynamic_symbol_count", 0)] { let ks = Value::Sym(vm.intern(k)); vm.hash_set(h, ks, Value::Int(v))?; } Ok(h) }),
    ]);
    let n = vm.intern("MRUBY_RELEASE_NO");
    vm.heap.class_mut(c.object).consts.insert(n, Slot::from(Value::Int(40100)));

    // nil / true / false
    vm.define_methods(c.nil_class, &[
        ("to_s", |vm, _s, _a, _b| Ok(vm.str_new(b""))),
        ("inspect", |vm, _s, _a, _b| Ok(vm.str_new(b"nil"))),
        // to_a/to_h/to_i/to_f are mruby-object-ext (`ext_object.rs`)
        // `nil_match`: nil matches nothing, and it is the only object core gives `=~` to
        ("=~", |vm, _s, a, _b| { argc!(vm, a, 1); Ok(Value::Nil) }),
        ("&", |_vm, _s, _a, _b| Ok(Value::False)),
        ("|", |_vm, _s, a, _b| Ok(Value::bool(a.first().map(|v| v.truthy()).unwrap_or(false)))),
        ("^", |_vm, _s, a, _b| Ok(Value::bool(a.first().map(|v| v.truthy()).unwrap_or(false)))),
    ]);
    vm.define_methods(c.true_class, &[
        ("to_s", |vm, _s, _a, _b| Ok(vm.str_new(b"true"))),
        ("inspect", |vm, _s, _a, _b| Ok(vm.str_new(b"true"))),
        ("&", |_vm, _s, a, _b| Ok(Value::bool(a.first().map(|v| v.truthy()).unwrap_or(false)))),
        ("|", |_vm, _s, _a, _b| Ok(Value::True)),
        ("^", |_vm, _s, a, _b| Ok(Value::bool(!a.first().map(|v| v.truthy()).unwrap_or(false)))),
    ]);
    vm.define_methods(c.false_class, &[
        ("to_s", |vm, _s, _a, _b| Ok(vm.str_new(b"false"))),
        ("inspect", |vm, _s, _a, _b| Ok(vm.str_new(b"false"))),
        ("&", |_vm, _s, _a, _b| Ok(Value::False)),
        ("|", |_vm, _s, a, _b| Ok(Value::bool(a.first().map(|v| v.truthy()).unwrap_or(false)))),
        ("^", |_vm, _s, a, _b| Ok(Value::bool(a.first().map(|v| v.truthy()).unwrap_or(false)))),
    ]);
    // Comparable / Enumerable bodies come from mrblib (`clamp` is mruby-compar-ext's Ruby).
}

/// `equal?`: identity; Floats compare bit for bit (mruby reads the boxed representation).
pub fn same_object(a: Value, b: Value) -> bool {
    match (a, b) { (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits(), _ => a == b }
}

pub fn object_id(v: Value) -> Value {
    match v {
        Value::Nil => Value::Int(8),
        Value::True => Value::Int(20),
        Value::False => Value::Int(0),
        Value::Int(i) => Value::Int(i.wrapping_mul(2).wrapping_add(1)),
        Value::Float(f) => Value::Int(f.to_bits() as i64),
        Value::Sym(s) => Value::Int((s.0 as i64) << 8 | 0x0c),
        Value::Obj(o) => Value::Int((o.0 as i64 + 1) * 8),
    }
}

/// `#<ClassName:0x...>` — the address is the heap index, so it is stable.
pub fn any_to_s(vm: &mut Vm, v: Value) -> String {
    let c = vm.real_class_of(v);
    let cn = vm.class_name(c);
    match v {
        Value::Obj(o) => format!("#<{cn}:0x{:012x}>", (o.0 as usize + 1) * 0x40),
        _ => format!("#<{cn}>"),
    }
}

fn obj_inspect(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    if let Value::Obj(o) = s {
        let ivars = vm.heap.get(o).ivars.clone();
        if !ivars.is_empty() && matches!(vm.heap.get(o).kind, ObjKind::Object) {
            let mut out = any_to_s(vm, s);
            out.pop(); // '>'
            let mut first = true;
            for (k, v) in ivars {
                out.push_str(if first { " " } else { ", " });
                first = false;
                out.push_str(&vm.sym_name(k));
                out.push('=');
                out.push_str(&vm.inspect_str(v.get())?);
            }
            out.push('>');
            return Ok(vm.str_from(out));
        }
    }
    let t = any_to_s(vm, s);
    Ok(vm.str_from(t))
}

fn mod_to_s(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let o = s.obj().unwrap();
    let cd = vm.heap.class(o);
    if cd.is_singleton {
        let att = cd.attached.map(|s| s.get()).unwrap_or(Value::Nil);
        let inner = if let Value::Obj(x) = att { if vm.heap.is_class(x) { vm.class_name(x) } else { any_to_s(vm, att) } } else { any_to_s(vm, att) };
        return Ok(vm.str_from(format!("#<Class:{inner}>")));
    }
    if cd.name.is_none() && cd.iclass_of.is_none() {
        let t = any_to_s(vm, s);
        return Ok(vm.str_from(t));
    }
    let n = vm.class_name(o);
    Ok(vm.str_from(n))
}

/// `NameError` unless `n` looks like a constant name (uppercase first letter).
pub fn check_const_name(vm: &mut Vm, n: crate::symbol::Sym) -> VmResult<()> {
    let name = vm.sym_name(n);
    let ok = name.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false) && name.chars().all(|c| c == '_' || c.is_alphanumeric() || !c.is_ascii());
    if ok { Ok(()) } else { Err(vm.raise(vm.core.name_error, &format!("wrong constant name {name}"))) }
}

/// `Kernel#clone`: like dup, but keeps the singleton class and the frozen state.
fn clone(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let o = match s { Value::Obj(o) => o, _ => return Ok(s) };
    if matches!(vm.heap.get(o).kind, ObjKind::Data { .. }) {
        let n = { let c = vm.real_class_of(s); vm.class_name(c) };
        return Err(vm.raise_type(&format!("can't clone {n}")));
    }
    let d = dup(vm, s, &[], Value::Nil)?;
    let Value::Obj(n) = d else { return Ok(d) };
    if n == o { return Ok(d); }
    let cur = vm.heap.get(o).class;
    if vm.heap.class(cur).is_singleton {
        let methods = vm.heap.class(cur).methods.clone();
        let sup = vm.heap.class(cur).superclass;
        let sc = vm.singleton_class(d)?;
        vm.heap.class_mut(sc).methods = methods;
        // keep modules extended into the original
        vm.heap.class_mut(sc).superclass = sup;
    }
    let frozen = vm.heap.get(o).frozen;
    vm.heap.get_mut(n).frozen = frozen;
    Ok(d)
}

pub fn class_arg(vm: &mut Vm, v: Value) -> VmResult<crate::value::ObjId> {
    match v {
        Value::Obj(o) if vm.heap.is_class(o) => Ok(o),
        _ => Err(vm.raise_type("class or module required")),
    }
}

pub fn sym_arg(vm: &mut Vm, v: Value) -> VmResult<crate::symbol::Sym> {
    match v {
        Value::Sym(s) => Ok(s),
        _ => match vm.str_bytes(v) {
            Some(b) => { let b = b.to_vec(); Ok(vm.syms.intern(&b)) }
            None => { let d = vm.inspect_str(v)?; Err(vm.raise_type(&format!("{d} is not a symbol nor a string"))) }
        },
    }
}

fn is_a(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    let c = class_arg(vm, a[0])?;
    Ok(Value::bool(vm.obj_is_kind_of(s, c)))
}


/// `Kernel#send` / `__send__` when reached from native code. From bytecode the
/// VM redirects in place instead (`Vm::op_send_redirect`), which is why this
/// must stay a plain fn: the VM recognises it by address.
pub fn send(vm: &mut Vm, s: Value, a: &[Value], b: Value) -> VmResult<Value> {
    if a.is_empty() { return Err(vm.argnum_error(0, "1+")); }
    let m = sym_arg(vm, a[0])?;
    vm.funcall(s, m, &a[1..], b)
}

fn method_missing(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    let name = match a.first() { Some(Value::Sym(m)) => vm.sym_name(*m), _ => "?".into() };
    let desc = vm.describe_for_error(s);
    Err(vm.raise(vm.core.no_method_error, &format!("undefined method '{name}' for {desc}")))
}

fn dup(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let o = match s { Value::Obj(o) => o, _ => return Ok(s) };
    let (class, ivars, kind) = {
        let h = vm.heap.get(o);
        let kind = match &h.kind {
            ObjKind::Object => ObjKind::Object,
            ObjKind::String(b) => ObjKind::String(b.clone()),
            ObjKind::Array(v) => ObjKind::Array(v.clone()),
            ObjKind::Hash(hd) => ObjKind::Hash(hd.clone()),
            ObjKind::Range { .. } => ObjKind::Object, // `initialize_copy` fills the copy in, as the reference's `range_initialize_copy` does
            ObjKind::Exception => ObjKind::Exception,
            // a Regexp copies its source and flags and compiles its own pattern
            // (`regexp_init_copy`), which `initialize_copy` does below; a MatchData is never
            // copied, `dup` on one answering a plain object as the reference's does
            #[cfg(feature = "regexp")]
            ObjKind::Regexp(_) | ObjKind::MatchData { .. } => ObjKind::Object,
            // a handle names one value the host owns, and the free hook fires once per object
            // that carries it: copying it would tell the host to drop that value twice, and
            // the second object would go on naming what is no longer there. Only the host
            // knows how to copy its own value, so only the host can define `dup` (a closure
            // that allocates a new one and calls `Vm::data_new` again).
            ObjKind::Data { .. } => { let n = { let c = vm.real_class_of(s); vm.class_name(c) }; return Err(vm.raise_type(&format!("can't dup {n}"))); }
            ObjKind::BigInt(b) => ObjKind::BigInt(b.clone()),
            ObjKind::Class(cd) => {
                let (hc, data, is_module, ivars) = (h.class, crate::object::ClassData { name: None, superclass: cd.superclass, methods: cd.methods.clone(), vis: cd.vis.clone(), consts: cd.consts.clone(), cvars: cd.cvars.clone(), is_module: cd.is_module, instance_kind: cd.instance_kind, outer: None, ..Default::default() }, cd.is_module, h.ivars.clone());
                // an origin's table belongs to the copy too
                let data = match cd.origin { Some(org) => { let o = vm.heap.class(org); crate::object::ClassData { methods: o.methods.clone(), vis: o.vis.clone(), superclass: o.superclass, ..data } } None => data };
                let n = vm.heap.alloc(hc, ObjKind::Class(data));
                vm.heap.get_mut(n).ivars = ivars;
                // prepended modules of the original are prepended to the copy too (farthest first)
                if vm.heap.class(o).origin.is_some() {
                    let origin = vm.heap.class(o).origin.unwrap();
                    let mut mods = vec![];
                    let mut cc = vm.heap.class(o).superclass;
                    while let Some(x) = cc { if x == origin { break; } if let Some(m) = vm.heap.class(x).iclass_of { mods.push(m); } cc = vm.heap.class(x).superclass; }
                    for m in mods.iter().rev() { vm.prepend_module(n, *m)?; }
                }
                // singleton methods (class methods) come along
                let cur = vm.heap.get(o).class;
                if vm.heap.class(cur).is_singleton {
                    let (methods, vis) = { let c = vm.heap.class(cur); (c.methods.clone(), c.vis.clone()) };
                    let sc = vm.singleton_class(Value::Obj(n))?;
                    vm.heap.class_mut(sc).methods = methods;
                    vm.heap.class_mut(sc).vis = vis;
                } else if !is_module { vm.singleton_class(Value::Obj(n))?; }
                return Ok(Value::Obj(n));
            }
            ObjKind::Proc(_) | ObjKind::Env(_) | ObjKind::Break { .. } | ObjKind::Fiber(_) | ObjKind::Task(_) => return Ok(s),
        };
        (vm.real_class_of(s), h.ivars.clone(), kind)
    };
    let n = vm.heap.alloc(class, kind);
    vm.heap.get_mut(n).ivars = ivars;
    // a copy of a String is read the way the original is (`RSTR_ENC_CR_COPY` in `mrb_str_dup`)
    vm.heap.get_mut(n).binary = vm.heap.get(o).binary;
    // mruby `init_copy`: the copy gets `initialize_copy(original)` (Ruby overrides may refuse)
    let ic = vm.intern("initialize_copy");
    vm.funcall(Value::Obj(n), ic, &[s], Value::Nil)?;
    Ok(Value::Obj(n))
}

fn attr(vm: &mut Vm, s: Value, a: &[Value], reader: bool, writer: bool) -> VmResult<Value> {
    let cls = s.obj().unwrap();
    let mut out = vec![];
    for n in a {
        let n = sym_arg(vm, *n)?;
        let name = vm.sym_name(n);
        let valid = name.chars().next().map(|c| c == '_' || c.is_alphabetic() || !c.is_ascii()).unwrap_or(false) && name.chars().all(|c| c == '_' || c.is_alphanumeric() || !c.is_ascii());
        if !valid { return Err(vm.raise(vm.core.name_error, &format!("invalid attribute name '{name}'"))); }
        let iv = vm.intern(&format!("@{name}"));
        let (vis, _) = vm.current_def_vis(cls);
        if reader {
            vm.def_method(cls, n, Method::AttrReader(iv), vis)?;
            out.push(Value::Sym(n));
        }
        if writer {
            let w = vm.intern(&format!("{name}="));
            vm.def_method(cls, w, Method::AttrWriter(iv), vis)?;
            out.push(Value::Sym(w));
        }
    }
    Ok(vm.ary_new(out))
}

fn method_list(vm: &Vm, class: crate::value::ObjId, want: Option<Vis>, inherited: bool) -> Vec<Value> {
    let mut list = vec![];
    let mut seen: Vec<crate::symbol::Sym> = vec![];
    let mut c = Some(class);
    let mut first = true;
    let has_origin = vm.heap.class(class).origin.is_some();
    let mut passed_origin = false;
    while let Some(x) = c {
        let cd = vm.heap.class(x);
        if cd.origin.is_some() { c = cd.superclass; first = false; continue; }
        if !inherited && !first {
            if has_origin && !passed_origin {
                if cd.origin_of == Some(class) { passed_origin = true; } else { c = cd.superclass; continue; } // prepended modules are not "own"
            } else { break; }
        }
        let owner = vm.table_owner(x);
        let t = vm.heap.class(owner);
        for (k, m) in &t.methods {
            if seen.contains(k) { continue; }
            seen.push(*k);
            if matches!(m, Method::Undef) { continue; }
            let vis = t.vis.get(k).copied().unwrap_or(Vis::Public);
            if want.map(|w| w == vis).unwrap_or(true) { list.push(Value::Sym(*k)); }
        }
        c = cd.superclass;
        first = false;
    }
    list
}

fn flat_syms(vm: &mut Vm, a: &[Value]) -> VmResult<Vec<crate::symbol::Sym>> {
    let mut out = vec![];
    for v in a {
        if let Some(list) = vm.ary_vals(*v) { for x in list { out.push(sym_arg(vm, x)?); } } else { out.push(sym_arg(vm, *v)?); }
    }
    Ok(out)
}

/// `public`/`private`/`protected`: with names sets them, without changes the frame default.
fn set_vis(vm: &mut Vm, s: Value, a: &[Value], vis: Vis) -> VmResult<Value> {
    let m = s.obj().unwrap();
    if a.is_empty() {
        vm.set_scope_vis(vis, false);
        return Ok(Value::Nil);
    }
    let names = flat_syms(vm, a)?;
    for n in &names { vm.set_visibility(m, *n, vis)?; }
    Ok(if names.len() == 1 { Value::Sym(names[0]) } else { let v: Vec<Value> = names.iter().map(|n| Value::Sym(*n)).collect(); vm.ary_new(v) })
}

impl Vm {
    /// Deterministic hash used by `Object#hash` and Hash keys.
    pub fn value_hash(&self, v: Value) -> i64 {
        fn fnv(bytes: &[u8]) -> i64 {
            let mut h: u64 = 0xcbf29ce484222325;
            for b in bytes { h ^= *b as u64; h = h.wrapping_mul(0x100000001b3); }
            (h >> 1) as i64
        }
        match v {
            Value::Obj(o) => match &self.heap.get(o).kind {
                ObjKind::String(b) => fnv(b),
                // `mrb_bint_hash`: by value, so a wide integer works as a Hash key
                ObjKind::BigInt(b) => fnv(&b.hash_bytes()),
                // by the handle, to agree with `==`: two objects naming the same host value
                // are one key in a Hash (`Vm::data_new`)
                ObjKind::Data { tag, handle } => {
                    let mut bytes = [0u8; 12];
                    bytes[..4].copy_from_slice(&tag.to_le_bytes());
                    bytes[4..].copy_from_slice(&handle.to_le_bytes());
                    fnv(&bytes)
                }
                _ => (o.0 as i64 + 1) * 8,
            },
            Value::Int(i) => i,
            Value::Float(f) => f.to_bits() as i64 >> 1,
            Value::Sym(s) => s.0 as i64 * 31 + 7,
            Value::Nil => 8,
            Value::True => 20,
            Value::False => 0,
        }
    }
}
