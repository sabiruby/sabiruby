//! The `Checkpoint` module a restored program finds again: `save`, `suspend`, `resumed?`,
//! `Checkpoint::Error`, and `Checkpoint::Box` (the writer's example of a C data type that
//! travels through its side table of dump and restore functions).
//!
//! This reader does not write images (the direction is one way for now), so `save` and
//! `suspend` here write nothing and answer what the side that goes on sees: `save` answers
//! `:saved`, `suspend` answers `:resumed`. That is the writer's reference run (`--ignore`), so
//! a program that suspends twice runs on to its end here after the first image.

use alloc::{format, string::String, vec::Vec};

use crate::error::VmResult;
use crate::object::Aspec;
use crate::value::{ObjId, Value};
use crate::vm::{Vm, ROOT};

fn module(vm: &mut Vm) -> Option<ObjId> {
    let s = vm.syms.lookup_str("Checkpoint")?;
    match vm.heap.class(vm.core.object).consts.get(&s).map(|v| v.get()) {
        Some(Value::Obj(o)) if vm.heap.is_class(o) => Some(o),
        _ => None,
    }
}

fn under(vm: &mut Vm, name: &str) -> Option<ObjId> {
    let m = module(vm)?;
    let s = vm.syms.lookup_str(name)?;
    match vm.heap.class(m).consts.get(&s).map(|v| v.get()) {
        Some(Value::Obj(o)) if vm.heap.is_class(o) => Some(o),
        _ => None,
    }
}

/// `Checkpoint::Error` with `msg`.
pub(crate) fn error(vm: &mut Vm, msg: &str) -> crate::error::VmError {
    let c = under(vm, "Error").unwrap_or(vm.core.runtime_error);
    vm.raise(c, msg)
}

/// What the writer refuses where the call is made, whatever happens next.
fn request(vm: &mut Vm, what: &str, answer: &str) -> VmResult<Value> {
    if vm.cur != ROOT {
        return Err(error(vm, &format!("{what} from a Fiber or Task is not supported in format v0")));
    }
    let s = vm.intern(answer);
    Ok(Value::Sym(s))
}

fn box_tag(vm: &mut Vm) -> u32 {
    let m = module(vm).expect("Checkpoint");
    let k = vm.intern("__box_tag");
    match vm.heap.ivar_get(m, k) { Value::Int(t) => t as u32, _ => 0 }
}

fn box_value(vm: &mut Vm, s: Value) -> VmResult<i64> {
    let tag = box_tag(vm);
    match vm.data_of(s) {
        Some((t, h)) if t == tag => Ok(h as i64),
        _ => Err(vm.raise_type("not a Checkpoint::Box")),
    }
}

/// Defines the module, once. A VM that never resumes an image does not have it.
pub fn install(vm: &mut Vm) {
    if module(vm).is_some() { return; }
    let m = vm.define_module("Checkpoint");
    let se = vm.core.standard_error;
    let o = vm.core.object;
    vm.define_class_under(m, "Error", se);
    let _ = vm.define_module_function_args(m, "save", Aspec::opt(1), |vm, _s, _a, _b| request(vm, "Checkpoint.save", "saved"));
    let _ = vm.define_module_function_args(m, "suspend", Aspec::opt(1), |vm, _s, _a, _b| request(vm, "Checkpoint.suspend", "resumed"));
    let _ = vm.define_module_function_args(m, "resumed?", Aspec::none(), |vm, _s, _a, _b| {
        let m = module(vm).expect("Checkpoint");
        let k = vm.intern("__resumed");
        Ok(Value::bool(vm.heap.ivar_get(m, k).truthy()))
    });
    // Checkpoint::Box: the value is the handle of a Data object
    let bx = vm.define_class_under(m, "Box", o);
    let tag = vm.next_data_tag();
    let k = vm.intern("__box_tag");
    vm.heap.ivar_set(m, k, Value::Int(tag as i64));
    let sc = vm.singleton_class(Value::Obj(bx)).expect("metaclass");
    vm.define_method_args(sc, "new", Aspec::opt(1), |vm, s, a, _b| {
        let v = match a.first() { Some(v) => vm.expect_int(*v, "value")?, None => 0 };
        let tag = box_tag(vm);
        Ok(vm.data_new(s.obj().unwrap(), tag, v as u64))
    });
    vm.define_method_args(bx, "value", Aspec::none(), |vm, s, _a, _b| Ok(Value::Int(box_value(vm, s)?)));
    vm.define_method_args(bx, "value=", Aspec::req(1), |vm, s, a, _b| {
        box_value(vm, s)?;
        let v = vm.expect_int(a[0], "value")?;
        if let (Some(o), Some((tag, _))) = (s.obj(), vm.data_of(s)) {
            vm.heap.get_mut(o).kind = crate::object::ObjKind::Data { tag, handle: v as u64 };
        }
        Ok(a[0])
    });
}

/// Marks the state as restored (`Checkpoint.resumed?`).
pub(crate) fn set_resumed(vm: &mut Vm) {
    if let Some(m) = module(vm) {
        let k = vm.intern("__resumed");
        vm.heap.ivar_set(m, k, Value::True);
    }
}

/// The reader's half of the writer's side table for C data: the type name the image gives
/// and the bytes its dump function wrote, made into an object of `class` here.
pub(crate) fn restore_data(vm: &mut Vm, type_name: &str, blob: &[u8], class: ObjId) -> Result<Value, String> {
    match type_name {
        "Checkpoint::Box" => {
            let mut u: u64 = 0;
            for b in blob.iter().take(8) { u = (u << 8) | *b as u64; }
            let tag = box_tag(vm);
            Ok(vm.data_new(class, tag, u))
        }
        // mruby-time's entry (the writer's checkpoint_bundled.c): seconds and nanoseconds
        // since the epoch (int64, big-endian) and the zone (1 UTC, 2 local). A Time here is a
        // plain object with hidden ivars (builtins/ext_time.rs).
        "Time" => {
            if blob.len() < 17 { return Err(String::from("a Time image record is too short")); }
            let rd = |b: &[u8]| { let mut u: u64 = 0; for x in b { u = (u << 8) | *x as u64; } u as i64 };
            let (sec, nsec, utc) = (rd(&blob[0..8]), rd(&blob[8..16]), blob[16] == 1);
            let o = vm.heap.alloc(class, crate::object::ObjKind::Object);
            let (ks, kn, ku) = (vm.intern("__sec"), vm.intern("__nsec"), vm.intern("__utc"));
            vm.heap.ivar_set(o, ks, Value::Int(sec));
            vm.heap.ivar_set(o, kn, Value::Int(nsec));
            vm.heap.ivar_set(o, ku, Value::bool(utc));
            Ok(Value::Obj(o))
        }
        _ => Err(format!("C data type '{type_name}' has no restore function here")),
    }
}

/// The C data type names an image may carry that this reader restores.
pub fn data_types() -> Vec<&'static str> {
    alloc::vec!["Checkpoint::Box", "Time"]
}
