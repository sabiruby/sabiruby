//! Heap objects. Every non-immediate value is an entry in [`Heap`], addressed
//! by [`ObjId`]. Unreachable objects are reclaimed by a stop-the-world,
//! non-moving mark & sweep collector (`Vm::gc_collect`, see `docs/design/gc.md`):
//! the heap marks from the roots the VM hands it, sweeps what stayed white
//! and reuses the freed slots through a free list.

use alloc::vec::Vec;

use hashbrown::HashMap;

use crate::bigint::BigInt;
use crate::error::VmResult;
use crate::symbol::Sym;
use crate::value::{ObjId, Slot, Value};
use crate::vm::Vm;

/// Native (Rust) method: `fn(vm, self, args, block)`.
pub type NativeFn = fn(&mut Vm, Value, &[Value], Value) -> VmResult<Value>;

/// A native method that carries an environment: the same signature as [`NativeFn`], but a
/// closure the host built rather than a bare function. Registered with [`Vm::define_closure`].
///
/// `Arc` because [`Method`] is `Clone` (method lookup returns the method by value) and because
/// a `Vm` is `Send + Sync`; the closure must be `Send + Sync` for the same reason.
pub type NativeClosure = alloc::sync::Arc<ClosureBody>;

/// The closure behind a [`NativeClosure`], boxed so the `Arc` is a thin pointer: a `Method`
/// then stays 16 bytes, as it was before the variant existed, and method lookup (which clones
/// a `Method`) keeps copying one machine word pair.
pub struct ClosureBody {
    pub f: alloc::boxed::Box<dyn Fn(&mut Vm, Value, &[Value], Value) -> VmResult<Value> + Send + Sync>,
    /// What `Method#arity` answers for the method. A bare [`NativeFn`] is looked up in
    /// `Vm::native_arity` by its address, which a closure has no place in; it carries the
    /// number here instead. `-1` (`Vm::define_closure`) is "unknown", as it is for a native
    /// with no declared argument spec; `Vm::define_fn` knows it from the Rust signature.
    pub arity: i64,
}

/// Index of a loaded irep in [`Vm`].
pub type IrepId = usize;

#[derive(Clone)]
pub enum Method {
    /// A method written in Ruby: the Proc (irep + target class) to run.
    Ruby(ObjId),
    Native(NativeFn),
    /// A native method with an environment (`Vm::define_closure`). Everywhere a `Native` is
    /// dispatched, checked or described, a `Closure` behaves the same; what it cannot do is
    /// be compared by function address (`notimpl_fns`, `native_arity`, `Method#==` between
    /// two natives), where it compares by `Arc` identity or answers as an unknown native.
    Closure(NativeClosure),
    AttrReader(Sym),
    AttrWriter(Sym),
    /// `undef_method`: stops the lookup.
    Undef,
}

/// What a call site needs from a method lookup, without cloning the [`Method`].
///
/// `Method` gained a variant holding an `Arc` in stage 3, so cloning one stopped being a
/// plain copy: it branches on the discriminant, may touch a reference count, and — the part
/// that costs most on the dispatch path — gives the returned value drop glue. Every variant
/// but `Closure` is `Copy`; a `Closure` is named here only by its kind, and the one call site
/// that has to run it asks the owner's table for the `Arc` ([`crate::vm::Vm::closure_of`]),
/// which is the rare case. Lookups that want the `Method` itself still call `find_method`.
#[derive(Clone, Copy)]
pub enum MethodRef {
    Ruby(ObjId),
    Native(NativeFn),
    Closure,
    AttrReader(Sym),
    AttrWriter(Sym),
}

impl MethodRef {
    /// `None` for [`Method::Undef`], which stops a lookup rather than answering it.
    pub fn of(m: &Method) -> Option<MethodRef> {
        Some(match m {
            Method::Ruby(p) => MethodRef::Ruby(*p),
            Method::Native(f) => MethodRef::Native(*f),
            Method::Closure(_) => MethodRef::Closure,
            Method::AttrReader(s) => MethodRef::AttrReader(*s),
            Method::AttrWriter(s) => MethodRef::AttrWriter(*s),
            Method::Undef => return None,
        })
    }
}

#[derive(Default)]
pub struct ClassData {
    pub name: Option<Sym>,
    pub superclass: Option<ObjId>,
    pub methods: HashMap<Sym, Method>,
    pub consts: HashMap<Sym, Slot>,
    pub cvars: HashMap<Sym, Slot>,
    pub is_module: bool,
    pub is_singleton: bool,
    /// For a singleton class: the object it belongs to.
    pub attached: Option<Slot>,
    /// For an include class (mruby `MRB_TT_ICLASS`): the module whose
    /// method table and constants are shared.
    pub iclass_of: Option<ObjId>,
    /// Non-public methods of this table (absent = public).
    pub vis: HashMap<Sym, Vis>,
    /// For a class with prepended modules: the include class that now holds
    /// this class's own method table (mruby `MRB_FL_CLASS_IS_ORIGIN`).
    pub origin: Option<ObjId>,
    /// For an origin include class: the class it belongs to.
    pub origin_of: Option<ObjId>,
    /// Lexical parent for the name (`Outer::Inner`); `None` = top level.
    pub outer: Option<ObjId>,
    /// Representation of instances (copied by `Class#dup`); `None` = inherit.
    pub instance_kind: Option<InstanceKind>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vis { Public, Private, Protected }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstanceKind { Object, String, Array, Hash, Range, Exception, Proc, Fiber, NoAlloc }

#[derive(Clone)]
pub struct ProcData {
    pub irep: IrepId,
    /// The proc that was running when this one was created (`upper`).
    pub upper: Option<ObjId>,
    /// Captured environment (`REnv`), for blocks and lambdas.
    pub env: Option<ObjId>,
    pub target_class: Option<ObjId>,
    /// `MRB_PROC_STRICT`: methods and lambdas check arity; `return` returns from here.
    pub strict: bool,
    /// `MRB_PROC_SCOPE`: a method/class body (new scope for `return`).
    pub scope: bool,
    /// `MRB_PROC_ORPHAN`: the frame that created the block has returned.
    pub orphan: bool,
    /// For an alias of a Ruby method: the name it was defined under (`MRB_PROC_ALIAS`
    /// `body.mid`); the frame takes it as its `mid`.
    pub mid: Option<Sym>,
}

/// `REnv`: the locals a block can see. While the creating frame is alive the
/// values live on the VM stack (`attached`); when the frame is popped they are
/// copied into `values` (mruby's `mrb_env_detach`).
pub struct EnvData {
    /// The context (fiber) whose stack `base` indexes while `attached`.
    pub ctx: usize,
    pub base: usize,
    pub len: usize,
    /// Register (relative to `base`) holding the frame's block (`MRB_ENV_BIDX`).
    pub bidx: usize,
    pub attached: bool,
    pub values: Vec<Slot>,
    pub mid: Option<Sym>,
    pub target_class: Option<ObjId>,
    /// Default visibility / module_function state of the scope that owns this env
    /// (mruby keeps them in `REnv` flags so closures defined later still see them).
    pub vis: Vis,
    pub modfunc: bool,
    /// `instance_eval`/`class_eval` boundary (`MRB_ENV_VISIBILITY_BREAK`).
    pub vis_break: bool,
    /// The special variables of the scope this env belongs to (`struct RSvar`): `$~`
    /// (`MRB_SVAR_BACKREF`) and `$_` (`MRB_SVAR_LASTLINE`), which a block shares with the method
    /// it was written in. `None` is a scope that was never asked for one, which is what the
    /// reference's lazily allocated container is. `docs/design/gems.md` says why it sits here rather
    /// than on the frame.
    pub svar: Option<[Slot; SVAR_KEYS]>,
    /// Where a frame with no scope of its own sends its special variables once it has returned:
    /// the env of the scope below the load, which keeps the frame transparent past its own life
    /// (`svar_env_adopt_owner` / `mrb_svar_frame_container`).
    pub svar_fwd: Option<crate::value::ObjId>,
}

/// The special variables one scope holds, in the order `mrb_vm_svar_get` numbers them.
pub const SVAR_KEYS: usize = 2;
/// `$~` (`MRB_SVAR_BACKREF`).
pub const SVAR_BACKREF: usize = 0;
/// `$_` (`MRB_SVAR_LASTLINE`), which no global is registered for.
pub const SVAR_LASTLINE: usize = 1;

/// The elements of an Array: a buffer and the index in it where the elements start.
///
/// A plain `Vec<Slot>` makes `Array#shift` O(n) — `Vec::remove(0)` moves every remaining
/// element down one — which is what a queue written as `push`/`shift` pays on every step.
/// The reference does not: an `RArray` can point into a buffer it shares with another array,
/// and `shift` moves that pointer. Sharing is not reproduced here (two arrays never see each
/// other's writes, so nothing has to decide when to unshare); only the offset is, and it
/// lives in the array itself. That is on the "implementation you may choose" side: Ruby sees
/// the same elements in the same order, and `shift` becomes O(1).
///
/// `buf[..start]` is the room `shift` left in front. It is nil-filled, so the collector never
/// sees an element the array has already given up, and it is reclaimed by `compact` once it
/// has grown past the live elements — which takes as many `shift`s as the copy then costs, so
/// the amortized cost of `shift` stays constant.
///
/// The fields are private and the type derefs to `[Slot]`: every reader works on the slice
/// `buf[start..]` and cannot see the offset, and every writer goes through the methods below,
/// which are the only places that know about it.
#[derive(Clone, Default)]
pub struct ArrayData {
    buf: Vec<Slot>,
    start: usize,
}

/// Room left in front that `compact` tolerates before it reclaims it: below this an array is
/// too small for the copy to matter, and a `shift`-only array (which ends at `start == cap`,
/// `len == 0`) would otherwise copy on every step.
const ARY_SHIFT_SLACK: usize = 16;

impl ArrayData {
    pub fn new() -> ArrayData {
        ArrayData { buf: Vec::new(), start: 0 }
    }
    #[inline]
    pub fn len(&self) -> usize {
        self.buf.len() - self.start
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.buf.len() == self.start
    }
    #[inline]
    pub fn as_slice(&self) -> &[Slot] {
        &self.buf[self.start..]
    }
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [Slot] {
        &mut self.buf[self.start..]
    }
    /// Drops the room in front once it is bigger than the elements it precedes. Called after
    /// every `shift`; the copy is O(len) but at least `len` shifts had to happen to reach it.
    fn compact(&mut self) {
        if self.start > self.len() && self.start > ARY_SHIFT_SLACK {
            self.buf.drain(..self.start);
            self.start = 0;
        }
    }
    pub fn push(&mut self, v: Slot) {
        self.buf.push(v);
    }
    pub fn pop(&mut self) -> Option<Slot> {
        if self.is_empty() { None } else { self.buf.pop() }
    }
    pub fn clear(&mut self) {
        self.buf.clear();
        self.start = 0;
    }
    pub fn truncate(&mut self, len: usize) {
        self.buf.truncate(self.start + len);
    }
    pub fn resize(&mut self, len: usize, v: Slot) {
        self.buf.resize(self.start + len, v);
    }
    pub fn extend_from_slice(&mut self, other: &[Slot]) {
        self.buf.extend_from_slice(other);
    }
    /// `Vec::split_off`: the elements from `at` on, as a fresh buffer.
    pub fn split_off(&mut self, at: usize) -> Vec<Slot> {
        self.buf.split_off(self.start + at)
    }
    /// `Array#shift`: the first element in O(1).
    pub fn shift(&mut self) -> Option<Slot> {
        if self.is_empty() { return None; }
        let v = core::mem::replace(&mut self.buf[self.start], Slot::NIL);
        self.start += 1;
        self.compact();
        Some(v)
    }
    /// `Array#shift(n)`: the first `n` elements (or all of them) in O(n) for the copy out and
    /// O(1) for the array itself.
    pub fn shift_n(&mut self, n: usize) -> Vec<Slot> {
        let n = n.min(self.len());
        let out = self.buf[self.start..self.start + n].to_vec();
        for s in &mut self.buf[self.start..self.start + n] { *s = Slot::NIL; }
        self.start += n;
        self.compact();
        out
    }
    /// `Array#unshift`: O(1) when `shift` has left enough room in front, else a splice.
    pub fn unshift(&mut self, items: Vec<Slot>) {
        if items.len() <= self.start {
            self.start -= items.len();
            self.buf[self.start..self.start + items.len()].copy_from_slice(&items);
        } else {
            self.buf.splice(self.start..self.start, items);
        }
    }
    pub fn insert(&mut self, i: usize, v: Slot) {
        if i == 0 && self.start > 0 {
            self.start -= 1;
            self.buf[self.start] = v;
        } else {
            self.buf.insert(self.start + i, v);
        }
    }
    pub fn remove(&mut self, i: usize) -> Slot {
        if i == 0 { return self.shift().expect("remove(0) on an empty array"); }
        self.buf.remove(self.start + i)
    }
    /// `Vec::drain(from..to)`, collected: the elements removed, in order.
    pub fn drain_range(&mut self, from: usize, to: usize) -> Vec<Slot> {
        if from == 0 { return self.shift_n(to); }
        self.buf.drain(self.start + from..self.start + to).collect()
    }
    /// `Vec::splice(from..to, items)` with the return dropped.
    pub fn splice_range(&mut self, from: usize, to: usize, items: Vec<Slot>) {
        if from == 0 && to == 0 { return self.unshift(items); }
        self.buf.splice(self.start + from..self.start + to, items);
    }
}

impl From<Vec<Slot>> for ArrayData {
    fn from(buf: Vec<Slot>) -> ArrayData {
        ArrayData { buf, start: 0 }
    }
}

impl FromIterator<Slot> for ArrayData {
    fn from_iter<I: IntoIterator<Item = Slot>>(iter: I) -> ArrayData {
        ArrayData { buf: iter.into_iter().collect(), start: 0 }
    }
}

impl Extend<Slot> for ArrayData {
    fn extend<I: IntoIterator<Item = Slot>>(&mut self, iter: I) {
        self.buf.extend(iter);
    }
}

impl core::ops::Deref for ArrayData {
    type Target = [Slot];
    #[inline]
    fn deref(&self) -> &[Slot] {
        self.as_slice()
    }
}

impl core::ops::DerefMut for ArrayData {
    #[inline]
    fn deref_mut(&mut self) -> &mut [Slot] {
        self.as_mut_slice()
    }
}

/// A Hash: its entries in insertion order, the hash code of each key beside them, and the
/// value `Hash#default` answers with.
///
/// `entries` and `hashes` are private, and every read and write goes through one of the
/// methods below. That is not tidiness: the cached hash codes have to stay aligned with the
/// entries, and anything derived from the entries' *positions* -- a lookup index, say -- has
/// to be thrown away whenever a removal moves them. Spread over the forty-odd places that
/// used to reach into the fields, one of them would eventually forget. The same reasoning
/// made `Heap::class_mut` the single way to reach a `ClassData`: the method cache's
/// invalidation hangs off it.
#[derive(Clone, Default)]
pub struct HashData {
    /// Insertion-ordered entries. The order is this vector's, whatever else is built
    /// beside it, which is why `Hash#each` never has to sort anything.
    entries: Vec<(Slot, Slot)>,
    /// `hash` of each key, parallel to `entries` (rebuilt lazily when lengths differ).
    hashes: Vec<i64>,
    /// `hash code -> one entry with that code`, built once the hash has more than
    /// [`HASH_INDEX_THRESHOLD`] entries. This is the switch mruby makes from its "array"
    /// representation to a hash table at the same size, except that insertion order does
    /// not have to be stored anywhere: `entries` already has it.
    ///
    /// `None` while the hash is small, while the hash codes are stale (hashing a key can
    /// run Ruby, which the writes that replace every entry cannot do), and from a removal
    /// until it is rebuilt.
    index: Option<HashMap<i64, u32>>,
    /// `chain[i]` is another entry whose key hashes like entry `i`'s, or [`NO_ENTRY`].
    /// Keys in a hash are unique under `eql?`, so at most one entry of a chain can match
    /// and the order they come back in does not matter.
    chain: Vec<u32>,
    pub default: Slot,
}

/// Entries above which a lookup builds an index instead of walking the cached hash codes.
/// mruby switches an `RHash` from its "array" representation to a hash table at the same
/// size (`AR_DEFAULT_LEN` doubles up to 16), and for the same reason: below it the walk
/// over one `i64` slice is cheaper than a table lookup, and the table's memory is not worth
/// paying for the small hashes most programs are made of.
pub const HASH_INDEX_THRESHOLD: usize = 16;

/// End of a chain in [`HashData`]'s index.
const NO_ENTRY: u32 = u32::MAX;

impl HashData {
    /// A hash holding `entries`, whose key hash codes the first lookup fills in
    /// (`hashes_stale`).
    pub fn from_entries(entries: Vec<(Slot, Slot)>, default: Slot) -> HashData {
        HashData { entries, hashes: Vec::new(), index: None, chain: Vec::new(), default }
    }
    /// The entries in insertion order.
    #[inline]
    pub fn entries(&self) -> &[(Slot, Slot)] {
        &self.entries
    }
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// True while the cached hash codes do not describe the entries. Hashing a key can run
    /// Ruby (`hash` is a method), which a `&mut HashData` cannot do, so the writes that
    /// replace every entry leave the codes empty and [`Vm::hash_sync`](crate::vm::Vm) fills
    /// them in at the next lookup.
    #[inline]
    pub fn hashes_stale(&self) -> bool {
        self.hashes.len() != self.entries.len()
    }
    /// The hash code cached for entry `i`, if there is one.
    #[inline]
    pub fn hash_at(&self, i: usize) -> Option<i64> {
        self.hashes.get(i).copied()
    }
    /// Where a lookup for a key hashing to `kh` starts: the first entry whose key hashes the
    /// same, as `(position, key)`.
    ///
    /// A lookup only ever asks these two questions, and each answer is all it needs to leave
    /// the borrow again: verifying a candidate means calling `eql?`, which can be Ruby, so
    /// nothing may be held across it.
    pub fn first_candidate(&self, kh: i64) -> Option<(usize, Slot)> {
        match &self.index {
            Some(ix) => self.at(*ix.get(&kh)?),
            None => self.scan_from(kh, 0),
        }
    }
    /// The next entry whose key hashes to `kh`, for a lookup that has just rejected the one
    /// at `p`. Keys in a hash are unique under `eql?`, so the order these come back in does
    /// not matter — at most one of them can match.
    pub fn next_candidate(&self, p: usize, kh: i64) -> Option<(usize, Slot)> {
        match &self.index {
            // `eql?` may have run Ruby, which may have edited this hash, so the chain is
            // read defensively: a position that is no longer there ends the walk.
            Some(_) => self.at(*self.chain.get(p)?),
            None => self.scan_from(kh, p + 1),
        }
    }
    fn at(&self, p: u32) -> Option<(usize, Slot)> {
        if p == NO_ENTRY { return None; }
        let p = p as usize;
        Some((p, self.entries.get(p)?.0))
    }
    fn scan_from(&self, kh: i64, from: usize) -> Option<(usize, Slot)> {
        let n = self.hashes.len().min(self.entries.len());
        if from >= n { return None; }
        let d = self.hashes[from..n].iter().position(|c| *c == kh)?;
        Some((from + d, self.entries[from + d].0))
    }
    /// Builds the index, or drops it when the hash is too small for one or its hash codes
    /// are not usable. Every write that could have invalidated the index ends here.
    fn reindex(&mut self) {
        if self.entries.len() <= HASH_INDEX_THRESHOLD || self.hashes_stale() {
            self.index = None;
            self.chain = Vec::new();
            return;
        }
        let mut ix: HashMap<i64, u32> = HashMap::with_capacity(self.entries.len());
        let mut chain: Vec<u32> = Vec::new();
        chain.resize(self.entries.len(), NO_ENTRY);
        for (i, kh) in self.hashes.iter().enumerate() {
            // the new entry becomes the head of its chain and points at the old head
            chain[i] = ix.insert(*kh, i as u32).unwrap_or(NO_ENTRY);
        }
        self.index = Some(ix);
        self.chain = chain;
    }
    /// Appends an entry that is known not to be in the hash yet, with its key's hash code.
    pub fn push_entry(&mut self, k: Slot, v: Slot, kh: i64) {
        let at = self.entries.len() as u32;
        self.entries.push((k, v));
        self.hashes.push(kh);
        match &mut self.index {
            Some(ix) => self.chain.push(ix.insert(kh, at).unwrap_or(NO_ENTRY)),
            None => self.reindex(),
        }
    }
    /// Overwrites the value of entry `i`; the key and its hash code stay.
    pub fn set_value_at(&mut self, i: usize, v: Slot) {
        self.entries[i].1 = v;
    }
    /// Removes entry `i` and its hash code, keeping the order of the rest.
    pub fn remove_entry(&mut self, i: usize) -> (Slot, Slot) {
        if i < self.hashes.len() { self.hashes.remove(i); }
        let e = self.entries.remove(i);
        // every position after `i` moved down one, so every chain naming one is wrong.
        // Rebuilding is O(n), which is what `Vec::remove` just cost anyway.
        if self.index.is_some() { self.reindex(); }
        e
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.hashes.clear();
        self.index = None;
        self.chain = Vec::new();
    }
    /// Replaces every entry. The hash codes are dropped: the caller is handing over keys it
    /// did not hash (`replace`, `initialize_copy`, `merge`), and hashing them needs the VM.
    pub fn set_entries(&mut self, entries: Vec<(Slot, Slot)>) {
        self.entries = entries;
        self.hashes.clear();
        self.index = None;
        self.chain = Vec::new();
    }
    /// Replaces every entry together with the hash codes the caller already computed for
    /// them (`rehash`, `compact!`), which saves the next lookup from hashing them again.
    pub fn set_entries_with_hashes(&mut self, entries: Vec<(Slot, Slot)>, hashes: Vec<i64>) {
        self.entries = entries;
        self.hashes = hashes;
        self.reindex();
    }
    /// Fills in the hash codes `hashes_stale` asked for.
    pub fn set_hashes(&mut self, hashes: Vec<i64>) {
        self.hashes = hashes;
        self.reindex();
    }
}

impl Default for Value {
    fn default() -> Self {
        Value::Nil
    }
}

/// One task of mruby-task's scheduler (`mrb_task`). The queues it moves between are the VM's
/// (`Vm::task`), and its execution context is one of `Vm::contexts`, as a Fiber's is.
pub struct TaskData {
    /// index into `Vm::contexts`, or `usize::MAX` before the task was given one
    pub ctx: usize,
    /// 0-255, 0 highest (`MRB_TASK_PRIORITY_DEFAULT` is 128)
    pub priority: u8,
    /// `MRB_TASK_STATUS_*`
    pub status: u8,
    /// `MRB_TASK_REASON_*`
    pub reason: u8,
    /// ticks left of this task's timeslice while it runs
    pub timeslice: u8,
    /// the name `Task.new(name:)` was given, or nil
    pub name: Slot,
    /// what the block answered, or the exception it raised and did not handle
    pub result: Slot,
    /// tick to wake at (`MRB_TASK_REASON_SLEEP`, or a queue wait with a timeout);
    /// `u32::MAX` is "no timed wakeup"
    pub wakeup_tick: u32,
    /// the task this one is joining (`MRB_TASK_REASON_JOIN`)
    pub join: Option<ObjId>,
    /// the `Task::Queue` this one is waiting on (`MRB_TASK_REASON_QUEUE`)
    pub queue: Option<ObjId>,
    /// instructions this task has run, for a host that shows what a script spends
    /// (`Vm::task_instructions`)
    pub instructions: u64,
}

/// Why a `Break` object is unwinding the stack (mruby `RBREAK_TAG_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakTag {
    /// `return` (and `throw`): unwind to frame `ci_index` and return `value` from it.
    Break,
    /// `break` out of a block: the same, and the value is the answer even of a frame that
    /// answers its receiver (`Cci::KeepSelf`: `Foo.new { break 1 }` is 1).
    BlockBreak,
    /// `OP_JMPUW`: after the ensure body, jump to pc `value` (an Integer).
    Jump,
}

pub enum ObjKind {
    Object,
    /// mruby `RBreak`: a non-local exit that must first run `ensure` bodies.
    /// Never visible to Ruby code (only passes through EXCEPT/RAISEIF).
    Break { tag: BreakTag, ci_index: usize, value: Value },
    Class(ClassData),
    String(Vec<u8>),
    Array(ArrayData),
    Hash(HashData),
    Range { begin: Slot, end: Slot, excl: bool },
    Proc(ProcData),
    Env(EnvData),
    Exception,
    /// `RFiber`: index of the fiber's context in `Vm::contexts` (`usize::MAX` = not initialized).
    Fiber(usize),
    /// mruby-task's `mrb_task`: one runnable unit, its context among the fibers' and the rest
    /// the scheduler's bookkeeping (`docs/design/gems.md`).
    Task(alloc::boxed::Box<TaskData>),
    /// mruby `RBigint`: an Integer too wide for `Value::Int`. Its class is `Integer`, and a
    /// value that fits in an `i64` is never stored as one (`bint_norm`, see `docs/design/gems.md`).
    BigInt(BigInt),
    /// mruby-regexp's compiled pattern, which a Regexp owns (`mrb_regexp_pattern` behind its
    /// `DATA_PTR`); `@source`, `@flags` and `@named_captures` are ivars as they are there.
    /// A compiled pattern, or `None` while `initialize` holds the slot and the compile has not
    /// finished: the variant says the object was initialized at all, as `DATA_PTR` does, and the
    /// payload whether there is anything to search with (`re_uninitialized_p`).
    #[cfg(feature = "regexp")]
    Regexp(Option<alloc::sync::Arc<crate::regexp::Pattern>>),
    /// mruby-regexp's `MatchData`: the subject as it was at match time, the Regexp that made the
    /// match (nil for a quoted String pattern), and the capture positions in bytes.
    #[cfg(feature = "regexp")]
    MatchData { source: Slot, regexp: Slot, captures: alloc::vec::Vec<i32> },
    /// A value the host owns, named by a handle (mruby's `RData` and its `DATA_PTR`, without
    /// the pointer): `tag` says which kind of thing the host put there and `handle` which one.
    /// The VM never looks inside either; it only carries them, compares them (`==`, `eql?`,
    /// `hash`) and tells the host when the object is collected
    /// ([`Vm::set_on_free`](crate::Vm::set_on_free)). Built with
    /// [`Vm::data_new`](crate::Vm::data_new).
    Data { tag: u32, handle: u64 },
}

pub struct HeapObject {
    pub class: ObjId,
    pub ivars: Vec<(Sym, Slot)>,
    pub frozen: bool,
    /// `MRB_STR_ENCODING_BINARY`: a String made byte-read by `String#b`, which counts and cuts
    /// one position per byte whatever its bytes are. Meaningless for every other kind.
    pub binary: bool,
    pub kind: ObjKind,
}

/// `flags` bit: reached in the current mark phase.
const MARKED: u8 = 1;
/// `flags` bit: the slot is on the free list.
const FREE: u8 = 2;
/// Estimated bytes of one object header, for `malloc_increase`.
const OBJ_BYTES: usize = 64;
/// Fewest allocations between two automatic collections.
pub const GC_MIN_INTERVAL: usize = 4096;

pub struct Heap {
    objs: Vec<HeapObject>,
    /// `MARKED` / `FREE` per slot, parallel to `objs`.
    flags: Vec<u8>,
    /// Free slots, lowest index last (reused first).
    free: Vec<u32>,
    /// Allocations since the last collection.
    pub allocated_since_gc: usize,
    /// `allocated_since_gc` at which a collection becomes due.
    pub alloc_threshold: usize,
    /// Estimated bytes allocated since the last collection (`GC.stat[:malloc_increase]`).
    pub malloc_increase: usize,
    /// `GC.malloc_threshold`; 0 = the byte axis is off.
    pub malloc_threshold: usize,
    /// A collection is due; the VM runs it at the next instruction boundary.
    pub gc_pending: bool,
    /// `(tag, handle)` of every [`ObjKind::Data`] the last sweep freed, for the VM to hand to
    /// the host's free hook once the collection is over (`Vm::gc_collect`). The heap cannot
    /// call the hook itself: it is in the middle of rebuilding the free list.
    pub freed_data: Vec<(u32, u64)>,
    /// Bumped whenever a method lookup could answer differently than before: any mutable
    /// access to a `ClassData` (its table, its visibilities, its superclass, its origin or
    /// its iclass) and every allocation of a class (a collected class's `ObjId` comes back
    /// as a different class). [`crate::vm::Vm`]'s method cache keeps this number beside each
    /// entry and throws the entry away when it no longer matches.
    pub method_serial: u64,
}

impl Default for Heap {
    fn default() -> Heap {
        Heap { objs: Vec::new(), flags: Vec::new(), free: Vec::new(), allocated_since_gc: 0, alloc_threshold: GC_MIN_INTERVAL, malloc_increase: 0, malloc_threshold: 16777216, gc_pending: false, freed_data: Vec::new(), method_serial: 1 }
    }
}

/// Estimated payload bytes of a new object (strings, arrays and hashes only).
fn payload_bytes(kind: &ObjKind) -> usize {
    match kind {
        ObjKind::String(s) => s.len(),
        ObjKind::Array(a) => 16 * a.len(),
        ObjKind::Hash(h) => 40 * h.len(),
        ObjKind::BigInt(b) => 4 * b.mag.len(),
        _ => 0,
    }
}

impl Heap {
    /// Never collects: at most it flags a collection as due (`gc_pending`).
    pub fn alloc(&mut self, class: ObjId, kind: ObjKind) -> ObjId {
        self.allocated_since_gc += 1;
        self.malloc_increase += OBJ_BYTES + payload_bytes(&kind);
        if self.allocated_since_gc >= self.alloc_threshold || (self.malloc_threshold != 0 && self.malloc_increase > self.malloc_threshold) {
            self.gc_pending = true;
        }
        // a class coming back on a reused slot must not answer to a cached lookup of the
        // class that used to live there
        if matches!(kind, ObjKind::Class(_)) { self.method_serial += 1; }
        let o = HeapObject { class, ivars: Vec::new(), frozen: false, binary: false, kind };
        match self.free.pop() {
            Some(i) => {
                self.flags[i as usize] = 0;
                self.objs[i as usize] = o;
                ObjId(i)
            }
            None => {
                let id = ObjId(self.objs.len() as u32);
                self.objs.push(o);
                self.flags.push(0);
                id
            }
        }
    }
    /// Allocates a class object whose own class is filled in later.
    pub fn alloc_raw(&mut self, kind: ObjKind) -> ObjId {
        self.alloc(ObjId(u32::MAX), kind)
    }
    #[inline]
    pub fn get(&self, id: ObjId) -> &HeapObject {
        debug_assert!(!self.is_free(id), "access to freed object {:?}", id);
        &self.objs[id.0 as usize]
    }
    #[inline]
    pub fn get_mut(&mut self, id: ObjId) -> &mut HeapObject {
        debug_assert!(!self.is_free(id), "access to freed object {:?}", id);
        &mut self.objs[id.0 as usize]
    }
    /// Number of slots (live and free). Slot ids are below this.
    pub fn len(&self) -> usize {
        self.objs.len()
    }
    /// Every live object (`mrb_objspace_each_objects`), lowest id first.
    pub fn ids(&self) -> impl Iterator<Item = ObjId> + '_ {
        (0..self.objs.len()).map(|i| ObjId(i as u32)).filter(move |id| !self.is_free(*id))
    }
    pub fn is_empty(&self) -> bool {
        self.objs.is_empty()
    }
    /// Objects in use (`GC.stat[:live]`).
    pub fn live_count(&self) -> usize {
        self.objs.len() - self.free.len()
    }
    /// True for a slot that was swept and not reused yet.
    #[inline]
    pub fn is_free(&self, id: ObjId) -> bool {
        // a slot beyond the end was truncated by the sweep: free as well
        self.flags.get(id.0 as usize).is_none_or(|f| f & FREE != 0)
    }

    // ------------------------------------------------------------------ collection

    /// Marks `id` grey: pushes it on `work` unless already marked.
    #[inline]
    pub fn mark_id(&mut self, id: ObjId, work: &mut Vec<ObjId>) {
        let f = &mut self.flags[id.0 as usize];
        if *f & MARKED != 0 { return; }
        assert!(*f & FREE == 0, "GC: live reference to freed object {:?}", id);
        *f |= MARKED;
        work.push(id);
    }
    #[inline]
    pub fn mark_value(&mut self, v: Value, work: &mut Vec<ObjId>) {
        if let Value::Obj(o) = v { self.mark_id(o, work); }
    }
    pub fn mark_slots(&mut self, slots: &[Slot], work: &mut Vec<ObjId>) {
        for s in slots { self.mark_value(s.get(), work); }
    }
    /// True when the current mark phase reached `id`.
    pub fn is_marked(&self, id: ObjId) -> bool {
        self.flags[id.0 as usize] & MARKED != 0
    }
    /// Blackens the grey objects in `work` until none is left. What lives on
    /// context stacks (not heap objects) is handed back for the VM to scan:
    /// contexts reached through a Fiber go to `ctxs`, and the stack window
    /// `(ctx, base, len)` of an attached environment goes to `windows`.
    pub fn mark_drain(&mut self, work: &mut Vec<ObjId>, ctxs: &mut Vec<usize>, windows: &mut Vec<(usize, usize, usize)>) {
        while let Some(id) = work.pop() {
            // `objs` is only read and `flags` only written here: split the borrow.
            let Heap { objs, flags, .. } = self;
            let o = &objs[id.0 as usize];
            let mut mark = |v: Value| {
                if let Value::Obj(x) = v {
                    let f = &mut flags[x.0 as usize];
                    if *f & MARKED == 0 {
                        assert!(*f & FREE == 0, "GC: {:?} refers to freed object {:?}", id, x);
                        *f |= MARKED;
                        work.push(x);
                    }
                }
            };
            if o.class.0 != u32::MAX { mark(Value::Obj(o.class)); }
            for (_, v) in &o.ivars { mark(v.get()); }
            match &o.kind {
                // a Data holds a handle, not a Value: nothing in it is a reference
                ObjKind::Object | ObjKind::String(_) | ObjKind::Exception | ObjKind::BigInt(_) | ObjKind::Data { .. } => {}
                #[cfg(feature = "regexp")]
                ObjKind::Regexp(_) => {}
                ObjKind::Break { value, .. } => mark(*value),
                ObjKind::Array(a) => { for v in a.iter() { mark(v.get()); } }
                ObjKind::Hash(h) => {
                    for (k, v) in h.entries() { mark(k.get()); mark(v.get()); }
                    mark(h.default.get());
                }
                ObjKind::Range { begin, end, .. } => { mark(begin.get()); mark(end.get()); }
                ObjKind::Proc(p) => {
                    for x in [p.upper, p.env, p.target_class].into_iter().flatten() { mark(Value::Obj(x)); }
                }
                #[cfg(feature = "regexp")]
                ObjKind::MatchData { source, regexp, .. } => { mark(source.get()); mark(regexp.get()); }
                ObjKind::Env(e) => {
                    for v in &e.values { mark(v.get()); }
                    for v in e.svar.iter().flatten() { mark(v.get()); }
                    if let Some(f) = e.svar_fwd { mark(Value::Obj(f)); }
                    if let Some(t) = e.target_class { mark(Value::Obj(t)); }
                    // the values of an attached env live on its context's stack; only
                    // that window is kept (mruby marks `e->stack[0..len]`), not the fiber
                    if e.attached { windows.push((e.ctx, e.base, e.len)); }
                }
                ObjKind::Class(c) => {
                    for x in [c.superclass, c.iclass_of, c.origin, c.origin_of, c.outer].into_iter().flatten() { mark(Value::Obj(x)); }
                    for m in c.methods.values() { if let Method::Ruby(p) = m { mark(Value::Obj(*p)); } }
                    for v in c.consts.values() { mark(v.get()); }
                    for v in c.cvars.values() { mark(v.get()); }
                    if let Some(a) = c.attached { mark(a.get()); }
                }
                ObjKind::Fiber(ctx) => { if *ctx != usize::MAX { ctxs.push(*ctx); } }
                ObjKind::Task(t) => {
                    if t.ctx != usize::MAX { ctxs.push(t.ctx); }
                    mark(t.name.get());
                    mark(t.result.get());
                    for x in [t.join, t.queue].into_iter().flatten() { mark(Value::Obj(x)); }
                }
            }
        }
    }
    /// Frees every slot the mark phase did not reach and clears the marks.
    /// Trailing free slots are dropped from the table. Returns the number freed.
    pub fn sweep(&mut self) -> usize {
        let mut freed = 0;
        for i in 0..self.objs.len() {
            let f = self.flags[i];
            if f & MARKED != 0 {
                self.flags[i] = 0;
            } else if f & FREE == 0 {
                // the host's value goes with the object; the hook is called after the sweep,
                // when the VM is whole again (`Vm::gc_collect`)
                if let ObjKind::Data { tag, handle } = self.objs[i].kind { self.freed_data.push((tag, handle)); }
                // drop the payload; ObjKind::Object with class 0 is inert if touched by mistake
                self.objs[i] = HeapObject { class: ObjId(0), ivars: Vec::new(), frozen: false, binary: false, kind: ObjKind::Object };
                self.flags[i] = FREE;
                freed += 1;
            }
        }
        let mut len = self.objs.len();
        while len > 0 && self.flags[len - 1] & FREE != 0 { len -= 1; }
        self.objs.truncate(len);
        self.flags.truncate(len);
        if self.objs.capacity() > 4 * len + 4096 {
            self.objs.shrink_to(2 * len + 1024);
            self.flags.shrink_to(2 * len + 1024);
        }
        self.free.clear();
        for i in (0..len).rev() {
            if self.flags[i] & FREE != 0 { self.free.push(i as u32); }
        }
        freed
    }
    pub fn class(&self, id: ObjId) -> &ClassData {
        match &self.get(id).kind {
            ObjKind::Class(c) => c,
            _ => panic!("object {:?} is not a class", id),
        }
    }
    /// Mutable access to a class. Every caller is a potential change to what a lookup finds
    /// — a `def`, an `include`, an `undef_method`, a visibility — so this is where the method
    /// cache is invalidated, once, rather than at each of the forty-odd call sites.
    pub fn class_mut(&mut self, id: ObjId) -> &mut ClassData {
        self.method_serial += 1;
        match &mut self.get_mut(id).kind {
            ObjKind::Class(c) => c,
            _ => panic!("object {:?} is not a class", id),
        }
    }
    pub fn is_class(&self, id: ObjId) -> bool {
        matches!(self.get(id).kind, ObjKind::Class(_))
    }
    pub fn ivar_get(&self, id: ObjId, name: Sym) -> Value {
        self.get(id).ivars.iter().find(|(n, _)| *n == name).map(|(_, v)| v.get()).unwrap_or(Value::Nil)
    }
    pub fn ivar_set(&mut self, id: ObjId, name: Sym, v: Value) {
        let o = self.get_mut(id);
        if let Some(e) = o.ivars.iter_mut().find(|(n, _)| *n == name) {
            e.1 = Slot::from(v);
        } else {
            o.ivars.push((name, Slot::from(v)));
        }
    }
    pub fn bigint(&self, id: ObjId) -> Option<&BigInt> {
        match &self.get(id).kind {
            ObjKind::BigInt(b) => Some(b),
            _ => None,
        }
    }
    pub fn string(&self, id: ObjId) -> Option<&[u8]> {
        match &self.get(id).kind {
            ObjKind::String(s) => Some(s),
            _ => None,
        }
    }
    pub fn array(&self, id: ObjId) -> Option<&[Slot]> {
        match &self.get(id).kind {
            ObjKind::Array(a) => Some(a),
            _ => None,
        }
    }
    pub fn proc_data(&self, id: ObjId) -> &ProcData {
        match &self.get(id).kind {
            ObjKind::Proc(p) => p,
            _ => panic!("object {:?} is not a proc", id),
        }
    }
    pub fn env(&self, id: ObjId) -> &EnvData {
        match &self.get(id).kind {
            ObjKind::Env(e) => e,
            _ => panic!("object {:?} is not an env", id),
        }
    }
    pub fn env_mut(&mut self, id: ObjId) -> &mut EnvData {
        match &mut self.get_mut(id).kind {
            ObjKind::Env(e) => e,
            _ => panic!("object {:?} is not an env", id),
        }
    }
}
