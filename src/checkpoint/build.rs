//! Building a parsed image into the VM: ireps, objects (made first, filled next), the changes
//! to permanent objects, Hash entries, frozen bits, then the contexts (their stacks and frames)
//! and the scheduler's Tasks.
//!
//! The collector is held off for the whole of it (`gc_disabled`): until the frames are in
//! place nothing that is made here is reachable from a root.

use alloc::{format, string::String, vec, vec::Vec};

use crate::object::{ClassData, EnvData, InstanceKind, Method, ObjKind, ProcData, TaskData, TaskPlace, Vis};
use crate::rite::{CatchHandler, CatchType, Pool};
use crate::symbol::Sym;
use crate::value::{ObjId, Slot, Value};
use crate::vm::{CallInfo, Cci, Context, FiberState, Vm, VmIrep, ROOT};

use super::image::{self, EnvPlace, Image, IrepRec, MethBody, MethRec, ObjRec, PatchRec, PoolRec, ProcBody, Val, WaitRec};
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
    /// The image's contexts (index from 0 here) as this VM's (`Vm::contexts`).
    ctxs: Vec<usize>,
}

/// What [`Vm::checkpoint_run`] starts again once the image is built.
pub enum Plan {
    /// No Task was running: the current context (the root, or a Fiber it resumed) goes on.
    Context,
    /// A Task was running: the rest of its slice from the context `start` (the Task's own or a
    /// Fiber it resumed), then the scheduler's caller.
    Slice { task: ObjId, start: usize, ended: bool, entry: Entry },
}

/// Where the scheduler that ran the Task was entered from.
pub enum Entry {
    /// A host that runs the scheduler's loop itself (`mrb_task_run` from C): the loop goes on.
    Host,
    /// `Task.run`: the call is made again (the caller's pc is set back to it); the loop picks
    /// up the queues as they are.
    Run,
    /// `Task.pass` at the root: that step is over, the call answers nil (in this register).
    Pass(usize),
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
        // the writer's irep may count children and have no table of them (some of its
        // mrblib's): every entry is 0 then, and there is nothing to link
        if !r.reps.is_empty() && r.reps.iter().all(|c| *c == 0) { continue; }
        let reps = r.reps.iter().map(|c| b.irep(*c)).collect::<R<Vec<usize>>>()?;
        vm.ireps[first + i].reps = reps;
    }
    for (id, file, ents) in &img.lines {
        let k = *id as usize;
        if k == 0 || k > img.ireps.len() { return Err(format!("line numbers for irep @{id}, which is not in the image")); }
        let ir = &mut vm.ireps[first + k - 1];
        ir.lines = ents.clone();
        ir.filename = Some(file.clone());
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
                // in for it, for the frames that run it and the blocks whose `upper` it is. Its
                // lexical scope (the class bodies it was written in, which the constants are
                // looked up through) is that of this VM's method of the name, when that one is
                // Ruby too
                let class = method_class(vm, name)?;
                let irep = b.irep(*irep)?;
                let upper = match names::method(vm, name) {
                    Ok(Method::Ruby(p)) => match &vm.heap.get(p).kind { ObjKind::Proc(pd) => pd.upper, _ => None },
                    _ => None,
                };
                Made::Obj(vm.heap.alloc(vm.core.proc_, ObjKind::Proc(ProcData {
                    irep, upper, env: None, target_class: Some(class), strict: true, scope: true, orphan: false, mid: None,
                })))
            }
            ObjRec::Object(..) => Made::Obj(vm.heap.alloc(obj, ObjKind::Object)),
            ObjRec::Str(_, s) => Made::Obj(vm.heap.alloc(obj, ObjKind::String(s.clone()))),
            ObjRec::BinStr(_, s) => {
                let o = vm.heap.alloc(obj, ObjKind::String(s.clone()));
                vm.heap.get_mut(o).binary = true;
                Made::Obj(o)
            }
            ObjRec::Array(..) => Made::Obj(vm.heap.alloc(obj, ObjKind::Array(Default::default()))),
            ObjRec::Hash { .. } => Made::Obj(vm.heap.alloc(obj, ObjKind::Hash(Default::default()))),
            ObjRec::Range { excl, .. } => Made::Obj(vm.heap.alloc(obj, ObjKind::Range { begin: Slot::NIL, end: Slot::NIL, excl: *excl })),
            ObjRec::Exception { .. } => Made::Obj(vm.heap.alloc(obj, ObjKind::Exception)),
            ObjRec::Class { .. } | ObjRec::IClass { .. } => Made::Obj(vm.heap.alloc(obj, ObjKind::Class(ClassData::default()))),
            ObjRec::Proc { body: ProcBody::Func(f), .. } => Made::Refused(format!("a Proc made from the C function {f} cannot be carried here")),
            // the method table entry it is stands for its original (method_of); as a value it
            // has no counterpart here
            ObjRec::Proc { body: ProcBody::Alias(_), .. } => Made::Refused(String::from("a Proc made by alias_method, as a value")),
            ObjRec::Proc { .. } => Made::Obj(vm.heap.alloc(vm.core.proc_, ObjKind::Proc(ProcData {
                irep: 0, upper: None, env: None, target_class: None, strict: false, scope: false, orphan: false, mid: None,
            }))),
            ObjRec::Env { .. } => Made::Obj(vm.heap.alloc(obj, ObjKind::Env(blank_env()))),
            ObjRec::Data { type_name, blob, .. } => match super::module::restore_data(vm, type_name, blob, obj)? {
                Value::Obj(o) => Made::Obj(o),
                _ => unreachable!(),
            },
            // the Task's fields and its context come with the scheduler (build_tasks)
            ObjRec::Task { .. } => Made::Obj(vm.heap.alloc(obj, ObjKind::Task(alloc::boxed::Box::new(TaskData {
                ctx: usize::MAX, priority: 128, status: 0, reason: 0, timeslice: 0, name: Slot::NIL, result: Slot::NIL,
                wakeup_tick: u32::MAX, join: None, queue: None, instructions: 0, place: TaskPlace::NONE,
            })))),
            // its context comes with the contexts (build_contexts)
            ObjRec::Fiber { .. } => Made::Obj(vm.heap.alloc(vm.core.fiber, ObjKind::Fiber(usize::MAX))),
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
            // alias_method of a Ruby method: the original's method (its upper)
            Some(ObjRec::Proc { body: ProcBody::Alias(_), upper, .. }) => {
                let up = MethRec { name: m.name, vis: m.vis, body: MethBody::Proc(*upper) };
                method_of(vm, b, &up)
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
            ObjRec::Object(h, _) | ObjRec::Str(h, _) | ObjRec::BinStr(h, _) | ObjRec::Array(h, _) => *h,
            ObjRec::Hash { head, .. } | ObjRec::Range { head, .. } | ObjRec::Exception { head, .. } | ObjRec::Class { head, .. }
            | ObjRec::IClass { head, .. } | ObjRec::Proc { head, .. } | ObjRec::Env { head, .. } | ObjRec::Data { head, .. }
            | ObjRec::Task { head, .. } | ObjRec::Fiber { head, .. } => *head,
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
            ObjRec::Object(_, iv) | ObjRec::Data { ivars: iv, .. } | ObjRec::Task { ivars: iv, .. } => set_ivars(vm, b, o, iv)?,
            ObjRec::Fiber { .. } => {}
            ObjRec::Str(..) | ObjRec::BinStr(..) => {}
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
                let irep = match body { ProcBody::Irep(i) => b.irep(*i)?, ProcBody::Func(_) | ProcBody::Alias(_) => unreachable!() };
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
                    // the context is this VM's once the contexts are made (build_contexts)
                    EnvPlace::Stack(_, off) => { e.attached = true; e.base = *off as usize; }
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
            PatchRec::StrSet(o, frozen, bytes) => {
                let o = b.obj(*o)?;
                match &mut vm.heap.get_mut(o).kind {
                    ObjKind::String(s) => { *s = bytes.clone(); }
                    _ => return Err(String::from("string contents for an object that is not a String")),
                }
                vm.heap.get_mut(o).frozen = *frozen;
            }
            PatchRec::AryContents(o, frozen, vals) => {
                let o = b.obj(*o)?;
                let v = vals.iter().map(|x| b.val(*x).map(Slot::from)).collect::<R<Vec<Slot>>>()?;
                match &mut vm.heap.get_mut(o).kind {
                    ObjKind::Array(a) => { *a = v.into(); }
                    _ => return Err(String::from("array contents for an object that is not an Array")),
                }
                vm.heap.get_mut(o).frozen = *frozen;
            }
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

/// The C functions a frame of the writer may wait in (the top of a context that is not running),
/// and what they are here.
const SWITCH_FUNCS: &[&str] = &["F:Fiber#resume", "F:Fiber#transfer", "F:S:Fiber#yield", "F:S:Task#run", "F:S:Task#pass"];

fn fiber_state(code: u8) -> R<FiberState> {
    Ok(match code {
        0 => FiberState::Created, 1 => FiberState::Running, 2 => FiberState::Resumed,
        3 => FiberState::Suspended, 4 => FiberState::Transferred, 5 => FiberState::Terminated,
        c => return Err(format!("bad context status {c}")),
    })
}

/// One context's stack and frames as this VM keeps them. The C function frame the writer has on
/// top of a context that waits (in `Fiber#resume`, `Fiber.yield`, ...) is not a frame here: the
/// value that call is given lands in a register (`Context::pending_reg`), which is that frame's
/// first; it is answered with the name of the function.
fn context_frames(vm: &mut Vm, b: &Builder, cx: usize) -> R<(Vec<Slot>, Vec<CallInfo>, Option<(String, usize)>)> {
    let rec = &b.img.ctxs[cx];
    let mut stack = rec.stack.iter().map(|v| b.val(*v).map(Slot::from)).collect::<R<Vec<Slot>>>()?;
    let mut ci = Vec::new();
    let mut waits = None;
    for (k, f) in rec.frames.iter().enumerate() {
        if f.cci != 0 { return Err(format!("frame {k} of context {} was entered from C, which cannot be carried", cx + 1)); }
        if let Some(name) = &f.cfunc {
            if k + 1 != rec.frames.len() { return Err(format!("a C function frame below the top of context {}", cx + 1)); }
            if !SWITCH_FUNCS.contains(&name.as_str()) { return Err(format!("context {} waits in the C function {name}", cx + 1)); }
            waits = Some((name.clone(), f.base as usize));
            break;
        }
        let p = b.obj(f.proc_).map_err(|e| format!("frame {k} of context {}: {e}", cx + 1))?;
        let pd = match &vm.heap.get(p).kind { ObjKind::Proc(pd) => pd.clone(), _ => return Err(format!("frame {k} does not run a Proc")) };
        let irep = pd.irep;
        let iseq_len = vm.ireps[irep].iseq.len();
        if f.pc == u32::MAX || f.pc as usize >= iseq_len { return Err(format!("frame {k}: pc {} out of range", f.pc)); }
        let kw = match f.nk { 0 => false, 15 => true, n => return Err(format!("frame {k}: {n} keyword arguments not yet packed into a Hash")) };
        let base = f.base as usize;
        if base > rec.stack.len() { return Err(format!("frame {k}: its registers start past the stack")); }
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
            // the root's bottom frame is the one the host started: the run loop returns when it does
            cci: if k == 0 && cx == 0 { Cci::Skip } else { Cci::None },
            vis: vis_of(f.vis), modfunc: f.vis & 8 != 0, vis_break: f.vis & 4 != 0, method_block: false,
        });
    }
    if let Some((_, reg)) = &waits { if stack.len() <= *reg { stack.resize(reg + 1, Slot::NIL); } }
    Ok((stack, ci, waits))
}

/// The contexts: the root's frames, and a context of this VM for each of the others (a Task's,
/// a Fiber's). Answers the context that was running and, for each image context, the C function
/// its top frame waits in.
fn build_contexts(vm: &mut Vm, b: &mut Builder) -> R<Vec<Option<(String, usize)>>> {
    let img = b.img;
    if img.ctxs.is_empty() { return Err(String::from("no contexts")); }
    let cur = img.current as usize;
    if cur == 0 || cur > img.ctxs.len() { return Err(String::from("current context out of range")); }
    b.ctxs = vec![ROOT];
    for _ in 1..img.ctxs.len() {
        vm.contexts.push(Context::new(FiberState::Created));
        b.ctxs.push(vm.contexts.len() - 1);
    }
    let mut waits = Vec::new();
    for cx in 0..img.ctxs.len() {
        let rec = &img.ctxs[cx];
        let id = b.ctxs[cx];
        let status = fiber_state(rec.status)?;
        // a finished one keeps nothing here (the writer's Task keeps its last frame, at the
        // end of its code)
        let (stack, ci, w) = if status == FiberState::Terminated { (Vec::new(), Vec::new(), None) } else { context_frames(vm, b, cx)? };
        let first_proc = rec.frames.first().and_then(|f| b.obj(f.proc_).ok());
        let c = &mut vm.contexts[id];
        c.prev = match rec.prev { 0 => None, p if (p as usize) <= img.ctxs.len() => Some(b.ctxs[p as usize - 1]), _ => return Err(String::from("context out of range")) };
        c.fib = b.obj_opt(rec.fib)?;
        c.pending_reg = w.as_ref().map(|(_, r)| *r);
        if id != ROOT { c.proc_ = first_proc; }
        c.status = match status {
            // the writer's root is "created" while the program runs (it is never resumed)
            FiberState::Created if id == ROOT => FiberState::Running,
            // a Fiber that has not run yet: its block is laid out when it is resumed, as here
            // (`fiber_switch`); a Task's context is "created" in the writer all along
            FiberState::Created if ci.len() <= 2 && rec.frames.iter().all(|f| f.pc == 0) && rec.fib != 0 && !is_task_context(img, cx) => FiberState::Created,
            FiberState::Created => FiberState::Suspended,
            s => s,
        };
        if c.status == FiberState::Created && id != ROOT {
            c.stack = Vec::new();
            c.ci = Vec::new();
        } else {
            c.stack = stack;
            c.ci = ci;
        }
        waits.push(w);
    }
    // the envs on a stack: whose stack
    for (i, r) in img.objs.iter().enumerate() {
        if let (ObjRec::Env { place: EnvPlace::Stack(cx, off), len, .. }, Made::Obj(e)) = (r, &b.objs[i]) {
            let cx = *cx as usize;
            if cx == 0 || cx > img.ctxs.len() { return Err(format!("env #{} on the stack of a context out of range", i + 1)); }
            let id = b.ctxs[cx - 1];
            let off = *off as usize;
            if vm.contexts[id].status == FiberState::Terminated {
                // a finished context's frames are gone here: its env keeps the values (the
                // writer's Task does not detach them when it stops; this VM does, stop_task)
                let rec = &img.ctxs[cx - 1];
                let vals = (0..*len as usize).map(|k| rec.stack.get(off + k).map_or(Ok(Slot::NIL), |v| b.val(*v).map(Slot::from))).collect::<R<Vec<Slot>>>()?;
                if let ObjKind::Env(ed) = &mut vm.heap.get_mut(*e).kind { ed.attached = false; ed.values = vals; ed.ctx = ROOT; }
                continue;
            }
            let c = &mut vm.contexts[id];
            if !c.ci.iter().any(|f| f.base == off && f.env == Some(*e)) { return Err(format!("env #{} on the stack at {off} has no frame", i + 1)); }
            if c.stack.len() < off + *len as usize { c.stack.resize(off + *len as usize, Slot::NIL); }
            if let ObjKind::Env(ed) = &mut vm.heap.get_mut(*e).kind { ed.ctx = id; }
        }
    }
    // the Fiber objects
    for (i, r) in img.objs.iter().enumerate() {
        if let (ObjRec::Fiber { ctx, .. }, Made::Obj(f)) = (r, &b.objs[i]) {
            if *ctx == 0 { continue; }
            if *ctx as usize > img.ctxs.len() { return Err(format!("Fiber #{} of a context out of range", i + 1)); }
            vm.heap.get_mut(*f).kind = ObjKind::Fiber(b.ctxs[*ctx as usize - 1]);
        }
    }
    Ok(waits)
}

fn is_task_context(img: &Image, cx: usize) -> bool {
    img.sched.tasks.iter().any(|t| t.ctx as usize == cx + 1)
}

/// The ticks of the writer's clock in this one's.
fn conv_ticks(t: i64, unit: u16) -> i64 {
    let here = crate::builtins::ext_task::TICK_UNIT_MS as i64;
    if unit == 0 || unit as i64 == here { return t; }
    let ms = t * unit as i64;
    if ms >= 0 { (ms + here - 1) / here } else { ms / here }
}

/// The scheduler: its clock and its Tasks, in their queues in the image's order.
fn build_tasks(vm: &mut Vm, b: &Builder) -> R<()> {
    let s = &b.img.sched;
    let now = conv_ticks(s.tick as i64, s.unit) as u32;
    vm.task.tick = now;
    vm.task.tick_seen = now;
    vm.task.wakeup_tick = match s.wakeup { Some(w) => now.wrapping_add(conv_ticks(w as i64, s.unit) as u32), None => u32::MAX };
    let task_obj = |i: u32| -> R<ObjId> {
        let t = s.tasks.get((i as usize).wrapping_sub(1)).ok_or_else(|| format!("Task #{i} out of range"))?;
        b.obj(t.obj)
    };
    for (i, t) in s.tasks.iter().enumerate() {
        let o = b.obj(t.obj)?;
        if !matches!(vm.heap.get(o).kind, ObjKind::Task(_)) { return Err(format!("Task #{} has no Task object", i + 1)); }
        let main = t.queue == 255;
        let ctx = if main { ROOT } else {
            if t.ctx == 0 || t.ctx as usize > b.img.ctxs.len() { return Err(format!("Task #{} has no context", i + 1)); }
            b.ctxs[t.ctx as usize - 1]
        };
        let (wakeup, join, queue) = match &t.wait {
            WaitRec::None => (u32::MAX, None, None),
            WaitRec::Sleep(left) => (now.wrapping_add(conv_ticks(*left as i64, s.unit) as u32), None, None),
            WaitRec::Join(j) => (u32::MAX, if *j == 0 { None } else { Some(task_obj(*j)?) }, None),
            WaitRec::Queue(q, left) => (left.map_or(u32::MAX, |l| now.wrapping_add(conv_ticks(l as i64, s.unit) as u32)), None, b.obj_opt(*q)?),
        };
        let name = if main && matches!(t.name, Val::Nil) { vm.str_new(b"main") } else { b.val(t.name)? };
        let result = b.val(t.result)?;
        if let ObjKind::Task(td) = &mut vm.heap.get_mut(o).kind {
            **td = TaskData {
                ctx, priority: t.priority, status: t.status, reason: t.reason, timeslice: t.timeslice,
                name: Slot::from(name), result: Slot::from(result), wakeup_tick: wakeup, join, queue,
                instructions: 0, place: TaskPlace::NONE,
            };
        }
        if main { vm.task.main = Some(o); } else { crate::builtins::ext_task::checkpoint_enqueue(vm, o); }
    }
    Ok(())
}

/// Where the instruction that ends at `pc` starts (its `EXT` prefix included), and what it is.
fn insn_ending_at(vm: &Vm, irep: usize, pc: usize) -> Option<(usize, crate::opcode::Op, usize, usize, usize)> {
    use crate::opcode::{Op, Operands};
    let code = &vm.ireps.get(irep)?.iseq;
    let mut at = 0usize;
    while at < pc {
        let start = at;
        let mut op = Op::from_u8(*code.get(at)?)?;
        at += 1;
        let ext = match op { Op::Ext1 => 1, Op::Ext2 => 2, Op::Ext3 => 3, _ => 0 };
        if ext != 0 { op = Op::from_u8(*code.get(at)?)?; at += 1; }
        let (aw, bw) = (ext == 1 || ext == 3, ext == 2 || ext == 3);
        let rd = |at: &mut usize, wide: bool| -> Option<usize> {
            let v = if wide { ((*code.get(*at)? as usize) << 8) | *code.get(*at + 1)? as usize } else { *code.get(*at)? as usize };
            *at += if wide { 2 } else { 1 };
            Some(v)
        };
        let (mut a, mut bb, mut c) = (0, 0, 0);
        match op.operands() {
            Operands::Z => {}
            Operands::B => a = rd(&mut at, aw)?,
            Operands::BB => { a = rd(&mut at, aw)?; bb = rd(&mut at, bw)?; }
            Operands::BBB => { a = rd(&mut at, aw)?; bb = rd(&mut at, bw)?; c = rd(&mut at, false)?; }
            Operands::BS => { a = rd(&mut at, aw)?; bb = rd(&mut at, true)?; }
            Operands::BSS => { a = rd(&mut at, aw)?; bb = rd(&mut at, true)?; c = rd(&mut at, true)?; }
            Operands::S => a = rd(&mut at, true)?,
            Operands::W => at += 3,
        }
        if at == pc { return Some((start, op, a, bb, c)); }
    }
    None
}

/// The scheduler's caller: the context the running Task was entered from waits in `Task.run`
/// or `Task.pass` (or, with no frame, the host runs the loop).
fn entry_of(vm: &mut Vm, e: usize, wait: Option<&(String, usize)>) -> R<Entry> {
    use crate::opcode::Op;
    let (name, reg) = match wait {
        None if vm.contexts[e].ci.is_empty() && e == ROOT => return Ok(Entry::Host),
        None => return Err(String::from("the running Task was entered from a context that runs no scheduler")),
        Some((n, r)) => (n.as_str(), *r),
    };
    match name {
        "F:S:Task#pass" => Ok(Entry::Pass(reg)),
        "F:S:Task#run" => {
            let (irep, pc, base) = match vm.contexts[e].ci.last() { Some(t) => (t.irep, t.pc, t.base), None => return Err(String::from("Task.run with no caller")) };
            let run = vm.intern("run");
            let (start, op, a, bb, c) = insn_ending_at(vm, irep, pc).ok_or_else(|| String::from("the call of Task.run is not found"))?;
            let sends = matches!(op, Op::Send | Op::Sendb | Op::Ssend | Op::Ssendb | Op::Send0 | Op::Ssend0);
            let packs = matches!(op, Op::Send | Op::Sendb | Op::Ssend | Op::Ssendb) && !matches!((c >> 4) & 0xf, 0 | 15);
            if !sends || packs || vm.ireps[irep].syms.get(bb) != Some(&run) || base + a != reg {
                return Err(String::from("the call of Task.run cannot be made again here"));
            }
            vm.contexts[e].ci.last_mut().unwrap().pc = start;
            vm.contexts[e].pending_reg = None;
            Ok(Entry::Run)
        }
        n => Err(format!("the scheduler was entered from {n}")),
    }
}

/// Puts the context that goes on first in the VM's own fields (`Vm::stack`, `Vm::ci`, `cur`) and
/// says what goes on.
fn place(vm: &mut Vm, b: &Builder, waits: &[Option<(String, usize)>]) -> R<Plan> {
    let s = &b.img.sched;
    let current = b.ctxs[b.img.current as usize - 1];
    let (first, plan) = if s.running == 0 {
        (current, Plan::Context)
    } else {
        let t = s.tasks.get(s.running as usize - 1).ok_or_else(|| String::from("running Task out of range"))?;
        if t.queue == 255 { return Err(String::from("the main Task cannot be the running one")); }
        let task = b.obj(t.obj)?;
        let tctx = b.ctxs[t.ctx as usize - 1];
        let e = vm.contexts[tctx].prev.ok_or_else(|| String::from("the running Task has no context it was entered from"))?;
        let ecx = b.ctxs.iter().position(|x| *x == e).unwrap_or(0);
        let entry = entry_of(vm, e, waits[ecx].as_ref())?;
        if let Entry::Pass(_) = entry { vm.contexts[e].pending_reg = None; }
        (e, Plan::Slice { task, start: current, ended: s.switch_pending, entry })
    };
    // the running one's stack and frames are the VM's own while it runs
    vm.contexts[first].status = FiberState::Running;
    vm.stack = core::mem::take(&mut vm.contexts[first].stack);
    vm.ci = core::mem::take(&mut vm.contexts[first].ci);
    vm.cur = first;
    if vm.ci.is_empty() && !matches!(plan, Plan::Slice { entry: Entry::Host, .. }) { return Err(String::from("no frames")); }
    Ok(plan)
}

/// Builds the image into `vm`, which must be idle (no frame). On success the frames are in
/// place and [`Vm::checkpoint_run`] goes on from the top one.
pub fn build(vm: &mut Vm, img: &Image) -> R<Plan> {
    if !vm.ci.is_empty() || vm.cur != ROOT || vm.contexts.len() != 1 || (0..4).any(|q| !vm.task.queues[q].is_empty()) {
        return Err(String::from("restore needs an idle VM"));
    }
    let mut b = Builder { img, syms: vec![], ireps: vec![], objs: vec![], hash_entries: vec![], nested: vec![], ctxs: vec![] };
    b.syms = intern_syms(vm, img);
    let gc_was = core::mem::replace(&mut vm.gc_disabled, true);
    let r = (|| -> R<Plan> {
        build_ireps(vm, &mut b)?;
        make_objects(vm, &mut b)?;
        fill_objects(vm, &mut b)?;
        apply_patches(vm, &mut b)?;
        fix_nested(vm, &b)?;
        fill_hashes(vm, &b)?;
        freeze(vm, &b);
        let waits = build_contexts(vm, &mut b)?;
        build_tasks(vm, &b)?;
        place(vm, &b, &waits)
    })();
    vm.gc_disabled = gc_was;
    if r.is_err() {
        vm.stack.clear();
        vm.ci.clear();
        vm.cur = ROOT;
        vm.contexts.truncate(1);
        vm.contexts[ROOT].stack.clear();
        vm.contexts[ROOT].ci.clear();
        vm.contexts[ROOT].status = FiberState::Created;
        for q in 0..4 { vm.task.queues[q] = Default::default(); }
        vm.task.main = None;
    }
    r
}
