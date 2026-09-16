//! Kernel: I/O, raise, block_given?, conversions.

use alloc::{string::String, vec, vec::Vec};

use crate::argc;
use crate::error::{VmError, VmResult};
use crate::object::ObjKind;
use crate::value::Value;
use crate::vm::Vm;

pub fn init(vm: &mut Vm) {
    let k = vm.core.kernel;
    vm.define_methods(k, &[
        // `mrb_obj_init_copy`: called on every dup/clone; Module has its own
        ("initialize_copy", |vm, s, a, _b| { if a.len() != 1 { return Err(vm.argnum_error(a.len(), "1")); } if s == a[0] { return Ok(s); } if vm.real_class_of(s) != vm.real_class_of(a[0]) || core::mem::discriminant(&s) != core::mem::discriminant(&a[0]) { return Err(vm.raise_type("initialize_copy should take same class object")); } Ok(s) }),
        // `mrb_obj_cmp`: 0 for the same object or `==`, nil otherwise
        ("<=>", |vm, s, a, _b| { if a.len() != 1 { return Err(vm.argnum_error(a.len(), "1")); } if s == a[0] || vm.equal(s, a[0])? { return Ok(Value::Int(0)); } Ok(Value::Nil) }),
        ("puts", puts),
        ("print", print),
        ("printf", printf),
        ("putc", putc),
        ("p", p),
        ("raise", raise),
        ("block_given?", block_given),
        ("lambda", |vm, _s, _a, b| make_proc(vm, b, true)),
        ("proc", |vm, _s, _a, b| make_proc(vm, b, false)),
        ("__id__", |vm, s, a, _b| { argc!(vm, a, 0); Ok(super::object::object_id(s)) }),
        ("object_id", |vm, s, a, _b| { argc!(vm, a, 0); Ok(super::object::object_id(s)) }),
        ("iterator?", block_given),
        ("global_variables", |vm, _s, _a, _b| { let l: Vec<Value> = vm.globals.keys().map(|k| Value::Sym(*k)).collect(); Ok(vm.ary_new(l)) }),
        ("local_variables", |vm, _s, _a, _b| Ok(vm.ary_new(vec![]))),
        ("instance_variable_names", |vm, s, _a, _b| { let names: Vec<Value> = match s { Value::Obj(o) => vm.heap.get(o).ivars.iter().map(|(k, _)| Value::Sym(*k)).collect(), _ => vec![] }; Ok(vm.ary_new(names)) }),
        // the encoding the build has (`MRB_UTF8_STRING` in the reference, the feature `utf8` here)
        ("__ENCODING__", |vm, _s, _a, _b| Ok(vm.str_new(if cfg!(feature = "utf8") { b"UTF-8".as_slice() } else { b"ASCII-8BIT".as_slice() }))),
        // `case`/`when` with a splat: `when *list` compiles to `__case_eqq`
        ("__case_eqq", |vm, s, a, _b| {
            argc!(vm, a, 1);
            let eqq = vm.s.eqq;
            if s.is_nil() { return Ok(Value::False); }
            let to_a = vm.intern("to_a");
            let list = if vm.ary(s).is_some() { s } else if !vm.respond_to(s, to_a) { return Ok(Value::bool(vm.funcall(s, eqq, &[a[0]], Value::Nil)?.truthy())); } else {
                let r = vm.funcall(s, to_a, &[], Value::Nil)?;
                if r.is_nil() { return vm.funcall(s, eqq, &[a[0]], Value::Nil); }
                r
            };
            for it in vm.ary_vals(list).unwrap_or_default() {
                if vm.funcall(it, eqq, &[a[0]], Value::Nil)?.truthy() { return Ok(Value::True); }
            }
            Ok(Value::False)
        }),
        ("__printstr__", |vm, _s, a, _b| { for v in a { let b = vm.as_string(*v)?; vm.write_out(&b); } Ok(Value::Nil) }),
        ("!~", |vm, s, a, _b| { argc!(vm, a, 1); let m = vm.intern("=~"); let r = vm.funcall(s, m, &[a[0]], Value::Nil)?; Ok(Value::bool(!r.truthy())) }),
    ]);
    // `MRB_MT_PRIVATE` in `krn_rom_entries` (src/kernel.c) for the entry written above
    vm.mark_private(k, &["initialize_copy"]);
    // the other MRB_MT_PRIVATE entries of the same table
    vm.define_private_methods(k, &[
        // `defined?` is compiled to these (kernel.c); they answer the description string or nil
        ("__defined_ivar?", |vm, s, a, _b| { argc!(vm, a, 1); let n = super::object::sym_arg(vm, a[0])?; let ok = match s { Value::Obj(o) => vm.heap.get(o).ivars.iter().any(|(k, _)| *k == n), _ => false }; Ok(defined_str(vm, ok, "instance-variable")) }),
        ("__defined_gvar?", |vm, _s, a, _b| { argc!(vm, a, 1); let n = super::object::sym_arg(vm, a[0])?; let ok = vm.globals.contains_key(&n) || Some(n) == vm.s.backref; Ok(defined_str(vm, ok, "global-variable")) }),
        ("__defined_cvar?", |vm, _s, a, _b| { argc!(vm, a, 1); let n = super::object::sym_arg(vm, a[0])?; let ci = *vm.ci.last().unwrap(); let cls = vm.cvar_class_of(ci.proc_); let ok = vm.cvar_lookup(cls, n).is_some(); Ok(defined_str(vm, ok, "class variable")) }),
        ("__defined_const?", |vm, _s, a, _b| { argc!(vm, a, 1); let n = super::object::sym_arg(vm, a[0])?; let ci = *vm.ci.last().unwrap(); let ok = vm.const_lookup_noraise(&ci, n).is_some(); Ok(defined_str(vm, ok, "constant")) }),
        ("__defined_const_path?", |vm, _s, a, _b| { argc!(vm, a, 2); let (p, c) = (super::object::sym_arg(vm, a[0])?, super::object::sym_arg(vm, a[1])?); let ci = *vm.ci.last().unwrap(); let ok = match vm.const_lookup_noraise(&ci, p) { Some(Value::Obj(o)) if vm.heap.is_class(o) => vm.const_get(o, c).is_some(), _ => false }; Ok(defined_str(vm, ok, "constant")) }),
        ("__defined_method?", |vm, s, a, _b| { argc!(vm, a, 1); let n = super::object::sym_arg(vm, a[0])?; let ok = vm.respond_to(s, n); Ok(defined_str(vm, ok, "method")) }),
        ("__defined_yield?", |vm, s, a, b| { let r = block_given(vm, s, a, b)?; Ok(defined_str(vm, r.truthy(), "yield")) }),
        ("__defined_super?", |vm, _s, _a, _b| { let ci = *vm.ci.last().unwrap(); let ok = match ci.mid { Some(m) => match vm.heap.class(ci.target_class).superclass { Some(sup) => vm.find_method(sup, m).is_some(), None => false }, None => false }; Ok(defined_str(vm, ok, "super")) }),
    ]);
    // module functions: callable as Kernel.raise, private as instance methods
    // (`mrb_define_module_function`, kernel.c)
    for name in ["raise", "block_given?", "iterator?", "p", "print", "printf", "putc", "puts", "lambda", "proc", "__printstr__"] {
        let _ = vm.make_module_function(k, name);
    }
}

fn puts_value(vm: &mut Vm, v: Value, depth: usize) -> VmResult<()> {
    if let Some(items) = vm.ary_vals(v) {
        if items.is_empty() && depth == 0 { vm.write_out(b"\n"); }
        for it in items { puts_value(vm, it, depth + 1)?; }
        return Ok(());
    }
    let mut b = vm.as_string(v)?;
    if b.last() != Some(&b'\n') { b.push(b'\n'); }
    vm.write_out(&b);
    Ok(())
}

fn puts(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    if a.is_empty() { vm.write_out(b"\n"); }
    for v in a { puts_value(vm, *v, 0)?; }
    Ok(Value::Nil)
}

fn print(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    for v in a { let b = vm.as_string(*v)?; vm.write_out(&b); }
    Ok(Value::Nil)
}

/// `Kernel#printf`. The reference has it in mruby-io (`mrblib/kernel.rb`): `$stdout.printf(...)`,
/// and `IO#printf` is `write sprintf(*args)`. There is no IO here, so it writes where `print`
/// writes and answers nil, which is what the reference answers (`IO#write`'s count does not come
/// back out of `Kernel#printf`). The one deviation is that the reference reaches `sprintf` as a
/// Ruby call and so would see it redefined; this one shares the formatter itself.
fn printf(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    let (out, _binary) = super::ext_sprintf::sprintf_bytes(vm, a)?;
    vm.write_out(&out);
    Ok(Value::Nil)
}

/// `Kernel#putc` (mruby-io `mrblib/kernel.rb`: `$stdout.putc(c); nil`, so nil and not the
/// argument `IO#putc` answers). An Integer writes the one byte `c & 0xff`; anything else is made
/// a String (`mrb_obj_as_string`) and the *first character* of it goes out — one byte in a
/// byte-read build, the whole UTF-8 sequence in a character-read one (`io_putc` reads
/// `mrb_utf8len` under `MRB_UTF8_STRING` without asking whether the string itself is binary,
/// which was checked against the reference: `putc("\u2192".b)` writes three bytes there too).
fn putc(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    if let Value::Int(i) = a[0] {
        vm.write_out(&[(i & 0xff) as u8]);
        return Ok(Value::Nil);
    }
    let b = vm.as_string(a[0])?;
    if !b.is_empty() {
        let n = super::string::utf8len(&b, 0, super::string::UTF8);
        vm.write_out(&b[..n]);
    }
    Ok(Value::Nil)
}

fn p(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    for v in a {
        let mut b = vm.inspect(*v)?;
        b.push(b'\n');
        vm.write_out(&b);
    }
    match a.len() {
        0 => Ok(Value::Nil),
        1 => Ok(a[0]),
        _ => Ok(vm.ary_new(a.to_vec())),
    }
}

/// `mrb_f_block_given_p_m`: the block of the enclosing *method* frame, found by
/// walking the caller's proc chain up to the first scope proc.
fn block_given(vm: &mut Vm, _s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let ci = match vm.ci.last() { Some(c) => *c, None => return Ok(Value::False) };
    let mut p = Some(ci.proc_);
    let mut e = None;
    while let Some(pid) = p {
        let pd = vm.heap.proc_data(pid);
        if pd.scope { break; }
        e = pd.env;
        p = pd.upper;
    }
    let p = match p { Some(p) => p, None => return Ok(Value::False) };
    let blk = if let Some(e) = e {
        if vm.heap.env(e).mid.is_none() { return Ok(Value::False); } // top level / class body
        let bidx = vm.heap.env(e).bidx;
        if bidx >= vm.heap.env(e).len && !vm.heap.env(e).attached { return Ok(Value::False); }
        vm.env_value(e, bidx)
    } else {
        // the frame running `p` itself (a top-level or class-body frame has no block)
        match vm.ci.iter().rev().find(|c| c.proc_ == p) {
            Some(c) if c.mid.is_some() => { let b = Vm::frame_bidx(c); vm.stack.get(c.base + b).map(|s| s.get()).unwrap_or(Value::Nil) }
            _ => return Ok(Value::False),
        }
    };
    Ok(Value::bool(!blk.is_nil()))
}

fn defined_str(vm: &mut Vm, ok: bool, s: &str) -> Value { if ok { vm.str_new(s.as_bytes()) } else { Value::Nil } }

/// `raise`, `raise "msg"`, `raise Class`, `raise Class, "msg"`, `raise exc`.
pub(crate) fn raise(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 0, 3);
    let exc = match a.len() {
        // mruby 4.1.0-rc: a bare `raise` is RuntimeError with an empty message (verified, not a re-raise)
        0 => vm.exc_new(vm.core.runtime_error, ""),
        _ => {
            if let Some(msg) = vm.str_bytes(a[0]).map(|b| b.to_vec()) {
                if a.len() > 1 { return Err(vm.raise_type("exception class/object expected")); }
                let s = String::from_utf8_lossy(&msg).into_owned();
                vm.exc_new(vm.core.runtime_error, &s)
            } else {
                let exception = vm.intern("exception");
                if !vm.respond_to(a[0], exception) { return Err(vm.raise_type("exception class/object expected")); }
                let rest = &a[1..a.len().min(2)];
                let e = vm.funcall(a[0], exception, rest, Value::Nil)?;
                if !vm.obj_is_kind_of(e, vm.core.exception) { return Err(vm.raise_type("exception object expected")); }
                e
            }
        }
    };
    Err(VmError::Raise(exc))
}

// `Integer()`, `Float()`, `String()`, `Array()`, `Hash()`, `__method__` are mruby-kernel-ext (`ext_kernel.rs`)

fn make_proc(vm: &mut Vm, b: Value, lambda: bool) -> VmResult<Value> {
    match b {
        Value::Obj(o) if matches!(vm.heap.get(o).kind, ObjKind::Proc(_)) => {
            if lambda { if let ObjKind::Proc(pd) = &mut vm.heap.get_mut(o).kind { pd.strict = true; } }
            Ok(b)
        }
        _ => Err(vm.raise_arg("tried to create Proc object without a block")),
    }
}

