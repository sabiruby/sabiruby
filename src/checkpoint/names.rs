//! Permanent names: what an image calls the things the writer's VM had before the program
//! ran (its core classes, the methods of its mrblib, its C functions), found again here.
//!
//! The writer names them by how they are reached from `Object` (the image format's list):
//! `T` is main, `Integer` or `Math::DomainError` a class by constant path, `S:<name>` its
//! singleton class, `I:<name>/<k>` the k-th include class right above it, `M:<name>#<meth>`
//! the Proc of a method written in Ruby, `E:<name>#<meth>` that Proc's environment,
//! `F:<name>#<meth>` a method written in C, `V:<name>@<ivar>` the object a class keeps in an
//! instance variable or a constant, `G:$<name>` the object a global holds, `X:<what>` an
//! object the writer's VM makes up front. Nothing here keeps a table of those names: each one
//! is looked up by walking this VM's own classes the same way. A name that has nothing here is
//! an error naming it, and the image is refused.

use alloc::{format, string::String};

use crate::object::{Method, ObjKind};
use crate::value::{ObjId, Value};
use crate::vm::Vm;

type R<T> = Result<T, String>;

/// A class by constant path from `Object` (`Integer`, `Math::DomainError`), or by one of the
/// prefixed forms that name a class (`S:`, `I:`, `G:`).
fn class_by_name(vm: &mut Vm, name: &str) -> R<ObjId> {
    match object(vm, name)? {
        Value::Obj(o) if vm.heap.is_class(o) => Ok(o),
        _ => Err(format!("'{name}' is not a class here")),
    }
}

fn const_in(vm: &mut Vm, class: ObjId, name: &str) -> Option<Value> {
    let s = vm.syms.lookup_str(name)?;
    vm.heap.class(class).consts.get(&s).map(|v| v.get())
}

/// `<class>#<method>` split at the last `#` (a class path never has one, a method name never
/// has one either, and `S:` paths nest).
fn split_method(rest: &str) -> R<(&str, &str)> {
    rest.rfind('#').map(|i| (&rest[..i], &rest[i + 1..])).ok_or_else(|| format!("bad method name '{rest}'"))
}

/// The method `meth` of `class` as this VM has it: its own table first, then the chain above
/// it (where this VM defines it on an ancestor rather than on the class itself).
pub fn method(vm: &mut Vm, name: &str) -> R<Method> {
    let rest = name.strip_prefix("M:").or_else(|| name.strip_prefix("F:")).ok_or_else(|| format!("'{name}' is not a method name"))?;
    let (cname, mname) = split_method(rest)?;
    let class = class_by_name(vm, cname)?;
    let mid = vm.intern(mname);
    if let Some(m) = vm.heap.class(class).methods.get(&mid) { return Ok(m.clone()); }
    match vm.find_method(class, mid) {
        Some((m, _)) => Ok(m),
        None => Err(format!("method '{name}' does not exist here")),
    }
}

/// The object a permanent name stands for.
pub fn object(vm: &mut Vm, name: &str) -> R<Value> {
    if name == "T" { return Ok(Value::Obj(vm.top_self)); }
    if let Some(rest) = name.strip_prefix("S:") {
        let o = object(vm, rest)?;
        return vm.singleton_class(o).map(Value::Obj).map_err(|_| format!("'{name}': no singleton class here"));
    }
    if let Some(rest) = name.strip_prefix("I:") {
        let i = rest.rfind('/').ok_or_else(|| format!("bad name '{name}'"))?;
        let k: usize = rest[i + 1..].parse().map_err(|_| format!("bad name '{name}'"))?;
        let class = class_by_name(vm, &rest[..i])?;
        let mut c = vm.heap.class(class).superclass;
        let mut j = 0;
        while let Some(x) = c {
            if vm.heap.class(x).iclass_of.is_none() { break; }
            if j == k { return Ok(Value::Obj(x)); }
            j += 1;
            c = vm.heap.class(x).superclass;
        }
        return Err(format!("'{name}': no such include class here"));
    }
    if name.starts_with("M:") {
        return match method(vm, name)? {
            Method::Ruby(p) => Ok(Value::Obj(p)),
            _ => Err(format!("'{name}' is not a method written in Ruby here")),
        };
    }
    if let Some(rest) = name.strip_prefix("E:") {
        let p = match object(vm, &format!("M:{rest}"))? { Value::Obj(p) => p, _ => unreachable!() };
        return vm.heap.proc_data(p).env.map(Value::Obj).ok_or_else(|| format!("'{name}': that method has no environment here"));
    }
    if let Some(rest) = name.strip_prefix("V:") {
        let i = rest.find('@').ok_or_else(|| format!("bad name '{name}'"))?;
        let (cname, var) = (&rest[..i], &rest[i + 1..]);
        let class = class_by_name(vm, cname)?;
        if var == "__classname__" {
            // the writer keeps a nested class's path as a String in the class; here the name
            // is made from the class, so a String of it stands in (equal, not the same object)
            let n = vm.class_name(class);
            return Ok(vm.str_from(n));
        }
        let found = if var.starts_with("@@") {
            vm.syms.lookup_str(var).and_then(|s| vm.heap.class(class).cvars.get(&s).map(|v| v.get()))
        } else if var.starts_with('@') {
            vm.syms.lookup_str(var).map(|s| vm.heap.ivar_get(class, s))
        } else {
            const_in(vm, class, var)
        };
        return match found {
            Some(v @ Value::Obj(_)) => Ok(v),
            _ => Err(format!("'{name}' does not exist here")),
        };
    }
    if let Some(g) = name.strip_prefix("G:") {
        return match vm.syms.lookup_str(g).and_then(|s| vm.globals.get(&s).map(|v| v.get())) {
            Some(v @ Value::Obj(_)) => Ok(v),
            _ => Err(format!("global '{g}' does not exist here")),
        };
    }
    if name.starts_with("X:") || name.starts_with("F:") || name.is_empty() {
        return Err(format!("'{name}' has no counterpart here"));
    }
    // a class or module by constant path
    let mut cur = vm.core.object;
    for (i, seg) in name.split("::").enumerate() {
        match const_in(vm, cur, seg) {
            Some(Value::Obj(o)) if vm.heap.is_class(o) => cur = o,
            _ if i > 0 => return Err(format!("class '{name}' does not exist here (at its part '{seg}')")),
            _ => return Err(format!("class '{name}' does not exist here")),
        }
    }
    Ok(Value::Obj(cur))
}

/// Whether `name` resolves here, for the table of names a reader is checked against
/// (`sabiruby --checkpoint-names`): `Ok(kind)` says what it found.
pub fn check(vm: &mut Vm, name: &str) -> R<&'static str> {
    if name == "F:attr_reader" || name == "F:attr_writer" { return Ok("attribute method"); }
    if name.starts_with("F:") {
        return Ok(match method(vm, name)? {
            Method::Ruby(_) => "method written in Ruby here",
            Method::Undef => return Err(format!("method '{name}' is undefined here")),
            _ => "native method",
        });
    }
    let v = match object(vm, name) {
        Ok(v) => v,
        // a method written in Ruby there and natively here: a method table's entry binds to it,
        // a frame runs the carried irep, only a reference to the Proc itself is refused
        Err(_) if name.starts_with("M:") && matches!(method(vm, name), Ok(Method::Native(..) | Method::Closure(_) | Method::AttrReader(_) | Method::AttrWriter(_))) => {
            return Ok("native method here (bound as a method entry; the Proc as a value is refused)");
        }
        Err(e) => return Err(e),
    };
    Ok(match v.obj().map(|o| &vm.heap.get(o).kind) {
        Some(ObjKind::Class(_)) => "class",
        Some(ObjKind::Proc(_)) => "Proc",
        Some(ObjKind::Env(_)) => "env",
        _ => "object",
    })
}
