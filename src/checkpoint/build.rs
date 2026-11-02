//! Building a parsed image into the VM: ireps, objects (made first, filled next), the changes
//! to permanent objects, Hash entries, frozen bits, then the stack and the frames.
//!
//! The collector is held off for the whole of it (`gc_disabled`): until the frames are in
//! place nothing that is made here is reachable from a root.

use alloc::{format, string::String, vec, vec::Vec};

use crate::object::{ClassData, EnvData, InstanceKind, Method, ObjKind, ProcData, Vis};
use crate::rite::{CatchHandler, CatchType, Pool};
use crate::symbol::Sym;
use crate::value::{ObjId, Slot, Value};
use crate::vm::{CallInfo, Cci, Vm, VmIrep, ROOT};

use super::image::{self, EnvPlace, Image, IrepRec, MethBody, MethRec, ObjRec, PatchRec, PoolRec, ProcBody, Val};
use super::names;

type R<T> = Result<T, String>;

/// What an object record became.
#[derive(Clone)]
enum Made {
    Obj(ObjId),
    /// A permanent name with nothing here, or a Proc whose body is a C function: fine as a
    /// method table's entry (which is looked up by name), refused as a value.
    Refused(String),
}

struct Builder<'a> {
    img: &'a Image,
    syms: Vec<Sym>,
    ireps: Vec<usize>,
    objs: Vec<Made>,
    /// Hash entries, put in once every object and method is in place (`hash` and `eql?` may be
    /// the program's own).
    hash_entries: Vec<(ObjId, Val, Val)>,
    /// Classes whose name the image gives as a path (`Outer::Inner`): the outer one is found
    /// once the constants are in.
    nested: Vec<(ObjId, String)>,
}

fn vis_of(code: u8) -> Vis {
    match code & 3 { 1 => Vis::Private, 2 => Vis::Protected, _ => Vis::Public }
}

impl<'a> Builder<'a> {
    fn sym(&self, i: u32) -> R<Option<Sym>> {
        if i == 0 { return Ok(None); }
        self.syms.get(i as usize - 1).copied().map(Some).ok_or_else(|| format!("symbol #{i} out of range"))
    }
    fn sym_req(&self, i: u32) -> R<Sym> {
        self.sym(i)?.ok_or_else(|| String::from("a name is missing"))
    }
    fn obj(&self, i: u32) -> R<ObjId> {
        match self.objs.get((i as usize).wrapping_sub(1)) {
            Some(Made::Obj(o)) => Ok(*o),
            Some(Made::Refused(why)) => Err(why.clone()),
            None => Err(format!("object #{i} out of range")),
        }
    }
    fn obj_opt(&self, i: u32) -> R<Option<ObjId>> {
        if i == 0 { Ok(None) } else { self.obj(i).map(Some) }
    }
    /// An object that must be a class (a superclass, a method table's owner, ...).
    fn class(&self, vm: &Vm, i: u32) -> R<ObjId> {
        let o = self.obj(i)?;
        if vm.heap.is_class(o) { Ok(o) } else { Err(format!("object #{i} is not a class")) }
    }
    fn class_opt(&self, vm: &Vm, i: u32) -> R<Option<ObjId>> {
        if i == 0 { Ok(None) } else { self.class(vm, i).map(Some) }
    }
    fn env(&self, vm: &Vm, i: u32) -> R<ObjId> {
        let o = self.obj(i)?;
        if matches!(vm.heap.get(o).kind, ObjKind::Env(_)) { Ok(o) } else { Err(format!("object #{i} is not an env")) }
    }
    fn val(&self, v: Val) -> R<Value> {
        Ok(match v {
            Val::Nil | Val::Undef => Value::Nil,
            Val::False => Value::False,
            Val::True => Value::True,
            Val::Int(i) => Value::Int(i),
            Val::Float(f) => Value::Float(f),
            Val::Sym(s) => Value::Sym(self.sym_req(s)?),
            Val::Obj(o) => Value::Obj(self.obj(o)?),
        })
    }
    fn rec(&self, i: u32) -> Option<&'a ObjRec> {
        self.img.objs.get((i as usize).wrapping_sub(1))
    }
    fn irep(&self, i: u32) -> R<usize> {
        self.ireps.get((i as usize).wrapping_sub(1)).copied().ok_or_else(|| format!("irep @{i} out of range"))
    }
}

fn intern_syms(vm: &mut Vm, img: &Image) -> Vec<Sym> {
    img.syms.iter().map(|s| vm.syms.intern(s)).collect()
}

/// The irep records, each checked against its hash and appended to `Vm::ireps`.
fn build_ireps(vm: &mut Vm, b: &mut Builder) -> R<()> {
    let img = b.img;
    let first = vm.ireps.len();
    let empty = vm.syms.intern(b"");
    for (i, r) in img.ireps.iter().enumerate() {
        if r.kind == 1 {
            return Err(format!("irep @{} is permanent (hash {:016x}) and its content is not in the image; \
                                this reader has a different mrblib and needs the content", i + 1, r.hash));
        }
        if image::irep_hash(img, r) != r.hash { return Err(format!("irep @{} content hash mismatch", i + 1)); }
        let ir = make_irep(b, r, empty)?;
        vm.ireps.push(ir);
        b.ireps.push(first + i);
    }
    for (i, r) in img.ireps.iter().enumerate() {
        let reps = r.reps.iter().map(|c| b.irep(*c)).collect::<R<Vec<usize>>>()?;
        vm.ireps[first + i].reps = reps;
    }
    Ok(())
}

fn make_irep(b: &Builder, r: &IrepRec, empty: Sym) -> R<VmIrep> {
    let mut catch = Vec::new();
    for h in r.catch.chunks(13) {
        let kind = match h[0] { 0 => CatchType::Rescue, 1 => CatchType::Ensure, t => return Err(format!("bad catch type {t}")) };
        let w = |k: usize| u32::from_be_bytes([h[k], h[k + 1], h[k + 2], h[k + 3]]);
        catch.push(CatchHandler { kind, begin: w(1), end: w(5), target: w(9) });
    }
    let mut pool = Vec::new();
    for p in &r.pool {
        pool.push(match p {
            PoolRec::Str(s) => Pool::Str(s.clone()),
            PoolRec::Int(i) => Pool::Int(*i),
            PoolRec::Float(f) => Pool::Float(*f),
            // the writer's packing: length, base, digits
            PoolRec::BigInt(x) if x.len() >= 2 => Pool::BigInt { base: x[1] as i8, digits: x[2..].to_vec() },
            PoolRec::BigInt(_) => return Err(String::from("bad big integer in a pool")),
        });
    }
    let mut syms = Vec::new();
    for s in &r.syms {
        // a missing name is the empty one, as a RITE file's 0xffff is read
        syms.push(b.sym(*s)?.unwrap_or(empty));
    }
    let lv = r.lv.iter().map(|s| b.sym(*s)).collect::<R<Vec<_>>>()?;
    Ok(VmIrep {
        nlocals: r.nlocals as usize, nregs: r.nregs as usize, iseq: r.iseq.clone(), catch, pool, syms,
        reps: Vec::new(), lv, lines: Vec::new(), filename: None,
    })
}

/// `M:Integer#times` -> (Integer, `times`)
fn method_class(vm: &mut Vm, name: &str) -> R<ObjId> {
    let rest = name.strip_prefix("M:").ok_or_else(|| format!("'{name}' is not a method name"))?;
    let i = rest.rfind('#').ok_or_else(|| format!("bad method name '{name}'"))?;
    match names::object(vm, &rest[..i])? {
        Value::Obj(c) if vm.heap.is_class(c) => Ok(c),
        _ => Err(format!("'{}' is not a class here", &rest[..i])),
    }
}

fn blank_env() -> EnvData {
    EnvData { ctx: ROOT, base: 0, len: 0, bidx: 0, attached: false, values: Vec::new(), mid: None, target_class: None,
              vis: Vis::Public, modfunc: false, vis_break: false, svar: None, svar_fwd: None }
}

/// Methods of the writer whose frames cannot go on here although their code is carried: the
/// rest of this VM finds them by something other than their code.
const FRAMES_REFUSED: &[(&str, &str)] = &[
    // the writer's `throw` finds `catch` by its Proc on the call stack; here `catch` is a native
    // loop frame (`ext_catch.rs`) and `throw` looks for that, so a carried `catch` frame is
    // never found and the throw escapes as UncaughtThrowError
    ("M:Kernel#catch", "throw here does not find the writer's catch frames"),
];

/// Pass 1: every object exists (permanent ones are looked up), its fields not filled yet.
fn make_objects(vm: &mut Vm, b: &mut Builder) -> R<()> {
    let img = b.img;
    let obj = vm.core.object;
    for r in &img.objs {
        let made = match r {
            ObjRec::Perm(name) => match names::object(vm, name) {
                Ok(Value::Obj(o)) => Made::Obj(o),
                Ok(_) => Made::Refused(format!("permanent '{name}' is not an object here")),
                Err(e) => Made::Refused(format!("permanent {e}")),
            },
            ObjRec::PermBody(name, irep) => {
                if let Some(why) = FRAMES_REFUSED.iter().find(|(n, _)| n == name).map(|(_, w)| w) {
                    return Err(format!("a frame runs {}: {why}", &name[2..]));
                }
                // the method's own Proc here runs other code: a Proc of the carried irep stands
                // in for it, for the frames that run it and the blocks whose `upper` it is
                let class = method_class(vm, name)?;
                let irep = b.irep(*irep)?;
                Made::Obj(vm.heap.alloc(vm.core.proc_, ObjKind::Proc(ProcData {
                    irep, upper: None, env: None, target_class: Some(class), strict: true, scope: true, orphan: false, mid: None,
                })))
            }
            ObjRec::Object(..) => Made::Obj(vm.heap.alloc(obj, ObjKind::Object)),
            ObjRec::Str(_, s) => Made::Obj(vm.heap.alloc(obj, ObjKind::String(s.clone()))),
            ObjRec::Array(..) => Made::Obj(vm.heap.alloc(obj, ObjKind::Array(Default::default()))),
            ObjRec::Hash { .. } => Made::Obj(vm.heap.alloc(obj, ObjKind::Hash(Default::default()))),
            ObjRec::Range { excl, .. } => Made::Obj(vm.heap.alloc(obj, ObjKind::Range { begin: Slot::NIL, end: Slot::NIL, excl: *excl })),
            ObjRec::Exception { .. } => Made::Obj(vm.heap.alloc(obj, ObjKind::Exception)),
            ObjRec::Class { .. } | ObjRec::IClass { .. } => Made::Obj(vm.heap.alloc(obj, ObjKind::Class(ClassData::default()))),
            ObjRec::Proc { body: ProcBody::Func(f), .. } => Made::Refused(format!("a Proc made from the C function {f} cannot be carried here")),
            ObjRec::Proc { .. } => Made::Obj(vm.heap.alloc(vm.core.proc_, ObjKind::Proc(ProcData {
                irep: 0, upper: None, env: None, target_class: None, strict: false, scope: false, orphan: false, mid: None,
            }))),
            ObjRec::Env { .. } => Made::Obj(vm.heap.alloc(obj, ObjKind::Env(blank_env()))),
            ObjRec::Data { type_name, blob, .. } => match super::module::restore_data(vm, type_name, blob, obj)? {
                Value::Obj(o) => Made::Obj(o),
                _ => unreachable!(),
            },
        };
        b.objs.push(made);
    }
    Ok(())
}

fn set_ivars(vm: &mut Vm, b: &Builder, o: ObjId, ivars: &[(u32, Val)]) -> R<()> {
    for (s, v) in ivars {
        let s = b.sym_req(*s)?;
        let v = b.val(*v)?;
        vm.heap.ivar_set(o, s, v);
    }
    Ok(())
}

/// A class's "instance variable" in the writer's sense: its constants, class variables and
/// the writer's own marks share one table there; here they have fields of their own.
/// `permanent` says the class was here already, whose name and owner are this VM's.
fn class_var(vm: &mut Vm, b: &mut Builder, c: ObjId, s: Sym, v: Value, permanent: bool) -> R<()> {
    let name = vm.syms.name(s).to_vec();
    match name.as_slice() {
        b"__classname__" => {
            if permanent { return Ok(()); }
            match v {
                Value::Sym(n) => { vm.heap.class_mut(c).name = Some(n); let o = vm.core.object; vm.heap.class_mut(c).outer = Some(o); }
                Value::Obj(p) => match vm.heap.string(p) {
                    Some(path) => {
                        let path = String::from_utf8_lossy(path).into_owned();
                        let last = String::from(path.rsplit("::").next().unwrap_or(&path));
                        let n = vm.intern(&last);
                        vm.heap.class_mut(c).name = Some(n);
                        b.nested.push((c, path));
                    }
                    None => return Err(String::from("a class name that is not a String")),
                },
                _ => {}
            }
        }
        b"__outer__" => { if !permanent { vm.heap.class_mut(c).outer = v.obj(); } }
        b"__attached__" => { if !permanent { vm.heap.class_mut(c).attached = Some(Slot::from(v)); } }
        n if n.starts_with(b"@@") => { vm.heap.class_mut(c).cvars.insert(s, Slot::from(v)); }
        n if n.first().is_some_and(|x| x.is_ascii_uppercase()) => { vm.heap.class_mut(c).consts.insert(s, Slot::from(v)); }
        _ => vm.heap.ivar_set(c, s, v),
    }
    Ok(())
}

fn class_var_remove(vm: &mut Vm, c: ObjId, s: Sym) {
    let name = vm.syms.name(s).to_vec();
    if name.starts_with(b"__") { return; }
    if name.starts_with(b"@@") { vm.heap.class_mut(c).cvars.remove(&s); }
    else if name.first().is_some_and(|x| x.is_ascii_uppercase()) { vm.heap.class_mut(c).consts.remove(&s); }
    else { vm.heap.get_mut(c).ivars.retain(|(n, _)| *n != s); }
}

/// A method table's entry: a Proc of the program, the reader's own method of the same name
/// for a permanent one, or the attribute methods the writer makes as closures.
fn method_of(vm: &mut Vm, b: &Builder, m: &MethRec) -> R<Method> {
    match &m.body {
        MethBody::Undef => Ok(Method::Undef),
        MethBody::Func(name, _) => names::method(vm, name),
        MethBody::Proc(p) => match b.rec(*p) {
            Some(ObjRec::Perm(name)) if name.starts_with("M:") => names::method(vm, name),
            Some(ObjRec::Proc { body: ProcBody::Func(f), env_kind, env, .. }) => {
                let attr = || -> R<Sym> {
                    if *env_kind != 1 { return Err(format!("{f} without its environment")); }
                    match b.rec(*env) {
                        Some(ObjRec::Env { place: EnvPlace::Values(v), .. }) if !v.is_empty() => match v[0] {
                            Val::Sym(s) => b.sym_req(s),
                            _ => Err(format!("{f}: the name is not a Symbol")),
                        },
                        _ => Err(format!("{f}: the environment is not a detached one")),
                    }
                };
                match f.as_str() {
                    "F:attr_reader" => Ok(Method::AttrReader(attr()?)),
                    "F:attr_writer" => Ok(Method::AttrWriter(attr()?)),
                    _ => names::method(vm, f),
                }
            }
            _ => Ok(Method::Ruby(b.obj(*p)?)),
        },
    }
}

fn define(vm: &mut Vm, b: &Builder, c: ObjId, m: &MethRec) -> R<()> {
    let s = b.sym_req(m.name)?;
    let meth = method_of(vm, b, m)?;
    let cd = vm.heap.class_mut(c);
    cd.methods.insert(s, meth);
    match vis_of(m.vis) {
        Vis::Public => { cd.vis.remove(&s); }
        v => { cd.vis.insert(s, v); }
    }
    Ok(())
}

/// Pass 2: the fields.
fn fill_objects(vm: &mut Vm, b: &mut Builder) -> R<()> {
    let img = b.img;
    for (i, r) in img.objs.iter().enumerate() {
        let o = match &b.objs[i] { Made::Obj(o) => *o, Made::Refused(_) => continue };
        let head = match r {
            ObjRec::Perm(_) | ObjRec::PermBody(..) => continue,
            ObjRec::Object(h, _) | ObjRec::Str(h, _) | ObjRec::Array(h, _) => *h,
            ObjRec::Hash { head, .. } | ObjRec::Range { head, .. } | ObjRec::Exception { head, .. } | ObjRec::Class { head, .. }
            | ObjRec::IClass { head, .. } | ObjRec::Proc { head, .. } | ObjRec::Env { head, .. } | ObjRec::Data { head, .. } => *head,
        };
        let class = match r {
            // the module an include class stands for is in its class field
            ObjRec::IClass { .. } => vm.core.class,
            ObjRec::Env { .. } => vm.core.object,
            // what the writer leaves without a class (NULL: its Procs, and the hidden Arrays its
            // C functions make, which a register may still hold) has the plain one here
            ObjRec::Proc { .. } if head.class == 0 => vm.core.proc_,
            ObjRec::Array(..) if head.class == 0 => vm.core.array,
            ObjRec::Str(..) if head.class == 0 => vm.core.string,
            ObjRec::Hash { .. } if head.class == 0 => vm.core.hash,
            _ => b.class(vm, head.class).map_err(|e| format!("class of #{}: {e}", i + 1))?,
        };
        vm.heap.get_mut(o).class = class;
        match r {
            ObjRec::Object(_, iv) | ObjRec::Data { ivars: iv, .. } => set_ivars(vm, b, o, iv)?,
            ObjRec::Str(..) => {}
            ObjRec::Array(_, items) => {
                let v = items.iter().map(|x| b.val(*x).map(Slot::from)).collect::<R<Vec<Slot>>>()?;
                if let ObjKind::Array(a) = &mut vm.heap.get_mut(o).kind { *a = v.into(); }
            }
            ObjRec::Hash { flags, pairs, ivars, .. } => {
                for (k, v) in pairs { b.hash_entries.push((o, *k, *v)); }
                for (s, v) in ivars {
                    let s = b.sym_req(*s)?;
                    let v = b.val(*v)?;
                    if vm.syms.name(s) == b"ifnone" {
                        if flags & 2 != 0 {
                            let dp = vm.s.default_proc;
                            vm.heap.ivar_set(o, dp, v);
                        } else if flags & 1 != 0 {
                            if let Some(h) = vm.hd_mut(o) { h.default = Slot::from(v); }
                        }
                    } else {
                        vm.heap.ivar_set(o, s, v);
                    }
                }
            }
            ObjRec::Range { begin, end, excl, .. } => {
                let (x, y) = (b.val(*begin)?, b.val(*end)?);
                vm.heap.get_mut(o).kind = ObjKind::Range { begin: Slot::from(x), end: Slot::from(y), excl: *excl };
            }
            ObjRec::Exception { mesg, bt, ivars, .. } => {
                let m = b.val(*mesg)?;
                if !m.is_nil() { let k = vm.s.mesg; vm.heap.ivar_set(o, k, m); }
                let t = b.val(*bt)?;
                if !t.is_nil() {
                    // the writer gives the backtrace as text, which is what `set_backtrace` keeps
                    let k = vm.intern("@__btstr");
                    vm.heap.ivar_set(o, k, t);
                    let r = vm.intern("@__raised");
                    vm.heap.ivar_set(o, r, Value::True);
                }
                set_ivars(vm, b, o, ivars)?;
            }
            ObjRec::Class { kind, sup, cflags, ivars, methods, .. } => {
                let sup = b.class_opt(vm, *sup)?;
                {
                    let cd = vm.heap.class_mut(o);
                    cd.superclass = sup;
                    cd.is_module = *kind == 9;
                    cd.is_singleton = *kind == 10;
                    if cflags & 1 != 0 { cd.instance_kind = Some(InstanceKind::NoAlloc); }
                }
                for (s, v) in ivars {
                    let s = b.sym_req(*s)?;
                    let v = b.val(*v)?;
                    class_var(vm, b, o, s, v, false)?;
                }
                for m in methods { define(vm, b, o, m)?; }
            }
            ObjRec::IClass { head, sup } => {
                let m = b.class(vm, head.class)?;
                let sup = b.class_opt(vm, *sup)?;
                let cd = vm.heap.class_mut(o);
                cd.iclass_of = Some(m);
                cd.superclass = sup;
            }
            ObjRec::Proc { flags, body, upper, env_kind, env, .. } => {
                let irep = match body { ProcBody::Irep(i) => b.irep(*i)?, ProcBody::Func(_) => unreachable!() };
                let upper = b.obj_opt(*upper)?;
                let (env, tc) = match env_kind {
                    1 => {
                        let e = b.env(vm, *env)?;
                        let tc = match b.rec(*env) { Some(ObjRec::Env { head, .. }) if head.class != 0 => Some(b.class(vm, head.class)?), _ => None };
                        (Some(e), tc.or(Some(vm.core.object)))
                    }
                    2 => (None, Some(b.class(vm, *env)?)),
                    _ => (None, None),
                };
                vm.heap.get_mut(o).kind = ObjKind::Proc(ProcData {
                    irep, upper, env, target_class: tc, strict: flags & 1 != 0, orphan: flags & 2 != 0, scope: flags & 4 != 0, mid: None,
                });
            }
            ObjRec::Env { head, mid, len, bidx, vis, place } => {
                let mut e = blank_env();
                e.mid = b.sym(*mid)?;
                e.len = *len as usize;
                e.bidx = *bidx as usize;
                e.target_class = b.class_opt(vm, head.class)?;
                e.vis = vis_of(*vis);
                e.vis_break = vis & 4 != 0;
                e.modfunc = vis & 8 != 0;
                match place {
                    EnvPlace::Stack(off) => { e.attached = true; e.base = *off as usize; }
                    EnvPlace::Values(v) => { e.values = v.iter().map(|x| b.val(*x).map(Slot::from)).collect::<R<Vec<Slot>>>()?; }
                }
                vm.heap.get_mut(o).kind = ObjKind::Env(e);
            }
            ObjRec::Perm(_) | ObjRec::PermBody(..) => {}
        }
    }
    Ok(())
}

fn is_class_obj(vm: &Vm, o: ObjId) -> bool { vm.heap.is_class(o) }

fn apply_patches(vm: &mut Vm, b: &mut Builder) -> R<()> {
    let img = b.img;
    for p in &img.patches {
        match p {
            PatchRec::MethodSet(c, m) => { let c = b.class(vm, *c)?; define(vm, b, c, m)?; }
            PatchRec::MethodRemove(c, s) => {
                let c = b.class(vm, *c)?;
                let s = b.sym_req(*s)?;
                let cd = vm.heap.class_mut(c);
                cd.methods.remove(&s);
                cd.vis.remove(&s);
            }
            PatchRec::IvSet(o, s, v) => {
                let o = b.obj(*o)?;
                let s = b.sym_req(*s)?;
                let v = b.val(*v)?;
                if is_class_obj(vm, o) { class_var(vm, b, o, s, v, true)?; } else { vm.heap.ivar_set(o, s, v); }
            }
            PatchRec::IvRemove(o, s) => {
                let o = b.obj(*o)?;
                let s = b.sym_req(*s)?;
                if is_class_obj(vm, o) { class_var_remove(vm, o, s); } else { vm.heap.get_mut(o).ivars.retain(|(n, _)| *n != s); }
            }
            PatchRec::SuperSet(c, s) => {
                let c = b.class(vm, *c)?;
                let s = b.class_opt(vm, *s)?;
                vm.heap.class_mut(c).superclass = s;
            }
            PatchRec::GvSet(s, v) => { let s = b.sym_req(*s)?; let v = b.val(*v)?; vm.globals.insert(s, Slot::from(v)); }
            PatchRec::GvRemove(s) => { let s = b.sym_req(*s)?; vm.globals.remove(&s); }
        }
    }
    Ok(())
}

/// `Outer::Inner` given as the name of a class: `outer` is the class the path's head names.
fn fix_nested(vm: &mut Vm, b: &Builder) -> R<()> {
    for (c, path) in &b.nested {
        let i = path.rfind("::").unwrap_or(0);
        if i == 0 { continue; }
        match names::object(vm, &path[..i]) {
            Ok(Value::Obj(o)) if vm.heap.is_class(o) => vm.heap.class_mut(*c).outer = Some(o),
            _ => return Err(format!("the outer class of {path} is not found")),
        }
    }
    Ok(())
}

fn fill_hashes(vm: &mut Vm, b: &Builder) -> R<()> {
    for (h, k, v) in &b.hash_entries {
        let (k, v) = (b.val(*k)?, b.val(*v)?);
        if let Err(e) = vm.hash_set(Value::Obj(*h), k, v) {
            let d = vm.describe_error(&e);
            return Err(format!("a Hash entry could not be put back: {d}"));
        }
    }
    Ok(())
}

fn freeze(vm: &mut Vm, b: &Builder) {
    for (i, r) in b.img.objs.iter().enumerate() {
        let frozen = match r {
            ObjRec::Object(h, _) | ObjRec::Str(h, _) | ObjRec::Array(h, _) => h.frozen,
            ObjRec::Hash { head, .. } | ObjRec::Range { head, .. } | ObjRec::Exception { head, .. } | ObjRec::Class { head, .. }
            | ObjRec::Data { head, .. } => head.frozen,
            _ => false,
        };
        if let (true, Made::Obj(o)) = (frozen, &b.objs[i]) { vm.heap.get_mut(*o).frozen = true; }
    }
}

/// The stack and the frames, which is where the program goes on.
fn build_frames(vm: &mut Vm, b: &Builder) -> R<()> {
    let img = b.img;
    if img.frames.is_empty() { return Err(String::from("no frames")); }
    let mut stack = img.stack.iter().map(|v| b.val(*v).map(Slot::from)).collect::<R<Vec<Slot>>>()?;
    let mut ci = Vec::new();
    for (k, f) in img.frames.iter().enumerate() {
        let p = b.obj(f.proc_).map_err(|e| format!("frame {k}: {e}"))?;
        let pd = match &vm.heap.get(p).kind { ObjKind::Proc(pd) => pd.clone(), _ => return Err(format!("frame {k} does not run a Proc")) };
        if f.cci != 0 { return Err(format!("frame {k} was entered from C, which cannot be carried")); }
        let irep = pd.irep;
        let iseq_len = vm.ireps[irep].iseq.len();
        if f.pc == u32::MAX || f.pc as usize >= iseq_len { return Err(format!("frame {k}: pc {} out of range", f.pc)); }
        let kw = match f.nk { 0 => false, 15 => true, n => return Err(format!("frame {k}: {n} keyword arguments not yet packed into a Hash")) };
        let base = f.base as usize;
        if base > img.stack.len() { return Err(format!("frame {k}: its registers start past the stack")); }
        let (env, tc) = match f.env_kind {
            1 => {
                let e = b.env(vm, f.env)?;
                let tc = vm.heap.env(e).target_class.or(pd.target_class).unwrap_or(vm.core.object);
                (Some(e), tc)
            }
            2 => (None, b.class(vm, f.env)?),
            _ => (None, pd.target_class.unwrap_or(vm.core.object)),
        };
        if let Some(e) = env {
            let ed = vm.heap.env(e);
            if ed.attached && ed.base != base { return Err(format!("frame {k}: its env is on the stack at {} but the frame at {base}", ed.base)); }
        }
        let nregs = vm.ireps[irep].nregs.max(4);
        if stack.len() < base + nregs { stack.resize(base + nregs, Slot::NIL); }
        ci.push(CallInfo {
            base, pc: f.pc as usize, irep, proc_: p, n: f.n & 0xf, kw, mid: b.sym(f.mid)?, target_class: tc, env,
            // the bottom frame is the one the host started: the run loop returns when it does
            cci: if k == 0 { Cci::Skip } else { Cci::None },
            vis: vis_of(f.vis), modfunc: f.vis & 8 != 0, vis_break: f.vis & 4 != 0, method_block: false,
        });
    }
    // every env on the stack must be some frame's
    for (i, r) in img.objs.iter().enumerate() {
        if let (ObjRec::Env { place: EnvPlace::Stack(off), len, .. }, Made::Obj(e)) = (r, &b.objs[i]) {
            let off = *off as usize;
            if !ci.iter().any(|c| c.base == off && c.env == Some(*e)) { return Err(format!("env #{} on the stack at {off} has no frame", i + 1)); }
            if stack.len() < off + *len as usize { stack.resize(off + *len as usize, Slot::NIL); }
        }
    }
    vm.stack = stack;
    vm.ci = ci;
    Ok(())
}

/// Builds the image into `vm`, which must be idle (no frame). On success the frames are in
/// place and [`Vm::checkpoint_run`] goes on from the top one.
pub fn build(vm: &mut Vm, img: &Image) -> R<()> {
    if !vm.ci.is_empty() || vm.cur != ROOT { return Err(String::from("restore needs an idle VM")); }
    let mut b = Builder { img, syms: vec![], ireps: vec![], objs: vec![], hash_entries: vec![], nested: vec![] };
    b.syms = intern_syms(vm, img);
    let gc_was = core::mem::replace(&mut vm.gc_disabled, true);
    let r = (|| -> R<()> {
        build_ireps(vm, &mut b)?;
        make_objects(vm, &mut b)?;
        fill_objects(vm, &mut b)?;
        apply_patches(vm, &mut b)?;
        fix_nested(vm, &b)?;
        fill_hashes(vm, &b)?;
        freeze(vm, &b);
        build_frames(vm, &b)
    })();
    vm.gc_disabled = gc_was;
    if r.is_err() { vm.stack.clear(); vm.ci.clear(); }
    r
}
