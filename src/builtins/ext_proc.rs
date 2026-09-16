//! mruby-proc-ext (`mrbgems/mruby-proc-ext/src/proc.c`); its Ruby part
//! (`curry`, `<<`, `>>`, `===`, `yield`, `to_proc`) is `src/mrblib/proc-ext.mrb`.

use alloc::{format, string::String, vec, vec::Vec};

use crate::error::VmResult;
use crate::object::ObjKind;
use crate::value::{ObjId, Value};
use crate::vm::Vm;

/// `mrb_proc_parameters` for a Ruby proc: read off the ENTER operand and the
/// local variable names of the irep.
pub(crate) fn parameters_of(vm: &mut Vm, p: ObjId) -> Value {
    let (irep, strict) = { let pd = vm.heap.proc_data(p); (pd.irep, pd.strict) };
    let ir = &vm.ireps[irep];
    if ir.lv.is_empty() || ir.iseq.first() != Some(&(crate::opcode::Op::Enter as u8)) || ir.iseq.len() < 4 { return vm.ary_new(vec![]); }
    let a = ((ir.iseq[1] as u32) << 16) | ((ir.iseq[2] as u32) << 8) | ir.iseq[3] as u32;
    let req = (a >> 18) & 0x1f;
    let opt = (a >> 13) & 0x1f;
    let rest = (a >> 12) & 1;
    let post = (a >> 7) & 0x1f;
    let key = (a >> 2) & 0x1f;
    let kdict = (a >> 1) & 1;
    let block = a & 1;
    let lv: Vec<Option<crate::symbol::Sym>> = ir.lv.clone();
    let (s_req, s_opt, s_rest, s_keyrest, s_block, s_key) = (vm.intern("req"), vm.intern("opt"), vm.intern("rest"), vm.intern("keyrest"), vm.intern("block"), vm.intern("key"));
    // (kind, count) in the reference's order; non-strict procs report req as opt
    let kinds = [
        (if strict { s_req } else { s_opt }, req), (s_opt, opt), (s_rest, rest), (if strict { s_req } else { s_opt }, post),
        (s_keyrest, kdict), (s_block, block), (s_key, key),
    ];
    let max: u32 = kinds.iter().map(|(_, n)| *n).sum();
    let mut out = vec![];
    let mut krest = None;
    let mut blk = None;
    let mut i: usize = 0;
    for (kind, n) in kinds {
        for _ in 0..n {
            let mut e = vec![Value::Sym(kind)];
            if (i as u32) < max { if let Some(Some(s)) = lv.get(i) { e.push(Value::Sym(*s)); } }
            if kind == s_block {
                if let Some(Some(s)) = lv.get(i + 1) { e.push(Value::Sym(*s)); }
                blk = Some(vm.ary_new(e)); i += 1; continue;
            }
            if kind == s_keyrest { krest = Some(vm.ary_new(e)); i += 1; continue; }
            let av = vm.ary_new(e);
            out.push(av);
            i += 1;
        }
        if n == 0 && kind == s_block { i += 1; }
    }
    if let Some(k) = krest { out.push(k); }
    if let Some(b) = blk { out.push(b); }
    vm.ary_new(out)
}

fn proc_inspect(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let base = super::object::any_to_s(vm, s);
    let strict = s.obj().map(|o| vm.heap.proc_data(o).strict).unwrap_or(false);
    let body = &base[..base.len() - 1];
    // where the body begins, or `-:-` where the build kept no line numbers (`proc_inspect`)
    let where_ = match s.obj().map(|o| source_location_of(vm, o)) {
        Some(Value::Obj(a)) => match &vm.heap.get(a).kind {
            ObjKind::Array(v) if v.len() == 2 => {
                let file = vm.str_bytes(v[0].get()).map(|b| String::from_utf8_lossy(b).into_owned());
                match (file, v[1].get()) {
                    (Some(f), Value::Int(l)) => format!("{f}:{l}"),
                    _ => String::from("-:-"),
                }
            }
            _ => String::from("-:-"),
        },
        _ => String::from("-:-"),
    };
    let text = format!("{body} {where_}{}>", if strict { " (lambda)" } else { "" });
    Ok(vm.str_from(text))
}

/// `[filename, line]` of where a Proc's body begins, or nil where the build kept no debug
/// information (`mrb_proc_source_location`). A native has no irep and answers nil there; here a
/// Proc is always an irep, and an alias shares the irep of the method it was made from, so the
/// reference's walk over `upper` has nothing to do.
pub fn source_location_of(vm: &mut Vm, p: ObjId) -> Value {
    if !matches!(vm.heap.get(p).kind, ObjKind::Proc(_)) { return Value::Nil; }
    let irep = vm.heap.proc_data(p).irep;
    let (line, file) = { let ir = &vm.ireps[irep]; (ir.line_of(0), ir.filename.clone()) };
    match (line, file) {
        (Some(line), Some(file)) => { let f = vm.str_from(file); vm.ary_new(vec![f, Value::Int(line as i64)]) }
        _ => Value::Nil,
    }
}

pub fn init(vm: &mut Vm) {
    let p = vm.core.proc_;
    vm.define_methods(p, &[
        ("inspect", proc_inspect),
        ("to_s", proc_inspect),
        ("lambda?", |vm, s, _a, _b| Ok(Value::bool(s.obj().map(|o| vm.heap.proc_data(o).strict).unwrap_or(false)))),
        ("parameters", |vm, s, _a, _b| { let o = s.obj().unwrap(); Ok(parameters_of(vm, o)) }),
        ("source_location", |vm, s, _a, _b| match s.obj() { Some(p) => Ok(source_location_of(vm, p)), None => Ok(Value::Nil) }),
    ]);
    // Kernel#proc as the gem's C function: the block itself (`&!`)
    let k = vm.core.kernel;
    fn kernel_proc(vm: &mut Vm, _s: Value, _a: &[Value], b: Value) -> VmResult<Value> {
        match b { Value::Obj(o) if matches!(vm.heap.get(o).kind, ObjKind::Proc(_)) => Ok(b), _ => Err(vm.raise_arg("no block given")) }
    }
    // `mrb_define_module_function_id(..., MRB_SYM(proc), ...)` (mruby-proc-ext proc.c)
    vm.define_module_function(k, "proc", kernel_proc).expect("Kernel singleton");
}
