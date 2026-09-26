//! `require`/`load`: the three natives the Ruby part (`src/mrblib/require.rb`) is built on.
//! The reference has none of this — mruby has no `require` — so the shape follows
//! picoruby-require (`docs/plans/eval-require-plan.md` 5): the search is Ruby, and what it cannot do
//! without POSIX gems is here.
//!
//! Reading a file is the host's (`src/host.rs`, the same trait `eval` compiles through), so a
//! build with no host has no `require` either, and the VM stays `no_std`.

use alloc::{format, string::String};

use crate::argc;
use crate::error::VmResult;
use crate::host::EvalOptions;
use crate::value::Value;
use crate::vm::Vm;

/// `LoadError`, which the Ruby part defines.
fn load_error(vm: &mut Vm, msg: &str) -> crate::error::VmError {
    let n = vm.intern("LoadError");
    let cls = match vm.const_get(vm.core.object, n) {
        Some(Value::Obj(c)) => c,
        _ => vm.core.standard_error,
    };
    vm.raise(cls, msg)
}

fn path_arg(vm: &mut Vm, v: Value) -> VmResult<String> {
    let b = vm.expect_str(v, "path")?;
    Ok(String::from_utf8_lossy(&b).into_owned())
}

/// `__file_exist?(path)` — the host's `file_exists`.
fn file_exist_p(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    let path = path_arg(vm, a[0])?;
    let Some(mut host) = vm.host.take() else { return Ok(Value::False) };
    let found = host.file_exists(&path);
    vm.host = Some(host);
    Ok(Value::bool(found))
}

/// `__load_file(path)` — the host's `read_file`, or nil where there is none.
fn load_file(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    let path = path_arg(vm, a[0])?;
    let Some(mut host) = vm.host.take() else {
        return Err(vm.raise(vm.core.not_implemented_error, "require: no host installed (Vm::set_host)"));
    };
    let read = host.read_file(&path);
    vm.host = Some(host);
    match read {
        Some(bytes) => {
            let v = vm.str_new(&bytes);
            // what a file holds is bytes; only a program that reads them as characters says so
            if !crate::builtins::string::valid_chars(&bytes, cfg!(feature = "utf8")) {
                vm.str_set_binary(v, true);
            }
            Ok(v)
        }
        None => Ok(Value::Nil),
    }
}

/// `__exec_file(source, path)` — RITE bytecode as it stands, Ruby source through the host's
/// compiler, run in a top-level frame of its own (`Vm::run_irep`, the shape `mrb_top_run` gives
/// a loaded file: its own scope, `self` = main, no enclosing environment).
fn exec_file(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 2);
    let path = path_arg(vm, a[1])?;
    let Some(src) = vm.str_bytes(a[0]).map(|b| b.to_vec()) else {
        return Err(load_error(vm, &format!("cannot load such file -- {path}")));
    };
    let bin = if src.starts_with(b"RITE") {
        // a compiled file is run as it stands; a version this VM does not read is a LoadError
        // rather than the reader's own message, since `require` is where it comes up
        if src.get(4..8) != Some(b"0400") {
            let ver = String::from_utf8_lossy(src.get(4..8).unwrap_or(b"????")).into_owned();
            return Err(load_error(vm, &format!("invalid RITE version {ver} -- {path}")));
        }
        src
    } else {
        let Some(mut host) = vm.host.take() else {
            return Err(vm.raise(vm.core.not_implemented_error, "require: no host installed (Vm::set_host)"));
        };
        let opts = EvalOptions { filename: &path, line: 1, scopes: &[], debug_info: true };
        let r = host.compile(&src, &opts);
        vm.host = Some(host);
        match r {
            Ok(b) => b,
            Err(msg) => {
                let text = format!("file {path} line 1: {msg}");
                let e = vm.intern("SyntaxError");
                let cls = match vm.const_get(vm.core.object, e) { Some(Value::Obj(c)) => c, _ => vm.core.standard_error };
                return Err(vm.raise(cls, &text));
            }
        }
    };
    let irep = match vm.load_ireps(&bin) {
        Ok(i) => i,
        Err(e) => return Err(load_error(vm, &format!("{e} -- {path}"))),
    };
    vm.run_irep(irep)
}

pub fn init(vm: &mut Vm) {
    let k = vm.core.kernel;
    vm.define_method(k, "__file_exist?", file_exist_p);
    vm.define_method(k, "__load_file", load_file);
    vm.define_method(k, "__exec_file", exec_file);
    for name in ["__file_exist?", "__load_file", "__exec_file"] {
        let n = vm.intern(name);
        let _ = vm.set_visibility(k, n, crate::object::Vis::Private);
    }
}
