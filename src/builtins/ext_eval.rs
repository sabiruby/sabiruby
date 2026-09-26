//! mruby-eval (`mrbgems/mruby-eval/src/eval.c`): `Kernel#eval`, and the string forms of
//! `BasicObject#instance_eval` and `Module#class_eval`/`module_eval`. No Ruby part.
//!
//! The reference compiles the string with the compiler linked into the VM, handing it the
//! caller's `RProc` chain so that the code generator can read the enclosing local variable
//! tables. The VM crate here is pure Rust and has no compiler, so the chain is turned into a
//! table of names and handed to the host (`src/host.rs`, `docs/plans/eval-require-plan.md`); the
//! compiler crate's own `Host` passes it to the same two places of the code generator
//! (`SABIRUBY_EVAL_SCOPES`, `compiler/vendor/VENDOR.md`).
//!
//! What makes the string see the caller's variables is the Proc it becomes: its environment
//! **is** the caller frame's environment and its `upper` is the caller's Proc, so the
//! `GETUPVAR`/`SETUPVAR` the compiler emits with depth 0 read and write the caller's
//! registers, exactly as in the reference.

use alloc::{format, string::String, vec, vec::Vec};

use crate::argc;
use crate::error::VmResult;
use crate::host::EvalOptions;
use crate::object::{ObjKind, ProcData};
use crate::value::{ObjId, Value};
use crate::vm::Vm;

/// The enclosing local variable names, caller first (`mrc_pm_options_init`'s walk over the
/// `RProc` chain). A hole (an unnamed `*`/`&` parameter) keeps its place as an empty name so
/// that a position stays a register index.
fn scopes_of(vm: &Vm, proc_: Option<ObjId>) -> Vec<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    let mut p = proc_;
    while let Some(pid) = p {
        let pd = vm.heap.proc_data(pid);
        let irep = &vm.ireps[pd.irep];
        let mut names = Vec::new();
        for i in 0..irep.nlocals.saturating_sub(1) {
            match irep.lv.get(i) {
                Some(Some(s)) => names.push(vm.syms.name(*s).to_vec()),
                _ => names.push(Vec::new()),
            }
        }
        out.push(names);
        if pd.scope { break; }
        p = pd.upper;
    }
    out
}

/// Compiles a string in the scope of `proc_` (its local variable names and those of the
/// scopes around it).
fn compile_with(vm: &mut Vm, src: &[u8], filename: &str, line: u32, proc_: ObjId) -> VmResult<Vec<u8>> {
    let scopes = scopes_of(vm, Some(proc_));
    let mut host = match vm.host.take() { Some(h) => h, None => return Err(vm.raise(vm.core.not_implemented_error, "eval: no compiler installed (Vm::set_host)")) };
    let opts = EvalOptions { filename, line, scopes: &scopes, debug_info: true };
    let r = host.compile(src, &opts);
    vm.host = Some(host);
    match r {
        Ok(b) => Ok(b),
        Err(msg) => {
            // the reference's wording: `file (eval) line 1: <message>`
            let text = format!("file {filename} line {line}: {msg}");
            let e = vm.intern("SyntaxError");
            let cls = match vm.const_get(vm.core.object, e) { Some(Value::Obj(c)) => c, _ => vm.core.standard_error };
            Err(vm.raise(cls, &text))
        }
    }
}

/// Compiles the string and makes the Proc that runs it in the caller's scope.
fn eval_proc(vm: &mut Vm, src: &[u8], file: Option<String>, line: u32) -> VmResult<ObjId> {
    let ci = match vm.ci.last() { Some(ci) => ci, None => return Err(vm.raise(vm.core.runtime_error, "eval outside of a frame")) };
    let (caller_proc, target_class) = (ci.proc_, ci.target_class);
    let filename = file.unwrap_or_else(|| String::from("(eval)"));
    let bin = compile_with(vm, src, &filename, line, caller_proc)?;
    let irep = vm.load(&bin)?;
    // the caller frame's environment, made now if it has none (`mrb_vm_ci_env` / `mrb_env_new`)
    let env = vm.caller_env();
    let p = vm.heap.alloc(vm.core.proc_, ObjKind::Proc(ProcData {
        irep, upper: Some(caller_proc), env: Some(env), target_class: Some(target_class),
        strict: false, scope: false, orphan: false, mid: None,
    }));
    Ok(p)
}

/// `mrb_load_string` called from a native while the VM runs, the way an embedding host does
/// (`mrbgems/mruby-regexp/test/backref_scope.c`). The string is compiled on its own and run in a
/// frame whose proc captured no scope, which is the frame `$~` resolution steps over.
pub(crate) fn top_load(vm: &mut Vm, src: &[u8], self_: Value) -> VmResult<Value> {
    let scopes: Vec<Vec<Vec<u8>>> = Vec::new();
    let mut host = match vm.host.take() { Some(h) => h, None => return Err(vm.raise(vm.core.not_implemented_error, "load: no compiler installed (Vm::set_host)")) };
    let opts = EvalOptions { filename: "(eval)", line: 1, scopes: &scopes, debug_info: true };
    let r = host.compile(src, &opts);
    vm.host = Some(host);
    let bin = match r {
        Ok(b) => b,
        Err(msg) => {
            let text = format!("file (eval) line 1: {msg}");
            let e = vm.intern("SyntaxError");
            let cls = match vm.const_get(vm.core.object, e) { Some(Value::Obj(c)) => c, _ => vm.core.standard_error };
            return Err(vm.raise(cls, &text));
        }
    };
    let irep = vm.load(&bin)?;
    let p = vm.heap.alloc(vm.core.proc_, ObjKind::Proc(ProcData {
        irep, upper: None, env: None, target_class: Some(vm.core.object),
        strict: false, scope: false, orphan: false, mid: None,
    }));
    // a host's load is a nested run (`mrb_load_string` from C), not a frame of the caller's
    vm.run_eval_nested(p, self_, None)
}

/// The `line` and `file` arguments of `eval`/`instance_eval`, with the reference's checks.
fn file_line(vm: &mut Vm, a: &[Value], at: usize) -> VmResult<(Option<String>, u32)> {
    let file = match a.get(at) {
        None | Some(Value::Nil) => None,
        Some(v) => match vm.str_bytes(*v) {
            Some(b) => Some(String::from_utf8_lossy(b).into_owned()),
            None => { let d = vm.describe_for_type_error(*v); return Err(vm.raise_type(&format!("{d} cannot be converted to String"))); }
        },
    };
    let line = match a.get(at + 1) { None => 1, Some(v) => vm.expect_int(*v, "line")? };
    Ok((file, line.max(0) as u32))
}

fn str_arg(vm: &mut Vm, v: Value) -> VmResult<Vec<u8>> {
    match vm.str_bytes(v) {
        Some(b) => Ok(b.to_vec()),
        None => { let d = vm.describe_for_error(v); Err(vm.raise_type(&format!("wrong argument type {d} (expected String)"))) }
    }
}

fn kernel_eval(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1, 4);
    let src = str_arg(vm, a[0])?;
    let binding = match a.get(1) {
        None | Some(Value::Nil) => None,
        Some(b) if super::ext_binding::is_binding(vm, *b) => Some(*b),
        Some(b) => { let d = vm.describe_for_error(*b); return Err(vm.raise_type(&format!("wrong argument type {d} (expected binding)"))); }
    };
    let (file, line) = file_line(vm, a, 2)?;
    match binding {
        Some(b) => eval_in_binding(vm, b, &src, file, line),
        None => { let p = eval_proc(vm, &src, file, line)?; vm.run_eval(p, s, None) }
    }
}

/// `Binding#eval(string, file = nil, line = 1)`, which the reference spells as
/// `eval(string, self, file, line)`.
fn binding_eval(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1, 3);
    let src = str_arg(vm, a[0])?;
    let (file, line) = file_line(vm, a, 1)?;
    eval_in_binding(vm, s, &src, file, line)
}

/// The string runs in the binding's scope, and the variables it defines at its top level
/// stay in the binding (`binding_eval_prepare` / `expand_lvspace`). Those names are found by
/// compiling once and reading the local variable table of the result: what the string
/// defines itself is exactly what did not resolve to an enclosing scope. The compilation
/// that runs is the second one, with the names merged, so an assignment writes into the
/// binding's own space and the next `eval` sees it.
fn eval_in_binding(vm: &mut Vm, binding: Value, src: &[u8], file: Option<String>, line: u32) -> VmResult<Value> {
    use super::ext_binding as bind;
    let (bproc, benv) = bind::parts(vm, binding)?;
    let filename = file.unwrap_or_else(|| String::from("(eval)"));
    let first = compile_with(vm, src, &filename, line, bproc)?;
    for name in top_locals(vm, &first) {
        if bind::search(vm, bproc, benv, name).is_none() {
            bind::merge_lvar(vm, bproc, benv, name, Value::Nil);
        }
    }
    let bin = compile_with(vm, src, &filename, line, bproc)?;
    let irep = vm.load(&bin)?;
    let target_class = vm.heap.env(benv).target_class.or(Some(vm.core.object));
    let p = vm.heap.alloc(vm.core.proc_, ObjKind::Proc(ProcData {
        irep, upper: Some(bproc), env: Some(benv), target_class,
        strict: false, scope: false, orphan: false, mid: None,
    }));
    let recv = binding.obj().map(|o| vm.heap.ivar_get(o, vm.s.brecv)).unwrap_or(Value::Nil);
    vm.run_eval(p, recv, None)
}

/// The names of the local variables of a compiled string's own top scope.
fn top_locals(vm: &mut Vm, bin: &[u8]) -> Vec<crate::symbol::Sym> {
    let rite = match crate::rite::parse(bin) { Ok(r) => r, Err(_) => return Vec::new() };
    let top = match rite.ireps.first() { Some(t) => t, None => return Vec::new() };
    let mut out = Vec::new();
    for n in top.lv.iter().flatten() {
        if n.first().map(|c| *c == b'*' || *c == b'&').unwrap_or(true) { continue; }
        out.push(vm.syms.intern(n));
    }
    out
}

/// `BasicObject#instance_eval(string)`: the receiver's singleton class is the target class.
fn instance_eval(vm: &mut Vm, s: Value, a: &[Value], b: Value) -> VmResult<Value> {
    // `object_eval`: with a block the form takes no argument at all
    if !b.is_nil() {
        argc!(vm, a, 0);
        return vm.exec_block_with_self(b, s, &[s], None);
    }
    if a.is_empty() { return Err(vm.raise_arg("no block given")); }
    argc!(vm, a, 1, 3);
    let src = str_arg(vm, a[0])?;
    let (file, line) = file_line(vm, a, 1)?;
    let p = eval_proc(vm, &src, file, line)?;
    let tc = match s {
        Value::Int(_) | Value::Float(_) | Value::Sym(_) => None,
        _ => Some(vm.singleton_class(s)?),
    };
    vm.run_eval(p, s, tc)
}

/// `Module#class_eval(string)` / `module_eval`: the module is the target class.
fn class_eval(vm: &mut Vm, s: Value, a: &[Value], b: Value) -> VmResult<Value> {
    if !b.is_nil() {
        argc!(vm, a, 0);
        return vm.exec_block_with_self(b, s, &[s], None);
    }
    if a.is_empty() { return Err(vm.raise_arg("no block given")); }
    argc!(vm, a, 1, 3);
    let src = str_arg(vm, a[0])?;
    let (file, line) = file_line(vm, a, 1)?;
    let p = eval_proc(vm, &src, file, line)?;
    let c = match s { Value::Obj(o) if vm.heap.is_class(o) => o, _ => { let d = vm.describe_for_error(s); return Err(vm.raise_type(&format!("{d} is not a class/module"))); } };
    vm.run_eval(p, s, Some(c))
}

pub fn init(vm: &mut Vm) {
    let k = vm.core.kernel;
    // `mrb_define_module_function_id(..., MRB_SYM(eval), ...)` (eval.c)
    vm.define_module_function(k, "eval", kernel_eval).expect("Kernel singleton");
    vm.define_method(vm.core.basic_object, "instance_eval", instance_eval);
    vm.define_methods(vm.core.module, &[("class_eval", class_eval), ("module_eval", class_eval)]);
    // `Binding#eval` belongs to this gem: it needs the compiler (`mrb_binding_eval`)
    let bn = vm.intern("Binding");
    if let Some(Value::Obj(bc)) = vm.const_get(vm.core.object, bn) { vm.define_method(bc, "eval", binding_eval); }
    let _ = vec![0u8];
}
