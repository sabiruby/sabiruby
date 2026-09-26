//! The interpreter: register machine executing mruby 4.1.0 bytecode.
//!
//! Frame layout follows mruby: a callee's `R0` is the caller's `R[a]`
//! (`base = caller.base + a`), so a method's return value lands in the
//! caller's target register by writing `stack[callee.base]`.

use alloc::{format, string::String, string::ToString, vec, vec::Vec};

use hashbrown::HashMap;

use crate::error::{VmError, VmResult};
use crate::object::{BreakTag, ClassData, EnvData, Heap, InstanceKind, IrepId, Method, MethodRef, ObjKind, ProcData, Vis, GC_MIN_INTERVAL};
use crate::inspect::{CatchHandlerInfo, DetachReason, SwitchKind, TraceEvent, UnwindBy};
use crate::opcode::{Op, Operands};
use crate::rite::{self, CatchType, Pool};
use crate::symbol::{Interner, Sym};
use crate::value::{slots_of, values_of, ObjId, Slot, Value};

pub struct VmIrep {
    pub nlocals: usize,
    pub nregs: usize,
    pub iseq: Vec<u8>,
    pub catch: Vec<rite::CatchHandler>,
    pub pool: Vec<Pool>,
    pub syms: Vec<Sym>,
    pub reps: Vec<IrepId>,
    pub lv: Vec<Option<Sym>>,
    /// `(start_pc, line)` from the DBG section (`mrbc -g`); empty without it.
    pub lines: Vec<(u32, u32)>,
    /// Source file name from the DBG section.
    pub filename: Option<String>,
}

impl VmIrep {
    /// The source line of `pc` (mruby `mrb_debug_get_line`).
    pub fn line_of(&self, pc: usize) -> Option<u32> {
        let pc = pc as u32;
        match self.lines.partition_point(|(start, _)| *start <= pc) {
            0 => None,
            i => Some(self.lines[i - 1].1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cci {
    /// Ordinary Ruby frame.
    None,
    /// Frame started from native code (`Vm::call_proc`); the interpreter loop
    /// returns to the native caller when this frame is popped.
    Skip,
    /// Ordinary frame that answers its R0 — the receiver — whatever it returns, unless a
    /// `break` ends it: `initialize` under `Class#new`, the block of `Class.new { }` (the
    /// tail of the reference's `new_iseq`, without a frame of its own: `Vm::keep_self`).
    KeepSelf,
}

/// `mrb_fiber_state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FiberState { Created, Running, Resumed, Suspended, Transferred, Terminated }

/// `mrb_context`: one register stack and one frame stack. The running
/// context's `stack`/`ci` live in the `Vm` fields; the entry here is empty
/// while it runs (they are swapped on every switch).
pub struct Context {
    pub stack: Vec<Slot>,
    pub ci: Vec<CallInfo>,
    pub status: FiberState,
    /// The context to return to on `Fiber.yield` / termination (`prev`).
    pub prev: Option<usize>,
    /// The Fiber object of this context, made lazily by `Fiber.current` for the root.
    pub fib: Option<ObjId>,
    /// The block the fiber runs (set by `Fiber#initialize`).
    pub proc_: Option<ObjId>,
    /// Resumed by native code (`mrb_fiber_resume`): a nested run loop is
    /// waiting on the host stack and the fiber's next yield must return from it.
    pub vmexec: bool,
    /// Register (absolute index into `stack`) of the `resume`/`yield`/`transfer`
    /// call this context is suspended in; the value it is switched back with
    /// lands there (mruby writes it to the pending C frame's `stack[0]`).
    pub pending_reg: Option<usize>,
}

impl Context {
    pub fn new(status: FiberState) -> Context {
        Context { stack: Vec::new(), ci: Vec::new(), status, prev: None, fib: None, proc_: None, vmexec: false, pending_reg: None }
    }
}

/// Index of the root context in `Vm::contexts`.
pub const ROOT: usize = 0;

/// mruby-task's scheduler state (`mrb_task_state`). The queues hold the Task objects, which is
/// what keeps a task the program dropped every other reference to alive.
#[derive(Default)]
pub struct TaskState {
    /// dormant, ready, waiting, suspended — the ready one sorted by priority, FIFO within one
    pub queues: [Vec<ObjId>; 4],
    /// ticks since the scheduler started (`MRB_TICK_UNIT` milliseconds apiece)
    pub tick: u32,
    /// the earliest tick a waiting task asked to be woken at; `u32::MAX` where none did
    pub wakeup_tick: u32,
    /// a switch is due at the next instruction boundary (the timeslice ran out, or a task the
    /// running one woke has a higher priority)
    pub switching: bool,
    /// `Task.run` is already running, so a nested one answers nil (`loop_running`)
    pub loop_running: bool,
    /// The task the scheduler handed the CPU to, which is the one a `sleep` or a `Task.pass`
    /// from a task context belongs to.
    pub running: Option<ObjId>,
    /// Instructions between two ticks. The reference's tick comes from a timer interrupt, which
    /// a `no_std` VM has none of; here it is the instruction count, so a timeslice is a fixed
    /// amount of work rather than of time (`docs/design/gems.md`). 0 turns the counting off entirely.
    pub tick_every: u64,
    /// instructions left until the next tick
    pub tick_left: u64,
    /// Whether an instruction tick also moves the clock (`tick`) on. A host with a clock of its
    /// own — a frame loop giving the VM its frame time — turns this off with
    /// [`Vm::task_external_clock`] and calls [`Vm::task_advance_ticks`] instead; the instruction
    /// count then only ends timeslices, which is what keeps one task from eating a whole frame.
    pub clock_from_instructions: bool,
    /// `Task.current` in the root context: the task that stands for the program itself, made on
    /// first use and in no queue (`mrb->task.main_task`)
    pub main: Option<ObjId>,
    /// what the scheduler runs at every entry, before it reads the ready queue
    /// (`mrb_task_set_scheduler_hook`); only the test helpers install one
    pub hook: Option<fn(&mut Vm)>,
    /// the collector is the scheduler's to drive (`GC.scheduler_driven`)
    pub gc_driven: bool,
    /// what `GC.debt_limit` holds, the safety valve of a scheduler that never idles
    pub gc_debt_limit: i64,

    // ---- time: best-effort limits on the host's clock (`docs/design/gems.md`, "Time limits")

    /// How a timeslice ends ([`Vm::task_set_timeslice`]).
    pub timeslice: Timeslice,
    /// The host's monotonic clock in nanoseconds ([`Vm::task_set_clock`]). The VM is `no_std`
    /// and reads no clock of its own.
    pub clock: Option<fn() -> u64>,
    /// when the running task was handed the CPU, on `clock`
    pub slice_start: u64,
    /// the current [`Vm::task_run_limits`]: the clock value past which the running slice is cut
    /// short, and the ones past which a task that cannot be switched out gets `Task::Overrun`
    pub run_soft: Option<u64>,
    pub run_hard: Option<u64>,
    pub run_hard_instructions: Option<u64>,
    /// the tick found the running task past a hard limit
    pub overrun: bool,
    /// whether a tick looks at the limits at all (none are set: it does not)
    pub limits_active: bool,
    /// whether natives count towards a look at the clock between two ticks: set only while a
    /// limit is kept on the clock, since one native can take longer than ten thousand
    /// instructions
    pub native_sampling: bool,
    /// natives between two looks at the clock, and how many are left
    pub native_every: u32,
    pub native_left: u32,
    /// a look at the clock asked for by the native count: the instructions that were left
    /// until the real tick, put back once the clock has been read
    pub forced: Option<u64>,
    /// `Task::Overrun`
    pub overrun_class: Option<ObjId>,
}

/// Instructions a tick lasts where nothing else drives one (`MRB_TICK_UNIT` has no meaning
/// without a clock).
pub const TASK_TICK_INSTRUCTIONS: u64 = 10_000;

/// Natives between two looks at the clock while a limit is kept on it.
pub const TASK_NATIVE_SAMPLE: u32 = 32;

/// How mruby-task's timeslice ends.
///
/// The reference's tick is a timer interrupt, so its timeslice is an amount of time. A `no_std`
/// VM has no timer, and the default here counts instructions instead: a timeslice is a fixed
/// amount of work, the same on every machine, which is what a replay or a lockstep game needs.
/// With a clock from the host ([`Vm::task_set_clock`]) a timeslice can be an amount of time, as
/// the reference's is, which also bounds work the instruction count does not see (a native that
/// takes long). The clock is read when a tick comes due (every [`TASK_TICK_INSTRUCTIONS`]) and
/// after every [`TASK_NATIVE_SAMPLE`] natives, so a time slice ends that much late at worst.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Timeslice {
    /// Three ticks of [`TASK_TICK_INSTRUCTIONS`] instructions (the default; deterministic).
    #[default]
    Instructions,
    /// `nanos` on the host's clock. Without a clock this is `Instructions`.
    Time { nanos: u64 },
    /// Whichever of the two comes first.
    Both { nanos: u64 },
}

/// What one turn of a host loop may spend ([`Vm::task_run_limits`]). Every field is optional.
///
/// The instruction budget is checked between timeslices, as [`Vm::task_run_budget`] always did.
/// The time budget also cuts the running timeslice short at the next look at the clock. Neither
/// can stop a task that is inside a native waiting for Ruby code it called (the `to_s` of
/// `Array#join`, `ObjectSpace.each_object { }`, a block a native called through `funcall`;
/// `docs/design/wait-anywhere.md` lists them): switching it out would need the native's Rust
/// frames to be kept, which they cannot be. The
/// overrun limits are for that case — past them, such a task gets `Task::Overrun` (an
/// `Exception`, not a `StandardError`, so a plain `rescue` does not keep it running), which
/// unwinds the native like any exception, and the task is switched out at the next boundary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunLimits {
    /// instructions over all tasks, checked between timeslices
    pub instructions: Option<u64>,
    /// nanoseconds on the host's clock
    pub time_ns: Option<u64>,
    /// nanoseconds after which a task that cannot be switched out gets `Task::Overrun`
    pub overrun_ns: Option<u64>,
    /// the same counted in instructions, for a host that keeps no clock (deterministic)
    pub overrun_instructions: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
pub struct CallInfo {
    pub base: usize,
    pub pc: usize,
    pub irep: IrepId,
    pub proc_: ObjId,
    /// Number of positional args (15 = packed into an array at R1).
    pub n: u8,
    pub kw: bool,
    pub mid: Option<Sym>,
    pub target_class: ObjId,
    pub env: Option<ObjId>,
    pub cci: Cci,
    /// Visibility given to methods defined by `def` in this frame (`private` with no arguments).
    pub vis: Vis,
    /// `module_function` with no arguments: following `def`s also become singleton methods.
    pub modfunc: bool,
    /// `instance_eval`/`class_eval` frame: visibility lookups stop here.
    pub vis_break: bool,
}

/// Entries in [`Vm::method_cache`]. A power of two, so the index is a mask.
const METHOD_CACHE_LEN: usize = 1024;

/// One line of the method cache: what `(class, mid)` resolved to, and the
/// [`Heap::method_serial`] it was resolved under. Direct-mapped and one deep, like the
/// reference's per-call-site inline cache but keyed globally: a miss is the chain walk that
/// used to happen every time, so a wrong guess costs nothing but the compare.
#[derive(Clone, Copy)]
struct MethodCacheLine {
    serial: u64,
    class: ObjId,
    mid: Sym,
    /// `None` = the lookup found nothing, which is worth caching too (`method_missing`).
    found: Option<(MethodRef, ObjId)>,
}

impl Default for MethodCacheLine {
    // serial 0 is younger than any live heap (`Heap::method_serial` starts at 1), so an
    // untouched line never matches
    fn default() -> Self { MethodCacheLine { serial: 0, class: ObjId(0), mid: Sym(0), found: None } }
}

/// Slots of the guard for the inline index opcodes (mruby `mrb_state::idx_class`), one per
/// (core class, operator) pair that `OP_GETIDX`, `OP_GETIDX0` and `OP_SETIDX` answer in Rust
/// instead of sending. The order is the reference's, and the `[]=` slot of a class is its
/// `[]` slot plus [`IDX_ARY_ASET`].
pub(crate) const IDX_ARY_AREF: usize = 0;
pub(crate) const IDX_HASH_AREF: usize = 1;
pub(crate) const IDX_STR_AREF: usize = 2;
pub(crate) const IDX_ARY_ASET: usize = 3;
pub(crate) const IDX_HASH_ASET: usize = 4;
// only mruby-regexp re-arms this one by name (`ext_regexp::init`)
#[cfg(feature = "regexp")]
pub(crate) const IDX_STR_ASET: usize = 5;
const IDX_SLOTS: usize = 6;

/// Well-known classes and modules.
#[derive(Clone, Copy)]
pub struct Core {
    pub basic_object: ObjId,
    pub object: ObjId,
    pub module: ObjId,
    pub class: ObjId,
    pub kernel: ObjId,
    pub comparable: ObjId,
    pub enumerable: ObjId,
    pub nil_class: ObjId,
    pub true_class: ObjId,
    pub false_class: ObjId,
    pub numeric: ObjId,
    pub integer: ObjId,
    pub float: ObjId,
    pub symbol: ObjId,
    pub string: ObjId,
    pub array: ObjId,
    pub hash: ObjId,
    pub range: ObjId,
    pub proc_: ObjId,
    pub exception: ObjId,
    pub standard_error: ObjId,
    pub runtime_error: ObjId,
    pub argument_error: ObjId,
    pub type_error: ObjId,
    pub name_error: ObjId,
    pub no_method_error: ObjId,
    pub zero_division_error: ObjId,
    pub local_jump_error: ObjId,
    pub index_error: ObjId,
    pub range_error: ObjId,
    pub key_error: ObjId,
    pub not_implemented_error: ObjId,
    /// 15.2.27, which mruby-regexp raises (`E_REGEXP_ERROR`).
    pub regexp_error: ObjId,
    pub stop_iteration: ObjId,
    pub frozen_error: ObjId,
    pub float_domain_error: ObjId,
    pub no_matching_pattern_error: ObjId,
    pub system_stack_error: ObjId,
    pub fiber: ObjId,
    pub fiber_error: ObjId,
    /// mruby-rational's `Rational` and mruby-complex's `Complex`, filled in by their
    /// `init` (the two are recognized by class, and a native asks often).
    pub rational: ObjId,
    pub complex: ObjId,
    /// mruby-regexp's `Regexp` and `MatchData`, filled in by its `init`: `$~` takes only a
    /// MatchData, and a literal pattern is one of these.
    pub regexp: ObjId,
    pub match_data: ObjId,
}

impl Core {
    /// Every class in the set (GC roots).
    pub fn ids(&self) -> [ObjId; 44] {
        [self.basic_object, self.object, self.module, self.class, self.kernel, self.comparable, self.enumerable,
         self.nil_class, self.true_class, self.false_class, self.numeric, self.integer, self.float, self.symbol,
         self.string, self.array, self.hash, self.range, self.proc_, self.exception, self.standard_error,
         self.runtime_error, self.argument_error, self.type_error, self.name_error, self.no_method_error,
         self.zero_division_error, self.local_jump_error, self.index_error, self.range_error, self.key_error,
         self.regexp_error,
         self.not_implemented_error, self.stop_iteration, self.frozen_error, self.float_domain_error,
         self.no_matching_pattern_error, self.system_stack_error, self.fiber, self.fiber_error,
         self.rational, self.complex, self.regexp, self.match_data]
    }
}

/// Frequently used symbols.
#[derive(Clone, Copy)]
pub struct Syms {
    pub initialize: Sym,
    /// The three names `mrb_define_method_raw` makes private whatever the caller asked for
    /// (src/class.c), kept here so that check costs no interning.
    pub initialize_copy: Sym,
    pub respond_to_missing: Sym,
    pub to_s: Sym,
    pub inspect: Sym,
    pub call: Sym,
    pub mesg: Sym,
    pub method_missing: Sym,
    pub eq: Sym,
    pub eqq: Sym,
    pub hash: Sym,
    pub eql: Sym,
    pub each: Sym,
    pub plus: Sym,
    pub minus: Sym,
    pub mul: Sym,
    pub div: Sym,
    pub lt: Sym,
    pub le: Sym,
    pub gt: Sym,
    pub ge: Sym,
    pub aref: Sym,
    pub aset: Sym,
    pub attached: Sym,
    pub default_: Sym,
    /// the instance variable a Hash keeps its default proc in (`builtins/hash.rs`)
    pub default_proc: Sym,
    /// The hidden instance variables of a Rational and a Complex (`ext_rational.rs`,
    /// `ext_complex.rs`), which their natives read on every operation.
    pub num: Sym,
    pub den: Sym,
    pub real: Sym,
    pub imag: Sym,
    /// The instance variables of a Binding (`ext_binding.rs`).
    pub bproc: Sym,
    pub benv: Sym,
    pub brecv: Sym,
    pub bpc: Sym,
    /// `$~`, the one name a match publishes (`mrb_gv_define_virtual`), and the hidden instance
    /// variables of a Regexp and a MatchData (`ext_regexp.rs`).
    pub backref: Option<Sym>,
    pub source: Sym,
    pub rflags: Sym,
    pub mdstr: Sym,
    pub mdre: Sym,
}

pub struct Vm {
    pub heap: Heap,
    pub syms: Interner,
    #[doc(hidden)]
    pub ireps: Vec<VmIrep>,
    #[doc(hidden)]
    pub stack: Vec<Slot>,
    #[doc(hidden)]
    pub ci: Vec<CallInfo>,
    pub globals: HashMap<Sym, Slot>,
    /// The exception being propagated (`mrb->exc`) between `L_RAISE` and `EXCEPT`.
    #[doc(hidden)]
    pub exc: Option<Value>,
    out: Vec<u8>,
    pub core: Core,
    pub s: Syms,
    pub top_self: ObjId,
    /// Instruction budget for [`Vm::step`]; `None` = unlimited.
    step_left: Option<u64>,
    /// The Proc whose body is a single `OP_CALL` (mruby `call_proc`): the
    /// method body of `Proc#call`, so calling a block does not re-enter the VM.
    #[doc(hidden)]
    pub call_proc: ObjId,
    /// The Proc whose body is a single `OP_RETURN R0`: the frame [`Vm::push_return_frame`]
    /// leaves under a call whose value a native drops for one of its own.
    ret_proc: ObjId,
    /// The Proc of a native loop frame (`LOOP_IREP`).
    pub(crate) loop_proc: ObjId,
    pub instructions: u64,
    /// Executions per opcode (index = opcode number), filled only while
    /// [`Vm::set_op_counting`] is on; the test runner reports which opcodes a workload never
    /// reached, and the Playground draws a histogram from it.
    pub op_counts: [u64; crate::opcode::OP_COUNT],
    /// Whether the instruction loop fills `op_counts`. Off by default: the counter is a
    /// read-modify-write per instruction and the tightest loops pay 3 to 7% for it.
    count_ops: bool,
    /// `(class, mid) -> (method, owner)`, thrown away wholesale by `Heap::method_serial`.
    method_cache: alloc::boxed::Box<[MethodCacheLine; METHOD_CACHE_LEN]>,
    /// What `[]` / `[]=` resolved to on the core class when the slot was armed (mruby
    /// `mrb_state::idx_builtin`): the implementation the index opcodes stand in for. Only a
    /// plain native qualifies, as only `MRB_METHOD_FUNC_P` does there.
    idx_builtin: [Option<crate::object::NativeFn>; IDX_SLOTS],
    /// The class each slot may answer for, or `None` while the operator is not the recorded
    /// implementation any more (mruby `mrb_state::idx_class`). The opcode compares the
    /// receiver's class — its singleton class where it has one — against this, so a subclass,
    /// a singleton `[]` and a redefined `Array#[]` are all rejected by the same compare.
    idx_class: [Option<ObjId>; IDX_SLOTS],
    /// The `Heap::method_serial` the two arrays above were computed under. mruby rechecks the
    /// slots from every method-table change; here the one counter that already invalidates the
    /// method cache says when the check is stale, so it is done on the next index opcode.
    idx_serial: u64,
    /// Nesting of native -> VM re-entries (`call_proc_with`); bounded to protect the host stack.
    native_depth: u32,
    /// Objects whose `inspect` is in progress (recursive containers print `[...]`).
    #[doc(hidden)]
    pub inspect_guard: Vec<ObjId>,
    /// Keyword hash of the native call in progress. `Vm::funcall` re-attaches it
    /// as keywords when a native forwards its arguments unchanged (`send`, `new`).
    #[doc(hidden)]
    pub pending_kw: Option<Value>,
    /// Pairs whose `==`/`eql?` is in progress (recursive containers compare equal).
    #[doc(hidden)]
    pub eq_guard: Vec<(ObjId, ObjId)>,
    /// `GC.disable` state: collections are postponed until `GC.enable`.
    pub gc_disabled: bool,
    pending_vis_break: bool,
    /// Native fns that stand for `mrb_notimplement()`: `respond_to?` answers false for them.
    #[doc(hidden)]
    pub notimpl_fns: Vec<crate::object::NativeFn>,
    pub gc_step_limit: i64,
    /// `GC.interval_ratio` (percent): the heap may grow to `live * ratio / 100` between collections.
    pub gc_interval_ratio: i64,
    /// Collect at the first instruction boundary after every allocation (`SABIRUBY_GC_STRESS`).
    #[doc(hidden)]
    pub gc_stress: bool,
    /// Natives running on the host stack (SEND -> native, `funcall` -> native).
    /// Their Rust locals may hold values the collector cannot see, so it does not
    /// run while this is non-zero (see `docs/design/gc.md`, "Contract for native code").
    #[doc(hidden)]
    pub native_active: u32,
    /// Objects the host keeps across calls (`mrb_gc_register`).
    #[doc(hidden)]
    pub gc_registered: Vec<ObjId>,
    /// The name the native being dispatched was called by (`Struct` accessors read it to
    /// find their member); valid at the native's entry only.
    #[doc(hidden)]
    pub native_mid: Option<Sym>,
    /// Objects alive after the last collection.
    pub live_after_gc: usize,
    /// Collections run so far.
    pub gc_count: u64,
    /// Total time in the collector, measured with `gc_clock` when the host sets one.
    pub gc_time_ns: u64,
    /// Monotonic clock in nanoseconds (the library is no_std; the CLI supplies one).
    pub gc_clock: Option<fn() -> u64>,
    /// Wall clock as (seconds, nanoseconds) since the Unix epoch, for `Time.now`
    /// (mruby-time); the epoch when the host sets none.
    pub wall_clock: Option<fn() -> (i64, i64)>,
    /// What `Kernel#sleep` outside a task waits with (mruby-sleep): the library is `no_std` and
    /// has nothing to wait on, so a host that can block lends it one. Without it a `sleep`
    /// outside a task returns at once.
    pub sleep_hook: Option<fn(micros: u64)>,
    /// What the VM asks the host for: compiling an `eval` string, reading a file
    /// (`Vm::set_host`, `src/host.rs`). `None` means `eval` is not available.
    #[doc(hidden)]
    pub host: crate::host::HostBox,
    /// Recording of what the interpreter does, for debuggers (`Vm::set_trace`, `src/inspect.rs`).
    /// `None` (the default) means nothing is recorded.
    pub trace: Option<Vec<crate::inspect::TraceEvent>>,
    /// All contexts (fibers); `contexts[cur]` is the running one (its stack/ci are in `stack`/`ci`).
    #[doc(hidden)]
    pub contexts: Vec<Context>,
    #[doc(hidden)]
    pub cur: usize,
    /// True while a native method called straight from a SEND instruction runs
    /// (mruby: the frame's `cci == CINFO_NONE`); false when called through
    /// `funcall` from other native code. Decides whether a fiber switch can
    /// continue in the current run loop or needs a nested one.
    #[doc(hidden)]
    pub direct_send: bool,
    /// Absolute register the native call in progress writes its result to.
    pub(crate) native_ret_reg: usize,
    /// Set by a fiber switch that must end the innermost run loop (yield or
    /// termination of a fiber resumed by native code): the loop returns this value.
    pub(crate) loop_exit: Option<Value>,
    /// Arity of natives as the reference declares it (`MRB_ARGS_*`), for `Method#arity`.
    #[doc(hidden)]
    pub native_arity: Vec<(crate::object::NativeFn, i64)>,
    /// mruby-task's scheduler (`src/builtins/ext_task.rs`).
    pub task: TaskState,
    /// What the host left with the VM for its own native code to read back
    /// (`Vm::set_host_state`, `Vm::host_state`). The VM never looks inside it.
    #[doc(hidden)]
    pub host_state: Option<alloc::boxed::Box<dyn core::any::Any + Send + Sync>>,
    /// What the host wants told when a Data object is collected (`Vm::set_on_free`).
    #[doc(hidden)]
    pub on_free: Option<alloc::boxed::Box<dyn Fn(u32, u64) + Send + Sync>>,
    /// One `HostStore` per host type, with the Data tag of its objects
    /// (`Vm::install_host_store`, `crate::host_store`). The VM only drops entries from them.
    pub(crate) host_stores: Vec<crate::host_store::HostStoreEntry>,
    /// The next number `Vm::next_data_tag` hands out.
    pub(crate) next_tag: u32,
}

/// Result of [`Vm::step`].
#[derive(Debug, PartialEq)]
pub enum Step {
    /// The budget ran out; call `step` again to continue.
    Paused,
    /// The top-level program finished with this value.
    Finished(Value),
}

impl Vm {
    /// A VM with the core classes and native methods, but without `mrblib`
    /// (so `Array#each`, `Integer#times`, `Enumerable` etc. are missing).
    pub fn new() -> Vm {
        let mut syms = Interner::default();
        let mut heap = Heap::default();
        // Bootstrap: BasicObject < Object < Module < Class, then fix up their classes.
        let mk = |heap: &mut Heap, syms: &mut Interner, name: &str, sup: Option<ObjId>, module: bool| {
            let n = syms.intern_str(name);
            heap.alloc_raw(ObjKind::Class(ClassData {
                name: Some(n),
                superclass: sup,
                is_module: module,
                ..Default::default()
            }))
        };
        let basic_object = mk(&mut heap, &mut syms, "BasicObject", None, false);
        let object = mk(&mut heap, &mut syms, "Object", Some(basic_object), false);
        let module = mk(&mut heap, &mut syms, "Module", Some(object), false);
        let class = mk(&mut heap, &mut syms, "Class", Some(module), false);
        let kernel = mk(&mut heap, &mut syms, "Kernel", None, true);
        let comparable = mk(&mut heap, &mut syms, "Comparable", None, true);
        let enumerable = mk(&mut heap, &mut syms, "Enumerable", None, true);
        let c = |heap: &mut Heap, syms: &mut Interner, name: &str, sup: ObjId| mk(heap, syms, name, Some(sup), false);
        let nil_class = c(&mut heap, &mut syms, "NilClass", object);
        let true_class = c(&mut heap, &mut syms, "TrueClass", object);
        let false_class = c(&mut heap, &mut syms, "FalseClass", object);
        let numeric = c(&mut heap, &mut syms, "Numeric", object);
        let integer = c(&mut heap, &mut syms, "Integer", numeric);
        let float = c(&mut heap, &mut syms, "Float", numeric);
        let symbol = c(&mut heap, &mut syms, "Symbol", object);
        let string = c(&mut heap, &mut syms, "String", object);
        let array = c(&mut heap, &mut syms, "Array", object);
        let hash = c(&mut heap, &mut syms, "Hash", object);
        let range = c(&mut heap, &mut syms, "Range", object);
        let proc_ = c(&mut heap, &mut syms, "Proc", object);
        let exception = c(&mut heap, &mut syms, "Exception", object);
        let standard_error = c(&mut heap, &mut syms, "StandardError", exception);
        let runtime_error = c(&mut heap, &mut syms, "RuntimeError", standard_error);
        let argument_error = c(&mut heap, &mut syms, "ArgumentError", standard_error);
        let type_error = c(&mut heap, &mut syms, "TypeError", standard_error);
        let name_error = c(&mut heap, &mut syms, "NameError", standard_error);
        let no_method_error = c(&mut heap, &mut syms, "NoMethodError", name_error);
        let zero_division_error = c(&mut heap, &mut syms, "ZeroDivisionError", standard_error);
        let local_jump_error = c(&mut heap, &mut syms, "LocalJumpError", standard_error);
        let index_error = c(&mut heap, &mut syms, "IndexError", standard_error);
        let range_error = c(&mut heap, &mut syms, "RangeError", standard_error);
        let key_error = c(&mut heap, &mut syms, "KeyError", index_error);
        // 15.2.27, defined by core beside the rest even though only mruby-regexp raises it
        let regexp_error = c(&mut heap, &mut syms, "RegexpError", standard_error);
        let script_error = c(&mut heap, &mut syms, "ScriptError", exception);
        let _syntax_error = c(&mut heap, &mut syms, "SyntaxError", script_error);
        let not_implemented_error = c(&mut heap, &mut syms, "NotImplementedError", script_error);
        let stop_iteration = c(&mut heap, &mut syms, "StopIteration", index_error);
        let frozen_error = c(&mut heap, &mut syms, "FrozenError", runtime_error);
        let float_domain_error = c(&mut heap, &mut syms, "FloatDomainError", range_error);
        let no_matching_pattern_error = c(&mut heap, &mut syms, "NoMatchingPatternError", standard_error);
        let system_stack_error = c(&mut heap, &mut syms, "SystemStackError", exception);
        // mruby-fiber
        let fiber = c(&mut heap, &mut syms, "Fiber", object);
        let fiber_error = c(&mut heap, &mut syms, "FiberError", standard_error);
        let core = Core {
            basic_object, object, module, class, kernel, comparable, enumerable, nil_class, true_class,
            false_class, numeric, integer, float, symbol, string, array, hash, range, proc_, exception,
            rational: object, complex: object, regexp: object, match_data: object,
            standard_error, runtime_error, argument_error, type_error, name_error, no_method_error,
            zero_division_error, local_jump_error, index_error, range_error, key_error, regexp_error,
            not_implemented_error, stop_iteration, frozen_error, float_domain_error,
            no_matching_pattern_error, system_stack_error, fiber, fiber_error,
        };
        // Every object allocated so far is a class or module: set its class.
        for i in 0..heap.len() {
            let id = ObjId(i as u32);
            let is_mod = heap.class(id).is_module;
            heap.get_mut(id).class = if is_mod { module } else { class };
        }
        for (cls, kind) in [(basic_object, InstanceKind::Object), (string, InstanceKind::String), (array, InstanceKind::Array), (hash, InstanceKind::Hash),
                            (range, InstanceKind::Range), (exception, InstanceKind::Exception), (proc_, InstanceKind::Proc), (fiber, InstanceKind::Fiber),
                            (integer, InstanceKind::NoAlloc), (float, InstanceKind::NoAlloc), (symbol, InstanceKind::NoAlloc), (nil_class, InstanceKind::NoAlloc),
                            (true_class, InstanceKind::NoAlloc), (false_class, InstanceKind::NoAlloc)] {
            heap.class_mut(cls).instance_kind = Some(kind);
        }
        let s = Syms {
            initialize: syms.intern_str("initialize"),
            initialize_copy: syms.intern_str("initialize_copy"),
            respond_to_missing: syms.intern_str("respond_to_missing?"),
            to_s: syms.intern_str("to_s"),
            inspect: syms.intern_str("inspect"),
            call: syms.intern_str("call"),
            mesg: syms.intern_str("mesg"),
            num: syms.intern_str("__num"),
            den: syms.intern_str("__den"),
            real: syms.intern_str("__real"),
            imag: syms.intern_str("__imag"),
            bproc: syms.intern_str("proc"),
            benv: syms.intern_str("env"),
            brecv: syms.intern_str("recv"),
            bpc: syms.intern_str("pc"),
            backref: None,
            source: syms.intern_str("@source"),
            rflags: syms.intern_str("@flags"),
            mdstr: syms.intern_str("@source"),
            mdre: syms.intern_str("@regexp"),
            method_missing: syms.intern_str("method_missing"),
            eq: syms.intern_str("=="),
            eqq: syms.intern_str("==="),
            hash: syms.intern_str("hash"),
            eql: syms.intern_str("eql?"),
            each: syms.intern_str("each"),
            plus: syms.intern_str("+"),
            minus: syms.intern_str("-"),
            mul: syms.intern_str("*"),
            div: syms.intern_str("/"),
            lt: syms.intern_str("<"),
            le: syms.intern_str("<="),
            gt: syms.intern_str(">"),
            ge: syms.intern_str(">="),
            aref: syms.intern_str("[]"),
            aset: syms.intern_str("[]="),
            attached: syms.intern_str("__attached__"),
            default_proc: syms.intern_str("__default_proc"),
            default_: syms.intern_str("default"),
        };
        let top_self = heap.alloc(object, ObjKind::Object);
        let call_irep = VmIrep { nlocals: 1, nregs: 4, iseq: vec![Op::Call as u8], catch: vec![], pool: vec![], syms: vec![], reps: vec![], lv: vec![], lines: vec![], filename: None };
        let call_proc = heap.alloc(core.proc_, ObjKind::Proc(ProcData { irep: 0, upper: None, env: None, target_class: Some(core.proc_), strict: true, scope: true, orphan: false, mid: None }));
        // `OP_RETURN R0`; R1 is where the call made above it answers (`Vm::push_return_frame`)
        let ret_irep = VmIrep { nlocals: 1, nregs: 2, iseq: vec![Op::Return as u8, 0], catch: vec![], pool: vec![], syms: vec![], reps: vec![], lv: vec![], lines: vec![], filename: None };
        let ret_proc = heap.alloc(core.proc_, ObjKind::Proc(ProcData { irep: 1, upper: None, env: None, target_class: Some(object), strict: true, scope: true, orphan: false, mid: None }));
        // `OP_DEBUG`, read as one step of a native loop (`Vm::push_loop_frame`)
        let loop_irep = VmIrep { nlocals: 1, nregs: LOOP_RESULT + 1, iseq: vec![Op::Debug as u8, 0, 0, 0, Op::Return as u8, LOOP_RESULT as u8], catch: vec![], pool: vec![], syms: vec![], reps: vec![], lv: vec![], lines: vec![], filename: None };
        let loop_proc = heap.alloc(core.proc_, ObjKind::Proc(ProcData { irep: LOOP_IREP, upper: None, env: None, target_class: Some(object), strict: true, scope: true, orphan: false, mid: None }));
        let mut vm = Vm {
            heap, syms, ireps: vec![call_irep, ret_irep, loop_irep], ret_proc, loop_proc, stack: Vec::new(), ci: Vec::new(), globals: HashMap::new(),
            exc: None, out: Vec::new(), core, s, top_self, step_left: None, instructions: 0, op_counts: [0; crate::opcode::OP_COUNT], count_ops: false, method_cache: alloc::boxed::Box::new([MethodCacheLine::default(); METHOD_CACHE_LEN]), idx_builtin: [None; IDX_SLOTS], idx_class: [None; IDX_SLOTS], idx_serial: 0, native_depth: 0, inspect_guard: Vec::new(), pending_kw: None, eq_guard: Vec::new(), gc_disabled: false, pending_vis_break: false, notimpl_fns: Vec::new(), gc_step_limit: 0, gc_interval_ratio: 200, gc_stress: false, native_active: 0, gc_registered: Vec::new(), native_mid: None, live_after_gc: 0, gc_count: 0, gc_time_ns: 0, gc_clock: None, wall_clock: None, sleep_hook: None, host: None, trace: None, call_proc,
            contexts: vec![Context::new(FiberState::Running)], cur: ROOT, direct_send: false, native_ret_reg: 0, loop_exit: None, native_arity: Vec::new(),
            task: TaskState { wakeup_tick: u32::MAX, tick_every: TASK_TICK_INSTRUCTIONS, tick_left: TASK_TICK_INSTRUCTIONS, clock_from_instructions: true, native_every: TASK_NATIVE_SAMPLE, native_left: TASK_NATIVE_SAMPLE, ..Default::default() },
            host_state: None, on_free: None, host_stores: Vec::new(), next_tag: 1,
        };
        // Constants for the core classes, Object includes Kernel.
        for i in 0..vm.heap.len() {
            let id = ObjId(i as u32);
            if vm.heap.is_class(id) {
                if let Some(n) = vm.heap.class(id).name {
                    vm.heap.class_mut(object).consts.insert(n, Slot::from(Value::Obj(id)));
                }
            }
        }
        vm.include_module(object, kernel);
        // Every class gets its metaclass up front (mruby `make_metaclass`), so
        // that `Sub.exception` finds the singleton method defined on `Exception`.
        for i in 0..vm.heap.len() {
            let id = ObjId(i as u32);
            if vm.heap.is_class(id) && !vm.heap.class(id).is_module {
                vm.singleton_class(Value::Obj(id)).expect("metaclass");
            }
        }
        crate::builtins::init(&mut vm);
        vm
    }

    /// A VM with `mrblib` (the Ruby part of mruby's core library) loaded.
    pub fn with_mrblib() -> VmResult<Vm> {
        let mut vm = Vm::new();
        vm.load_mrblib()?;
        Ok(vm)
    }

    /// Loads `mrblib` and the Ruby parts of the gems into a VM made by [`Vm::new`]
    /// (so a host can set options such as `gc_stress` first).
    pub fn load_mrblib(&mut self) -> VmResult<()> {
        let vm = self;
        vm.load_and_run(crate::MRBLIB_MRB)?;
        // `Kernel#\`` comes from the core `mrblib/kernel.rb` as a public method; mruby-io's
        // `mrblib/kernel.rb` then writes it again as `module_function def \``, and the
        // reference build this is measured against has that gem, so its instance copy is
        // private. Only that half is taken here. The public `Kernel.\`` the other half would
        // add shadows the instance method whenever `self` *is* Kernel — which is what mruby's
        // own `test/t/syntax.rb` ("External command execution.") does, and it passes there
        // only because mruby-io's body runs the command instead of raising. The body here is
        // the one mrblib gives (NotImplementedError: there is no shell in a no_std VM), so
        // the singleton copy would cost that assertion and buy nothing.
        let krn = vm.core.kernel;
        vm.mark_private(krn, &["`"]);
        // gems with a Ruby part, in the order of the reference gembox
        // (`mrbgems/default.gembox`: the *-ext gems before mruby-enumerator,
        // whose `Enumerable#zip` therefore wins over mruby-enum-ext's)
        for lib in [crate::MRBLIB_SPRINTF_MRB, crate::MRBLIB_COMPAR_EXT_MRB, crate::MRBLIB_ENUM_EXT_MRB, crate::MRBLIB_STRING_EXT_MRB, crate::MRBLIB_NUMERIC_EXT_MRB, crate::MRBLIB_ARRAY_EXT_MRB, crate::MRBLIB_HASH_EXT_MRB, crate::MRBLIB_RANGE_EXT_MRB, crate::MRBLIB_PROC_EXT_MRB, crate::MRBLIB_SYMBOL_EXT_MRB, crate::MRBLIB_OBJECT_EXT_MRB, crate::MRBLIB_SET_MRB, crate::MRBLIB_ENUMERATOR_MRB, crate::MRBLIB_ENUM_LAZY_MRB, crate::MRBLIB_ENUM_CHAIN_MRB, crate::MRBLIB_TOPLEVEL_EXT_MRB, crate::MRBLIB_CATCH_MRB, crate::MRBLIB_STRUCT_MRB, crate::MRBLIB_DATA_MRB, crate::MRBLIB_RATIONAL_MRB, crate::MRBLIB_COMPLEX_MRB, crate::MRBLIB_METHOD_MRB] {
            vm.load_and_run(lib)?;
        }
        // the String methods mrblib writes in Ruby that a native does better (`post_mrblib`);
        // with the feature `regexp` mruby-regexp's own `init` below takes those names instead
        #[cfg(not(feature = "regexp"))]
        crate::builtins::string::post_mrblib(vm);
        // mruby-regexp: its Ruby part opens the two classes its `init` then fills in, so a
        // build without the feature loads neither (the bytecode is not even embedded)
        #[cfg(feature = "regexp")]
        vm.load_and_run(crate::MRBLIB_REGEXP_MRB)?;
        vm.load_and_run(crate::MRBLIB_TASK_MRB)?;
        // mruby-regexp initialises after the core mrblib is loaded, as a gem does: it takes the
        // names of the String methods mrblib defines in Ruby (`sub`, `gsub`, which mix character
        // and byte units there) as well as the ones the natives hold
        #[cfg(feature = "regexp")]
        crate::builtins::ext_regexp::init(vm);
        // `require`/`load` last: it is SabiRuby's own Ruby part and reads the gems' names into
        // `$LOADED_FEATURES` (`docs/plans/eval-require-plan.md` 5)
        vm.load_and_run(crate::MRBLIB_REQUIRE_MRB)?;
        // that list is a literal in the Ruby part, so a build that left a gem out takes its
        // name back off: `require "regexp"` then raises LoadError, which is what a build
        // without the gem linked does
        #[cfg(not(feature = "regexp"))]
        {
            let feats = vm.global_get("$LOADED_FEATURES");
            if let Some(items) = vm.ary_vals(feats) {
                let kept: alloc::vec::Vec<Value> = items.into_iter()
                    .filter(|v| vm.str_bytes(*v) != Some(b"regexp".as_slice()))
                    .collect();
                let a = vm.ary_new(kept);
                vm.global_set("$LOADED_FEATURES", a);
            }
        }
        Ok(())
    }

    /// Runs one ready task of mruby-task's scheduler and comes back (`mrb_task_run_once`), which
    /// is what a host loop wants where `Task.run` would block until every task is done: one call
    /// per frame or per turn of an event loop.
    ///
    /// Answers the task's result where it finished, true where one ran or the clock was moved on
    /// to a sleeper's deadline, and nil where nothing ran and nothing could be made ready.
    ///
    /// **A nil answer is not the same as "nothing ran".** A task whose block is worth nil
    /// finishes with a nil result and answers nil here, exactly as a turn that found the
    /// scheduler empty does; the two cannot be told apart from this value alone. A loop that
    /// drives the scheduler until it is out of work should ask [`Vm::task_pending`], as
    /// [`Vm::task_run_limits`] does, and not read a nil as the end of its turn — a burst of
    /// tasks ending with nil would otherwise cost one turn of the host loop each
    /// (`docs/worklog/2026-09-17-task-end-nil.md`).
    pub fn task_run_once(&mut self) -> VmResult<Value> {
        crate::builtins::ext_task::task_run_once(self)
    }

    /// One turn of a host loop: ready tasks get the CPU, one timeslice each, until `budget`
    /// instructions have been spent or nothing is ready. Answers what it spent. A task that never
    /// yields is preempted at its timeslice, so a frame cannot be lost to one.
    pub fn task_run_budget(&mut self, budget: u64) -> VmResult<u64> {
        self.task_run_limits(RunLimits { instructions: Some(budget), ..Default::default() })
    }

    /// One turn of a host loop under [`RunLimits`]: ready tasks get the CPU until a limit is
    /// reached or nothing is ready. Answers the instructions spent. The time limits need a clock
    /// ([`Vm::task_set_clock`]) and are ignored without one.
    pub fn task_run_limits(&mut self, limits: RunLimits) -> VmResult<u64> {
        let start = self.instructions;
        let now = self.task.clock.map(|c| c());
        self.task.run_soft = now.zip(limits.time_ns).map(|(n, d)| n.saturating_add(d));
        self.task.run_hard = now.zip(limits.overrun_ns).map(|(n, d)| n.saturating_add(d));
        self.task.run_hard_instructions = limits.overrun_instructions.map(|d| start.saturating_add(d));
        self.task.overrun = false;
        self.task.native_left = self.task.native_every.max(1);
        crate::builtins::ext_task::update_limits(self);
        let r = self.task_run_limited(start, limits.instructions);
        self.task.run_soft = None;
        self.task.run_hard = None;
        self.task.run_hard_instructions = None;
        self.task.overrun = false;
        crate::builtins::ext_task::update_limits(self);
        r
    }

    fn task_run_limited(&mut self, start: u64, budget: Option<u64>) -> VmResult<u64> {
        loop {
            let spent = self.instructions - start;
            if budget.is_some_and(|b| spent >= b) { return Ok(spent); }
            if let Some(clock) = self.task.clock.filter(|_| self.task.run_soft.is_some() || self.task.run_hard.is_some()) {
                let now = clock();
                if self.task.run_soft.is_some_and(|soft| now >= soft) { return Ok(spent); }
                if self.task.run_hard.is_some_and(|hard| now >= hard) { return Ok(spent); }
            }
            // a hard limit also ends the run: a task that rescues Task::Overrun comes back ready
            // and would otherwise be handed the CPU again and again
            if self.task.run_hard_instructions.is_some_and(|hard| self.instructions >= hard) { return Ok(spent); }
            // Not the value `task_run_once` answers: a task that ends with a nil result answers
            // nil just as an empty scheduler does, and reading that as "nothing is runnable"
            // cost one turn of the host loop per ending task (`docs/design/gems.md`).
            if matches!(crate::builtins::ext_task::task_step(self)?, crate::builtins::ext_task::Step::Stuck) {
                return Ok(self.instructions - start);
            }
        }
    }

    /// Gives the scheduler the host's monotonic clock, in nanoseconds: what [`Timeslice::Time`]
    /// and the time fields of [`RunLimits`] are measured on. `None` takes it away.
    pub fn task_set_clock(&mut self, clock: Option<fn() -> u64>) {
        self.task.clock = clock;
        crate::builtins::ext_task::update_limits(self);
    }

    /// Natives between two looks at the clock while a limit is kept on it ([`TASK_NATIVE_SAMPLE`]
    /// by default). A native that takes long is noticed after at most this many of them, and
    /// every look costs a clock read: measured on a loop of eight million `push`/`pop` calls,
    /// 32 costs about 1%, 8 about 6%, 1 about a third.
    pub fn task_set_native_sample(&mut self, every: u32) {
        self.task.native_every = every.max(1);
        self.task.native_left = self.task.native_left.min(self.task.native_every);
    }

    /// How timeslices end from now on ([`Timeslice`]).
    pub fn task_set_timeslice(&mut self, timeslice: Timeslice) {
        self.task.timeslice = timeslice;
        crate::builtins::ext_task::update_limits(self);
    }

    /// Makes a task that runs the top level of `irep` (`mrb_create_task`, from a compiled program
    /// rather than from a Ruby block). The task is ready at once; nothing runs until the
    /// scheduler is asked to. The answer is the Task object, which the caller should
    /// [`Vm::gc_register`] while it holds it.
    pub fn task_spawn(&mut self, irep: crate::object::IrepId, priority: u8, name: Option<&str>) -> VmResult<ObjId> {
        crate::builtins::ext_task::task_spawn(self, irep, priority, name)
    }

    /// Moves mruby-task's clock on by `n` ticks and wakes what was sleeping until then
    /// (`mrb_tick`'s second half). For a host that has a clock of its own; see
    /// [`Vm::task_external_clock`].
    pub fn task_advance_ticks(&mut self, n: u32) {
        crate::builtins::ext_task::advance_ticks(self, n);
    }

    /// A queue a host can answer a script through (`Task::Queue`, the gem's own class). The
    /// script waits on it with `pop`, which parks its task until something is pushed — the shape
    /// an asynchronous host operation wants: hand the script the queue, do the work outside, push
    /// the result. Register it with [`Vm::gc_register`] while the host holds it.
    pub fn task_queue_new(&mut self) -> VmResult<ObjId> {
        crate::builtins::ext_task::queue_new(self)
    }

    /// Puts `value` in a queue made by [`Vm::task_queue_new`] and makes whatever was waiting on
    /// it ready to run again.
    pub fn task_queue_push(&mut self, queue: ObjId, value: Value) -> VmResult<()> {
        crate::builtins::ext_task::queue_push(self, queue, value)
    }

    /// How many items are waiting in a queue made by [`Vm::task_queue_new`] — what `Queue#size`
    /// answers. A host that drains a queue from outside the VM asks this rather than sending
    /// `size`, which would put a whole call on the stack to read one Array's length.
    pub fn task_queue_len(&mut self, queue: ObjId) -> VmResult<usize> {
        crate::builtins::ext_task::queue_len(self, queue)
    }

    /// Takes one item out of a queue made by [`Vm::task_queue_new`], or `None` where there is
    /// none. The host's counterpart of `Queue#pop`: a task that pops an empty queue parks until
    /// something is pushed, and a host is not a task and has nothing to park, so this answers
    /// `None` instead of waiting. A closed queue also answers `None`; [`Vm::funcall`] with
    /// `closed?` is the question that tells the two apart.
    pub fn task_queue_try_pop(&mut self, queue: ObjId) -> VmResult<Option<Value>> {
        crate::builtins::ext_task::queue_try_pop(self, queue)
    }

    /// Ticks until the earliest sleeping task is due, for a host that waits on a clock of its
    /// own: `None` where nothing is waiting on a deadline, `Some(0)` where one has passed. A
    /// host that has no work to do until then can wait that long before calling the scheduler
    /// again ([`Vm::task_advance_ticks`] first, so the clock is current when it runs).
    pub fn task_next_wakeup_ticks(&self) -> Option<u32> {
        crate::builtins::ext_task::next_wakeup_ticks(self)
    }

    /// Whether the scheduler still has something that can run: a ready task, or one sleeping
    /// until a deadline. False where every task is done, or waiting for something only another
    /// task could do — the point at which a host loop can stop calling the scheduler.
    pub fn task_pending(&self) -> bool {
        crate::builtins::ext_task::pending(self)
    }

    /// Milliseconds one tick stands for (`MRB_TICK_UNIT`), so a host can turn its frame time into
    /// ticks for [`Vm::task_advance_ticks`].
    pub fn task_tick_unit_ms(&self) -> u32 {
        crate::builtins::ext_task::TICK_UNIT_MS
    }

    /// Says the host moves the clock itself ([`Vm::task_advance_ticks`]); the instruction count
    /// then only ends timeslices.
    pub fn task_external_clock(&mut self, yes: bool) {
        self.task.clock_from_instructions = !yes;
    }

    /// What a task answered, or the exception it did not handle (`Task#value`).
    pub fn task_value(&self, task: ObjId) -> Value {
        crate::builtins::ext_task::task_result(self, task)
    }

    /// Instructions a task has run since it was made. A host can show what each script spends —
    /// the difference between two frames is what it spent on that frame — and see which one is
    /// using its timeslice.
    pub fn task_instructions(&self, task: ObjId) -> u64 {
        crate::builtins::ext_task::task_instructions(self, task)
    }

    /// Where a task stands in its own source: the innermost frame with debug info, as a file
    /// name and a line. Works while it is parked as well as while it runs, so a host can show
    /// the line a script is waiting on. `None` where the program carries no debug info
    /// (compiled without `-g`) or the task is over.
    pub fn task_location(&self, task: ObjId) -> Option<(alloc::string::String, u32)> {
        crate::builtins::ext_task::task_location(self, task)
    }

    /// Every frame of a task with debug info, innermost first, as a file name and a line. Where
    /// [`Vm::task_location`] answers the innermost one, this lets a host find the innermost frame
    /// *in the file it is showing* — a script parked inside a library method stands in the
    /// library, and what its author wants to see is the line of their own that is waiting.
    pub fn task_frames(&self, task: ObjId) -> alloc::vec::Vec<(alloc::string::String, u32)> {
        crate::builtins::ext_task::task_frames(self, task)
    }

    /// Which context a task runs in — the index into [`Snapshot::contexts`](crate::inspect::Snapshot)
    /// (`ContextView::index`), so a host that shows one script's frames and registers picks that
    /// context out of [`Vm::snapshot`] instead of parsing `#<Task n ctx=i>`. `None` for a task
    /// that has no context: not started yet, or over.
    pub fn task_context(&self, task: ObjId) -> Option<usize> {
        crate::builtins::ext_task::task_context(self, task)
    }

    /// Whether a task has run to its end (`Task#status == :DORMANT`).
    pub fn task_finished(&self, task: ObjId) -> bool {
        crate::builtins::ext_task::task_is_dormant(self, task)
    }

    // ------------------------------------------------------------------ host entry points
    //
    // The small reads and writes an embedder needs and had no public way to make. Each one wraps
    // what the VM already uses internally (`Heap::ivar_get`, the globals table, `TaskState::running`,
    // the object's kind) and means exactly the same thing; they are here so that a host does not
    // have to reach into `Vm`'s public fields, which are public for the crate's own use across
    // modules and are not a surface anything outside should depend on.

    /// The task the scheduler is running, for a native that was called from inside one: which
    /// script is asking. `None` where nothing is running under the scheduler — a [`Vm::funcall`]
    /// the host made itself, or before the first [`Vm::task_run_once`].
    ///
    /// A host entry point, and the way into the rest of the `task_*` set above: those take a task
    /// and read it ([`Vm::task_value`], [`Vm::task_instructions`], [`Vm::task_location`]), and a
    /// host that spawned the task has its `ObjId` already — a native called from Ruby does not,
    /// and this is where it gets one.
    pub fn task_running(&self) -> Option<ObjId> {
        self.task.running
    }

    /// An instance variable of an object by name (`@name`), or nil where it has none.
    ///
    /// A host entry point: where a host keeps something of its own on an object the VM owns — on
    /// a task, say, so that a native called from it can find what that task stands for
    /// ([`Vm::task_running`]). It is the same table `@name` reads in Ruby, so a script can see
    /// and change what the host left there; a host that does not want that should pick a name a
    /// script is unlikely to write.
    ///
    /// Takes `&self` rather than interning: a name the VM has never seen cannot be the name of an
    /// instance variable, so there is nothing to look up and nothing to add to the symbol table.
    /// [`Vm::ivar_set`] interns, as it must.
    pub fn ivar_get(&self, obj: ObjId, name: &str) -> Value {
        match self.syms.lookup_str(name) {
            Some(n) => self.heap.ivar_get(obj, n),
            None => Value::Nil,
        }
    }

    /// Puts `v` in an object's instance variable by name, as `@name = v` does in Ruby.
    /// A host entry point; [`Vm::ivar_get`] reads it back.
    ///
    /// The collector reaches instance variables through the object that holds them, so a
    /// [`Value`] left here is safe for as long as `obj` itself is (`docs/design/gc.md`) — which
    /// is the difference between this and a [`Value`] captured in a [`Vm::define_closure`].
    pub fn ivar_set(&mut self, obj: ObjId, name: &str, v: Value) {
        let n = self.intern(name);
        self.heap.ivar_set(obj, n, v);
    }

    /// A global variable by name (`$name`), or nil where nothing set it.
    ///
    /// A host entry point: globals are the one namespace a host and every script share without
    /// arranging anything, which is what a per-frame `$state` wants. `$~` is not here — a match
    /// belongs to the scope that made it (`mrb_gv_define_virtual`), so it is not in this table.
    ///
    /// Takes `&self` for the same reason [`Vm::ivar_get`] does: an un-interned name names nothing.
    pub fn global_get(&self, name: &str) -> Value {
        match self.syms.lookup_str(name) {
            Some(n) => self.globals.get(&n).map(|s| s.get()).unwrap_or(Value::Nil),
            None => Value::Nil,
        }
    }

    /// Sets a global variable by name, as `$name = v` does. A host entry point;
    /// [`Vm::global_get`] reads it back, and every script sees it.
    ///
    /// The collector walks the globals table, so a [`Value`] left here stays alive until
    /// something else is put in its place.
    pub fn global_set(&mut self, name: &str, v: Value) {
        let n = self.intern(name);
        self.globals.insert(n, Slot::from(v));
    }

    /// Whether a value is an exception object — an instance of `Exception` or of a class below
    /// it, which is what [`Vm::task_value`] answers for a task that ended by raising.
    ///
    /// A host entry point, and the reason it is one: the Ruby way to ask is `v.is_a?(Exception)`,
    /// and a host that asked that way would be running Ruby code — a method call, a class walk,
    /// possibly a redefined `is_a?` — to classify a value it is only about to print. This reads
    /// the object's representation instead and cannot fail or re-enter the VM.
    pub fn is_exception(&self, v: Value) -> bool {
        matches!(v.obj().map(|o| &self.heap.get(o).kind), Some(ObjKind::Exception))
    }

    /// Where `require` looks (`$LOAD_PATH`). The host decides: the `sabiruby` command uses the
    /// directory of the program and the working directory, an embedder whatever it serves files
    /// from. Empty by default, which makes every `require` of a plain name a LoadError.
    pub fn set_load_path(&mut self, paths: &[&str]) {
        let items: Vec<Value> = paths.iter().map(|p| self.str_new(p.as_bytes())).collect();
        let ary = self.ary_new(items);
        self.global_set("$LOAD_PATH", ary);
    }

    // ------------------------------------------------------------------ output

    /// Bytes written by `puts`/`p`/`print` since the last [`Vm::take_output`].
    pub fn take_output(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.out)
    }
    pub fn write_out(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }

    // ------------------------------------------------------------------ symbols

    pub fn intern(&mut self, name: &str) -> Sym {
        self.syms.intern_str(name)
    }
    pub fn sym_name(&self, s: Sym) -> String {
        self.syms.name_str(s)
    }

    // ------------------------------------------------------------------ objects

    pub fn str_new(&mut self, bytes: &[u8]) -> Value {
        Value::Obj(self.heap.alloc(self.core.string, ObjKind::String(bytes.to_vec())))
    }
    pub fn str_from(&mut self, s: String) -> Value {
        Value::Obj(self.heap.alloc(self.core.string, ObjKind::String(s.into_bytes())))
    }
    /// `bint_norm`: a wide integer becomes a `Value::Int` as soon as it fits, so the two
    /// representations never hold the same value (`==`, `eql?`, `hash` rely on it).
    pub fn bint_value(&mut self, b: crate::bigint::BigInt) -> Value {
        match b.to_i64() {
            Some(i) => Value::Int(i),
            None => Value::Obj(self.heap.alloc(self.core.integer, ObjKind::BigInt(b))),
        }
    }
    /// `mrb_as_bint`: an Integer (immediate or wide) as a [`BigInt`](crate::bigint::BigInt).
    pub fn as_bigint(&self, v: Value) -> Option<crate::bigint::BigInt> {
        match v {
            Value::Int(i) => Some(crate::bigint::BigInt::from_i64(i)),
            Value::Obj(o) => self.heap.bigint(o).cloned(),
            _ => None,
        }
    }
    /// True for an Integer that does not fit in a `Value::Int`.
    pub fn is_bigint(&self, v: Value) -> bool {
        matches!(v, Value::Obj(o) if matches!(self.heap.get(o).kind, ObjKind::BigInt(_)))
    }
    pub fn ary_new(&mut self, v: Vec<Value>) -> Value {
        Value::Obj(self.heap.alloc(self.core.array, ObjKind::Array(slots_of(&v).into())))
    }
    pub fn hash_new(&mut self) -> Value {
        Value::Obj(self.heap.alloc(self.core.hash, ObjKind::Hash(Default::default())))
    }
    pub fn range_new(&mut self, begin: Value, end: Value, excl: bool) -> Value {
        // not frozen: the reference's Range is immutable through its API only (`(1..2).frozen?` is false)
        let o = self.heap.alloc(self.core.range, ObjKind::Range { begin: Slot::from(begin), end: Slot::from(end), excl });
        Value::Obj(o)
    }
    /// `range_ptr_replace`: both ends must be comparable (`<=>` not nil).
    pub fn check_range_ends(&mut self, x: Value, y: Value) -> VmResult<()> {
        if x.is_nil() || y.is_nil() { return Ok(()); }
        let ok = match (x, y) {
            (Value::Int(_), Value::Int(_)) => true,
            (Value::Int(p), Value::Float(q)) => crate::builtins::numeric::int_float_cmp(p, q).is_some(),
            (Value::Float(p), Value::Int(q)) => crate::builtins::numeric::int_float_cmp(q, p).is_some(),
            (Value::Float(p), Value::Float(q)) => p.partial_cmp(&q).is_some(),
            _ => { let cmp = self.intern("<=>"); !self.funcall(x, cmp, &[y], Value::Nil)?.is_nil() }
        };
        if ok { Ok(()) } else { Err(self.raise_arg("bad value for range")) }
    }
    pub fn exc_new(&mut self, class: ObjId, msg: &str) -> Value {
        let e = self.heap.alloc(class, ObjKind::Exception);
        let m = self.str_new(msg.as_bytes());
        let mesg = self.s.mesg;
        self.heap.ivar_set(e, mesg, m);
        Value::Obj(e)
    }
    /// Builds an error to return with `?`: `Err(vm.raise(class, "msg"))`.
    pub fn raise(&mut self, class: ObjId, msg: &str) -> VmError {
        VmError::Raise(self.exc_new(class, msg))
    }
    /// A `NameError` carrying `@name`.
    pub fn name_error(&mut self, name: Sym, msg: &str) -> VmError {
        let e = self.exc_new(self.core.name_error, msg);
        if let Value::Obj(o) = e { let k = self.intern("@name"); self.heap.ivar_set(o, k, Value::Sym(name)); }
        VmError::Raise(e)
    }
    /// A `NoMethodError` carrying `@name` (NameError#name) and an empty `@args`.
    pub fn no_method_error(&mut self, mid: Sym, _recv: Value, msg: &str) -> VmError {
        let e = self.exc_new(self.core.no_method_error, msg);
        if let Value::Obj(o) = e {
            let k = self.intern("@name"); self.heap.ivar_set(o, k, Value::Sym(mid));
            let a = self.intern("@args"); let empty = self.ary_new(vec![]); self.heap.ivar_set(o, a, empty);
        }
        VmError::Raise(e)
    }
    pub fn raise_type(&mut self, msg: &str) -> VmError {
        self.raise(self.core.type_error, msg)
    }
    pub fn raise_arg(&mut self, msg: &str) -> VmError {
        self.raise(self.core.argument_error, msg)
    }
    pub fn argnum_error(&mut self, given: usize, expected: &str) -> VmError {
        self.raise(self.core.argument_error, &format!("wrong number of arguments (given {given}, expected {expected})"))
    }
    /// Whether the string is byte-read (`String#b`, `RSTR_BINARY_P`). A non-string answers no.
    pub fn str_binary(&self, v: Value) -> bool {
        v.obj().map(|o| self.heap.get(o).binary && matches!(self.heap.get(o).kind, ObjKind::String(_))).unwrap_or(false)
    }
    /// Marks the string byte-read (`RSTR_ENCODING_SET`).
    pub fn str_set_binary(&mut self, v: Value, binary: bool) {
        if let Some(o) = v.obj() { self.heap.get_mut(o).binary = binary; }
    }
    /// A new String read the way `like` is (`RSTR_ENC_COPY`).
    pub fn str_new_like(&mut self, bytes: &[u8], like: Value) -> Value {
        let binary = self.str_binary(like);
        let v = self.str_new(bytes);
        self.str_set_binary(v, binary);
        v
    }
    pub fn str_bytes(&self, v: Value) -> Option<&[u8]> {
        v.obj().and_then(|o| self.heap.string(o))
    }
    pub fn ary(&self, v: Value) -> Option<&[Slot]> {
        v.obj().and_then(|o| self.heap.array(o))
    }
    /// The elements of an Array as values (a copy).
    pub fn ary_vals(&self, v: Value) -> Option<Vec<Value>> {
        self.ary(v).map(|a| values_of(a))
    }
    /// The entries of a Hash in insertion order, as values (a copy); `None` when `v` is not a
    /// Hash. The host's counterpart of [`Vm::ary_vals`]: reading a Hash out is otherwise only
    /// possible key by key ([`Vm::hash_get`]), which needs the keys first.
    pub fn hash_entries(&self, v: Value) -> Option<Vec<(Value, Value)>> {
        let o = v.obj()?;
        match &self.heap.get(o).kind {
            ObjKind::Hash(hd) => Some(hd.entries().iter().map(|(k, val)| (k.get(), val.get())).collect()),
            _ => None,
        }
    }
    /// The keys of a Hash in insertion order (a copy); `None` when `v` is not a Hash. What
    /// `Hash#keys` answers, without a send — and, unlike the send, without building the Array
    /// on the Ruby heap for the host to read once and drop. The values are
    /// [`Vm::hash_entries`]; this is for a host that wants the keys and then looks up the ones
    /// it cares about with [`Vm::hash_get`].
    pub fn hash_keys(&self, v: Value) -> Option<Vec<Value>> {
        let o = v.obj()?;
        match &self.heap.get(o).kind {
            ObjKind::Hash(hd) => Some(hd.entries().iter().map(|(k, _)| k.get()).collect()),
            _ => None,
        }
    }
    pub fn expect_str(&mut self, v: Value, what: &str) -> VmResult<Vec<u8>> {
        match self.str_bytes(v) {
            Some(b) => Ok(b.to_vec()),
            None => Err(self.raise_type(&format!("{what} cannot be converted to String"))),
        }
    }
    pub fn expect_int(&mut self, v: Value, _what: &str) -> VmResult<i64> {
        match v {
            Value::Int(i) => Ok(i),
            // `mrb_ensure_int_type`: a Rational truncates, a Complex converts when its
            // imaginary part is zero
            Value::Obj(_) if crate::builtins::ext_rational::is_rational(self, v) || crate::builtins::ext_complex::is_complex(self, v) => {
                let to_i = self.intern("to_i");
                let r = self.funcall(v, to_i, &[], Value::Nil)?;
                self.expect_int(r, _what)
            }
            // `mrb_bint_as_int`: an Integer too wide for the operation that asked for it
            Value::Obj(o) if self.heap.bigint(o).is_some() => {
                match self.heap.bigint(o).unwrap().to_i64() {
                    Some(i) => Ok(i),
                    None => Err(self.raise(self.core.range_error, "integer out of range")),
                }
            }
            Value::Float(f) => {
                if f.is_nan() || f.is_infinite() { let s = crate::builtins::numeric::float_to_s(f); return Err(self.raise(self.core.float_domain_error, &s)); }
                if f >= 9223372036854775808.0 || f < -9223372036854775808.0 { let s = crate::builtins::numeric::float_to_s(f); return Err(self.raise(self.core.range_error, &format!("float {s} out of range of integer"))); }
                Ok(f as i64)
            }
            _ => { let d = self.describe_for_type_error(v); Err(self.raise_type(&format!("no implicit conversion of {d} into Integer"))) }
        }
    }

    // ------------------------------------------------------------------ classes

    pub fn class_of(&self, v: Value) -> ObjId {
        match v {
            Value::Nil => self.core.nil_class,
            Value::False => self.core.false_class,
            Value::True => self.core.true_class,
            Value::Int(_) => self.core.integer,
            Value::Float(_) => self.core.float,
            Value::Sym(_) => self.core.symbol,
            Value::Obj(o) => self.heap.get(o).class,
        }
    }
    /// The class as seen by Ruby (skipping singleton and include classes).
    pub fn real_class_of(&self, v: Value) -> ObjId {
        let mut c = self.class_of(v);
        loop {
            let cd = self.heap.class(c);
            if cd.is_singleton || cd.iclass_of.is_some() || cd.origin_of.is_some() {
                c = cd.superclass.expect("singleton without superclass");
            } else {
                return c;
            }
        }
    }
    pub fn class_name(&self, c: ObjId) -> String {
        let cd = self.heap.class(c);
        if let Some(m) = cd.iclass_of {
            return self.class_name(m);
        }
        if let Some(o) = cd.origin_of {
            return self.class_name(o);
        }
        match cd.name {
            Some(n) => match cd.outer {
                Some(o) if o != self.core.object => format!("{}::{}", self.class_name(o), self.syms.name_str(n)),
                _ => self.syms.name_str(n),
            },
            None => {
                let kind = if cd.is_module { "Module" } else { "Class" };
                format!("#<{kind}:0x{:012x}>", (c.0 as usize + 1) * 0x40)
            }
        }
    }
    pub fn define_class(&mut self, name: &str, superclass: ObjId) -> ObjId {
        let n = self.intern(name);
        if let Some(Value::Obj(c)) = self.heap.class(self.core.object).consts.get(&n).map(|s| s.get()) {
            return c;
        }
        let c = self.heap.alloc(self.core.class, ObjKind::Class(ClassData { name: Some(n), superclass: Some(superclass), ..Default::default() }));
        self.heap.class_mut(self.core.object).consts.insert(n, Slot::from(Value::Obj(c)));
        self.singleton_class(Value::Obj(c)).expect("metaclass");
        c
    }
    /// A class nested in a module or class (`Outer::Name`), as `mrb_define_class_under` is:
    /// the constant goes in `outer` rather than in `Object`, and `Class#name` answers with the
    /// qualified name. An existing constant of that name is answered with, as
    /// [`Vm::define_class`] does.
    pub fn define_class_under(&mut self, outer: ObjId, name: &str, superclass: ObjId) -> ObjId {
        if outer == self.core.object { return self.define_class(name, superclass); }
        let n = self.intern(name);
        if let Some(Value::Obj(c)) = self.heap.class(outer).consts.get(&n).map(|s| s.get()) {
            return c;
        }
        let c = self.heap.alloc(self.core.class, ObjKind::Class(ClassData {
            name: Some(n), superclass: Some(superclass), outer: Some(outer), ..Default::default()
        }));
        self.heap.class_mut(outer).consts.insert(n, Slot::from(Value::Obj(c)));
        self.singleton_class(Value::Obj(c)).expect("metaclass");
        c
    }
    pub fn define_module(&mut self, name: &str) -> ObjId {
        let n = self.intern(name);
        if let Some(Value::Obj(c)) = self.heap.class(self.core.object).consts.get(&n).map(|s| s.get()) {
            return c;
        }
        let c = self.heap.alloc(self.core.module, ObjKind::Class(ClassData { name: Some(n), is_module: true, ..Default::default() }));
        self.heap.class_mut(self.core.object).consts.insert(n, Slot::from(Value::Obj(c)));
        c
    }
    pub fn define_method(&mut self, class: ObjId, name: &str, f: crate::object::NativeFn) {
        let n = self.intern(name);
        self.def_method_raw(class, n, Method::Native(f));
    }
    /// Defines a native method from a closure, which — unlike [`Vm::define_method`]'s bare
    /// function pointer — can carry an environment of its own.
    ///
    /// The closure must be `Send + Sync + 'static`, as a [`Vm`] is; share mutable state
    /// through an `Arc<Mutex<_>>` of the host's choosing, or put it in the VM with
    /// [`Vm::set_host_state`] and read it back from the closure through the `&mut Vm` it is
    /// given. Everything else about the method is as `define_method`: it is dispatched by
    /// SEND and by [`Vm::funcall`], `respond_to?` and `method_defined?` see it, `Method#owner`
    /// and `#arity` answer for it (arity `-1`, as for any native with no declared argument
    /// spec), and an `Err` it returns raises in the Ruby frame that called it.
    ///
    /// The collector does not look inside the closure: a [`Value`] captured in it is not a root,
    /// so keep one across calls only through [`Vm::gc_register`] (`docs/design/gc.md`). Handles to the
    /// host's own data have no such problem.
    ///
    /// ```
    /// # fn main() -> Result<(), sabiruby::VmError> {
    /// use std::sync::{Arc, Mutex};
    /// let log = Arc::new(Mutex::new(Vec::<i64>::new()));
    /// let mut vm = sabiruby::Vm::with_mrblib()?;
    /// let sink = log.clone();
    /// let object = vm.core.object;
    /// vm.define_closure(object, "record", move |_vm, _self_, args, _blk| {
    ///     if let Some(sabiruby::Value::Int(i)) = args.first() { sink.lock().unwrap().push(*i); }
    ///     Ok(sabiruby::Value::Nil)
    /// });
    /// # Ok(()) }
    /// ```
    pub fn define_closure<F>(&mut self, class: ObjId, name: &str, f: F)
    where
        F: Fn(&mut Vm, Value, &[Value], Value) -> VmResult<Value> + Send + Sync + 'static,
    {
        self.define_closure_body(class, name, -1, alloc::boxed::Box::new(f));
    }

    /// [`Vm::define_closure`] with the arity the method reports, for a caller that knows how
    /// many arguments the body reads ([`Vm::define_fn`], which builds it from a Rust signature).
    pub(crate) fn define_closure_body(&mut self, class: ObjId, name: &str, arity: i64, f: alloc::boxed::Box<dyn Fn(&mut Vm, Value, &[Value], Value) -> VmResult<Value> + Send + Sync>) {
        let n = self.intern(name);
        self.def_method_raw(class, n, Method::Closure(alloc::sync::Arc::new(crate::object::ClosureBody { f, arity })));
    }

    // ------------------------------------------------------------------ host state

    /// Puts a value of the host's own in the VM, to be read back from native code.
    ///
    /// This is the other direction from the [`Host`](crate::Host) trait: `Host` is what the
    /// VM asks of its host (compile a string, read a file), this is what the host leaves with
    /// the VM. One value is kept; setting it again replaces what was there.
    ///
    /// ```
    /// # fn main() -> Result<(), sabiruby::VmError> {
    /// struct Counters { frames: u64 }
    /// let mut vm = sabiruby::Vm::with_mrblib()?;
    /// vm.set_host_state(Counters { frames: 0 });
    /// let object = vm.core.object;
    /// vm.define_closure(object, "tick", |vm, _self_, _args, _blk| {
    ///     let n = match vm.host_state_mut::<Counters>() { Some(c) => { c.frames += 1; c.frames } None => 0 };
    ///     Ok(sabiruby::Value::Int(n as i64))
    /// });
    /// # Ok(()) }
    /// ```
    pub fn set_host_state<T: core::any::Any + Send + Sync + 'static>(&mut self, state: T) {
        self.host_state = Some(alloc::boxed::Box::new(state));
    }
    /// The value [`Vm::set_host_state`] left, when it is a `T`.
    pub fn host_state<T: core::any::Any + Send + Sync + 'static>(&self) -> Option<&T> {
        self.host_state.as_ref().and_then(|b| b.downcast_ref::<T>())
    }
    /// The value [`Vm::set_host_state`] left, when it is a `T`, to change in place.
    pub fn host_state_mut<T: core::any::Any + Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.host_state.as_mut().and_then(|b| b.downcast_mut::<T>())
    }
    /// Takes the value [`Vm::set_host_state`] left out of the VM.
    pub fn take_host_state(&mut self) -> Option<alloc::boxed::Box<dyn core::any::Any + Send + Sync>> {
        self.host_state.take()
    }

    // ------------------------------------------------------------------ data objects

    /// An object of `class` that stands for a value the host owns: `tag` says what kind of
    /// thing it is and `handle` which one (the index in a slab of the host's, say). This is
    /// mruby's `RData` without the pointer — the VM carries the two numbers and never reads
    /// through them, so no raw pointer is involved and a stale handle cannot corrupt anything
    /// (the worst it can do is name the wrong thing in the host's own table).
    ///
    /// The object is an ordinary Ruby object otherwise: its class, its instance variables,
    /// `freeze`, `respond_to?` and the rest work as on any other. What is special is that
    /// `==`, `eql?` and `hash` go by `(tag, handle)` rather than by identity (so two objects
    /// naming the same host value are the same key in a Hash, while `equal?` still asks
    /// whether they are the same object), that `dup` and `clone` refuse (a handle must not be
    /// copied behind the host's back: see [`Vm::set_on_free`]), and that the host is told when
    /// it is collected.
    ///
    /// ```
    /// # fn main() -> Result<(), sabiruby::VmError> {
    /// let mut vm = sabiruby::Vm::with_mrblib()?;
    /// let player = vm.define_class("Player", vm.core.object);
    /// let v = vm.data_new(player, 1, 42);
    /// assert_eq!(vm.data_of(v), Some((1, 42)));
    /// # Ok(()) }
    /// ```
    pub fn data_new(&mut self, class: ObjId, tag: u32, handle: u64) -> Value {
        Value::Obj(self.heap.alloc(class, ObjKind::Data { tag, handle }))
    }
    /// The `(tag, handle)` of a [`Vm::data_new`] object; `None` for anything else.
    pub fn data_of(&self, v: Value) -> Option<(u32, u64)> {
        match v.obj().map(|o| &self.heap.get(o).kind) {
            Some(ObjKind::Data { tag, handle }) => Some((*tag, *handle)),
            _ => None,
        }
    }
    /// Sets what to run when a [`Vm::data_new`] object is collected, so the host can drop the
    /// value the handle named. One hook is kept for the whole VM; setting it again replaces it.
    ///
    /// It is called once per collected object, after the sweep, with the `(tag, handle)` the
    /// object carried. It is **not** given the `&mut Vm`, and that is the point: it runs while
    /// the collector is finishing, so there is no VM state a hook could safely read, let alone
    /// change. A hook that needs to reach the host's own data captures it (an `Arc<Mutex<_>>`
    /// of a slab, say); a hook that wants to run Ruby code queues the work for the next call
    /// the host makes into the VM.
    ///
    /// The hook fires for what the collector reclaims, so a handle held by a live object, or
    /// by one the VM is dropped with, never reaches it: a host that keeps its own table clears
    /// what is left when it puts the VM away.
    ///
    /// ```
    /// # fn main() -> Result<(), sabiruby::VmError> {
    /// use std::sync::{Arc, Mutex};
    /// let freed = Arc::new(Mutex::new(Vec::<u64>::new()));
    /// let sink = freed.clone();
    /// let mut vm = sabiruby::Vm::with_mrblib()?;
    /// vm.set_on_free(Box::new(move |_tag, handle| sink.lock().unwrap().push(handle)));
    /// # Ok(()) }
    /// ```
    pub fn set_on_free(&mut self, hook: alloc::boxed::Box<dyn Fn(u32, u64) + Send + Sync>) {
        self.on_free = Some(hook);
    }
    pub fn alias_method(&mut self, class: ObjId, new: Sym, old: Sym) -> VmResult<()> {
        match self.find_method(class, old) {
            Some((m, owner)) => {
                let vis = self.method_vis(owner, old);
                // `mrb_alias_method`: a Ruby method is aliased through a proc of its own that
                // carries the original name (`MRB_PROC_ALIAS`, `body.mid`), so `__method__` and
                // `super` in the method see the name it was defined under
                let m = match m {
                    Method::Ruby(p) => {
                        let mut pd = self.heap.proc_data(p).clone();
                        pd.mid = Some(pd.mid.unwrap_or(old));
                        let cls = self.heap.get(p).class;
                        Method::Ruby(self.heap.alloc(cls, ObjKind::Proc(pd)))
                    }
                    m => m,
                };
                self.def_method(class, new, m, vis)
            }
            None => { let n = self.sym_name(old); let cn = self.class_name(class); Err(self.name_error(old, &format!("undefined method '{n}' for class '{cn}'"))) }
        }
    }
    pub fn undef_method(&mut self, class: ObjId, mid: Sym) -> VmResult<()> {
        if self.heap.get(class).frozen { return Err(self.frozen_error(Value::Obj(class))); }
        if self.find_method(class, mid).is_none() { let n = self.sym_name(mid); let cn = self.class_name(class); return Err(self.name_error(mid, &format!("undefined method '{n}' for class '{cn}'"))); }
        let t = self.def_target(class);
        self.heap.class_mut(t).methods.insert(mid, Method::Undef);
        self.method_undefined_hook(class, mid)
    }
    /// The table that receives definitions for `class` (its origin when modules are prepended).
    pub fn def_target(&self, class: ObjId) -> ObjId {
        self.heap.class(class).origin.unwrap_or(class)
    }
    /// The three names that are private however they are *defined* (`mrb_define_method_raw`,
    /// src/class.c: the test comes before the one for the caller's own visibility, so it wins
    /// over `MRB_METHOD_PUBLIC_FL` and over the "singleton methods are always public" rule —
    /// `def obj.initialize` is private in the reference too).
    ///
    /// It does **not** reach the built-in method tables: mruby installs those as ROM layers
    /// (`mrb_mt_init_rom`, src/class.c), which write their entries into the table directly and
    /// never pass through `mrb_define_method_raw`. That is why every ROM table that wants a
    /// private `initialize` spells `MRB_MT_PRIVATE` out itself — and why `Struct#initialize`
    /// and `Random#initialize`, whose entries do not, are *public* in the reference. The
    /// natives here are those tables, so they say which of their entries are private
    /// ([`Vm::define_private_methods`]) instead of being caught by this rule.
    fn always_private(&self, mid: Sym) -> bool {
        mid == self.s.initialize || mid == self.s.initialize_copy || mid == self.s.respond_to_missing
    }
    /// Installs a method without hooks (used by initialization and internal copies).
    pub fn def_method_raw(&mut self, class: ObjId, mid: Sym, m: Method) {
        let t = self.def_target(class);
        self.heap.class_mut(t).methods.insert(mid, m);
        self.heap.class_mut(t).vis.remove(&mid);
    }
    /// Installs a method with visibility and calls the `method_added` hook.
    pub fn def_method(&mut self, class: ObjId, mid: Sym, m: Method, vis: Vis) -> VmResult<()> {
        if self.heap.get(class).frozen { return Err(self.frozen_error(Value::Obj(class))); }
        let t = self.def_target(class);
        self.heap.class_mut(t).methods.insert(mid, m);
        // initialize & co. are always private (class.c)
        let vis = if self.always_private(mid) { Vis::Private } else { vis };
        if vis == Vis::Public { self.heap.class_mut(t).vis.remove(&mid); } else { self.heap.class_mut(t).vis.insert(mid, vis); }
        self.method_added(class, mid)
    }
    /// Where the default visibility for `def` lives (class.c `find_visibility_scope`):
    /// the nearest scope frame or, once that frame has an environment, its env.
    /// `c` = the class being defined into (`None` = the frame's target class).
    pub fn visibility_scope(&self, c: Option<ObjId>) -> (Option<usize>, Option<ObjId>) {
        let top = self.ci.len() - 1;
        let ci = self.ci[top];
        let c = c.unwrap_or(ci.target_class);
        let brk_frame = |p: ObjId| -> bool {
            let pd = self.heap.proc_data(p);
            pd.upper.is_none() || pd.scope || pd.env.is_none() || ci.target_class != c || ci.vis_break
        };
        if brk_frame(ci.proc_) {
            return (Some(top), ci.env);
        }
        let mut p = ci.proc_;
        loop {
            let env = self.heap.proc_data(p).env;
            let up = self.heap.proc_data(p).upper;
            let stop = match up {
                None => true,
                Some(u) => { let ud = self.heap.proc_data(u); ud.upper.is_none() || ud.scope || ud.env.is_none() || match env { Some(e) => { let ed = self.heap.env(e); ed.target_class != Some(c) || ed.vis_break } None => true } }
            };
            if stop { return (None, env); }
            p = up.unwrap();
        }
    }
    /// The (visibility, module_function) that a `def` in the current frame gets.
    pub fn current_def_vis(&self, c: ObjId) -> (Vis, bool) {
        match self.visibility_scope(Some(c)) {
            (_, Some(e)) => { let ed = self.heap.env(e); (ed.vis, ed.modfunc) }
            (Some(i), None) => (self.ci[i].vis, self.ci[i].modfunc),
            _ => (Vis::Public, false),
        }
    }
    /// `private` etc. with no arguments: records the default on the scope.
    pub fn set_scope_vis(&mut self, vis: Vis, modfunc: bool) {
        match self.visibility_scope(None) {
            (_, Some(e)) => { let ed = self.heap.env_mut(e); ed.vis = vis; ed.modfunc = modfunc; }
            (Some(i), None) => { self.ci[i].vis = vis; self.ci[i].modfunc = modfunc; }
            _ => {}
        }
    }
    /// The hook a change to a method table fires. mruby has the same shape in three places
    /// (`mrb_method_added`, `undef_method` and `remove_method_id`, src/class.c): the name goes
    /// to the class itself, or — when the table belongs to a singleton class — to the object
    /// that class is attached to, under the `singleton_` name. The default definitions on
    /// `Module` and `BasicObject` do nothing, and finding one of those is what
    /// `mrb_func_basic_p` answers there: the call is skipped rather than made.
    fn method_table_hook(&mut self, class: ObjId, mid: Sym, plain: &str, singleton: &str) -> VmResult<()> {
        let cd = self.heap.class(class);
        let (recv, hook) = if cd.is_singleton { (cd.attached.map(|s| s.get()).unwrap_or(Value::Nil), self.intern(singleton)) } else { (Value::Obj(class), self.intern(plain)) };
        if let Some((Method::Native(_) | Method::Closure(_), owner)) = self.find_method(self.class_of(recv), hook) {
            if owner == self.core.module || owner == self.core.basic_object { return Ok(()); }
        }
        if self.respond_to(recv, hook) { self.funcall(recv, hook, &[Value::Sym(mid)], Value::Nil)?; }
        Ok(())
    }
    /// `mrb_method_added`: `method_added` / `singleton_method_added` hook.
    pub fn method_added(&mut self, class: ObjId, mid: Sym) -> VmResult<()> {
        self.method_table_hook(class, mid, "method_added", "singleton_method_added")
    }
    /// `undef_method`'s hook: `method_undefined` / `singleton_method_undefined`.
    pub fn method_undefined_hook(&mut self, class: ObjId, mid: Sym) -> VmResult<()> {
        self.method_table_hook(class, mid, "method_undefined", "singleton_method_undefined")
    }
    /// `remove_method_id`'s hook: `method_removed` / `singleton_method_removed`.
    pub fn method_removed_hook(&mut self, class: ObjId, mid: Sym) -> VmResult<()> {
        self.method_table_hook(class, mid, "method_removed", "singleton_method_removed")
    }
    /// `const_added` (`mrb_const_set`, src/variable.c): the module is told the name of a
    /// constant that was just assigned to it. mruby fires it from `mrb_const_set` alone, which
    /// is `OP_SETCONST`, `OP_SETMCNST` and `Module#const_set` — **not** `class Foo; end`, which
    /// goes through `setup_class` and writes the constant without the hook.
    pub fn const_added(&mut self, module: ObjId, name: Sym) -> VmResult<()> {
        let hook = self.intern("const_added");
        let recv = Value::Obj(module);
        if let Some((Method::Native(_) | Method::Closure(_), owner)) = self.find_method(self.class_of(recv), hook) {
            if owner == self.core.module { return Ok(()); }
        }
        if self.respond_to(recv, hook) { self.funcall(recv, hook, &[Value::Sym(name)], Value::Nil)?; }
        Ok(())
    }
    /// The class whose `methods`/`vis` tables a chain node reads: an include class
    /// reads its module's table (the module's origin when it has prepends).
    pub fn table_owner(&self, x: ObjId) -> ObjId {
        match self.heap.class(x).iclass_of { Some(m) => self.heap.class(m).origin.unwrap_or(m), None => x }
    }
    /// Visibility of `mid` as found from `class`.
    pub fn method_vis(&self, class: ObjId, mid: Sym) -> Vis {
        let mut c = Some(class);
        while let Some(x) = c {
            let cd = self.heap.class(x);
            let tbl_owner = self.table_owner(x);
            let t = self.heap.class(tbl_owner);
            if cd.origin.is_none() {
                if t.methods.contains_key(&mid) { return t.vis.get(&mid).copied().unwrap_or(Vis::Public); }
            }
            c = cd.superclass;
        }
        Vis::Public
    }
    /// `private :m` etc.: sets visibility in `class`, copying an inherited method first.
    pub fn set_visibility(&mut self, class: ObjId, mid: Sym, vis: Vis) -> VmResult<()> {
        if self.heap.get(class).frozen { return Err(self.frozen_error(Value::Obj(class))); }
        let t = self.def_target(class);
        if !self.heap.class(t).methods.contains_key(&mid) {
            match self.find_method(class, mid) {
                Some((m, _)) => { self.heap.class_mut(t).methods.insert(mid, m); }
                None => { let n = self.sym_name(mid); let cn = self.class_name(class); return Err(self.raise(self.core.name_error, &format!("undefined method '{n}' for class '{cn}'"))); }
            }
        }
        if vis == Vis::Public { self.heap.class_mut(t).vis.remove(&mid); } else { self.heap.class_mut(t).vis.insert(mid, vis); }
        Ok(())
    }
    /// `Module#prepend`: moves the class's own table into an origin include class once, then
    /// inserts the module's include class right below the class.
    pub fn prepend_module(&mut self, class: ObjId, module: ObjId) -> VmResult<()> {
        if self.heap.get(class).frozen { return Err(self.frozen_error(Value::Obj(class))); }
        if self.heap.class(class).origin.is_none() {
            let (methods, vis, sup) = { let c = self.heap.class_mut(class); (core::mem::take(&mut c.methods), core::mem::take(&mut c.vis), c.superclass) };
            let origin = self.heap.alloc(self.core.class, ObjKind::Class(ClassData { superclass: sup, origin_of: Some(class), methods, vis, ..Default::default() }));
            let c = self.heap.class_mut(class);
            c.superclass = Some(origin);
            c.origin = Some(origin);
        }
        // already prepended (between class and origin)?
        let origin = self.heap.class(class).origin.unwrap();
        let mut c = self.heap.class(class).superclass;
        while let Some(x) = c {
            if x == origin { break; }
            if self.heap.class(x).iclass_of == Some(module) { return Ok(()); }
            c = self.heap.class(x).superclass;
        }
        if module == class || self.module_chain_contains(module, class) { return Err(self.raise_arg("cyclic prepend detected")); }
        // the module's own chain goes in too, nearest first: insert in reverse right below the class
        // (a module already anywhere in the chain, prepended or included, is not added again)
        let mods = self.module_chain(module);
        for m in mods.iter().rev() {
            // mruby include_module_at(search_super=0): scan stops at the first real class,
            // so a module already prepended to a superclass is prepended again here
            let mut c = self.heap.class(class).superclass;
            let mut present = false;
            while let Some(x) = c {
                let cd = self.heap.class(x);
                if cd.iclass_of == Some(*m) { present = true; break; }
                if cd.iclass_of.is_none() && cd.origin_of.is_none() { break; }
                c = cd.superclass;
            }
            if present { continue; }
            let sup = self.heap.class(class).superclass;
            let ic = self.heap.alloc(self.core.class, ObjKind::Class(ClassData { superclass: sup, iclass_of: Some(*m), ..Default::default() }));
            self.heap.class_mut(class).superclass = Some(ic);
        }
        Ok(())
    }
    /// Defines the same native method on several classes.
    pub fn define_methods(&mut self, class: ObjId, list: &[(&str, crate::object::NativeFn)]) {
        for (n, f) in list {
            self.define_method(class, n, *f);
        }
    }
    /// `mrb_define_private_method`: a native method that is private from the moment it is
    /// defined. The reference writes it as the `MRB_MT_PRIVATE` bit of a ROM table entry
    /// (`include/mruby/class.h`); the hooks (`Module#included`, `#method_added`, …), the
    /// `defined?` helpers and `Module#private` itself all carry it.
    pub fn define_private_method(&mut self, class: ObjId, name: &str, f: crate::object::NativeFn) {
        let n = self.intern(name);
        self.def_method_raw(class, n, Method::Native(f));
        let t = self.def_target(class);
        self.heap.class_mut(t).vis.insert(n, Vis::Private);
    }
    /// Marks methods the class already has private: the `MRB_MT_PRIVATE` bit of an entry in a
    /// table written elsewhere. Use it where the body is already in a [`Vm::define_methods`]
    /// list; [`Vm::define_private_method`] where the entry can be written private outright.
    /// A name the class does not have is passed over (a build feature may have left it out).
    pub fn mark_private(&mut self, class: ObjId, names: &[&str]) {
        for n in names {
            let n = self.intern(n);
            if self.find_method(class, n).is_some() { let _ = self.set_visibility(class, n, Vis::Private); }
        }
    }
    /// [`Vm::define_private_method`] for a list, as [`Vm::define_methods`] is for public ones.
    pub fn define_private_methods(&mut self, class: ObjId, list: &[(&str, crate::object::NativeFn)]) {
        for (n, f) in list {
            self.define_private_method(class, n, *f);
        }
    }
    /// `Module#module_function` with arguments (`mrb_mod_module_function`, src/class.c) for a
    /// method the module already has: the method is copied onto the module's singleton class as
    /// a public one and the instance copy turns private. Answers false where there is no such
    /// method, which is what the caller — an `init` mirroring a gem — wants when the method
    /// belongs to a build feature that is off.
    pub fn make_module_function(&mut self, module: ObjId, name: &str) -> VmResult<bool> {
        let n = self.intern(name);
        let m = match self.find_method(module, n) { Some((m, _)) => m, None => return Ok(false) };
        let sc = self.singleton_class(Value::Obj(module))?;
        self.def_method_raw(sc, n, m);
        self.set_visibility(module, n, Vis::Private)?;
        Ok(true)
    }
    /// `mrb_define_module_function`: the same native as a **public** method on the module's
    /// singleton class and a **private** instance method of the module (src/class.c
    /// `mrb_define_module_function_id` is exactly those two calls). `Kernel.sprintf` and
    /// `sprintf` are one such pair.
    pub fn define_module_function(&mut self, module: ObjId, name: &str, f: crate::object::NativeFn) -> VmResult<()> {
        let sc = self.singleton_class(Value::Obj(module))?;
        self.define_method(sc, name, f);
        self.define_private_method(module, name, f);
        Ok(())
    }
    /// Inserts an include class for `module` right above `class` in the chain.
    pub fn include_module(&mut self, class: ObjId, module: ObjId) {
        let mods = self.module_chain(module);
        let mut at = self.def_target(class); // below the origin when modules are prepended
        for m in mods {
            // already in the chain?
            let mut c = self.heap.class(class).superclass;
            let mut present = false;
            while let Some(x) = c { if self.heap.class(x).iclass_of == Some(m) { present = true; break; } c = self.heap.class(x).superclass; }
            if present { continue; }
            let sup = self.heap.class(at).superclass;
            let ic = self.heap.alloc(self.core.class, ObjKind::Class(ClassData { superclass: sup, iclass_of: Some(m), ..Default::default() }));
            self.heap.class_mut(at).superclass = Some(ic);
            at = ic;
        }
    }
    /// The modules a module brings along (its prepends and includes), nearest first.
    pub fn module_chain(&self, module: ObjId) -> Vec<ObjId> {
        let mut out = vec![];
        let mut c = Some(module);
        while let Some(x) = c {
            let cd = self.heap.class(x);
            if let Some(m) = cd.iclass_of { for y in self.module_chain(m) { if !out.contains(&y) { out.push(y); } } }
            else if let Some(o) = cd.origin_of { if !out.contains(&o) { out.push(o); } }
            else if x == module && cd.origin.is_none() { out.push(x); }
            c = cd.superclass;
        }
        out
    }
    /// True when `class` appears in `module`'s chain (would make a cycle).
    pub fn module_chain_contains(&self, module: ObjId, class: ObjId) -> bool {
        let mut c = Some(module);
        while let Some(x) = c { let cd = self.heap.class(x); if x == class || cd.iclass_of == Some(class) { return true; } c = cd.superclass; }
        false
    }
    /// The singleton class of `v`, created on demand (`prepare_singleton_class`).
    pub fn singleton_class(&mut self, v: Value) -> VmResult<ObjId> {
        let o = match v {
            Value::Obj(o) => o,
            Value::Nil => return Ok(self.core.nil_class),
            Value::True => return Ok(self.core.true_class),
            Value::False => return Ok(self.core.false_class),
            _ => return Err(self.raise_type("can't define singleton")),
        };
        let cur = self.heap.get(o).class;
        if self.heap.class(cur).is_singleton && self.heap.class(cur).attached == Some(Slot::from(v)) {
            return Ok(cur);
        }
        // For a class, the metaclass's superclass is the superclass's metaclass.
        let sup = if self.heap.is_class(o) && !self.heap.class(o).is_module {
            match self.heap.class(o).superclass {
                Some(s) if !self.heap.class(s).is_singleton => Some(self.singleton_class(Value::Obj(s))?),
                Some(s) => Some(s),
                None => Some(cur),
            }
        } else {
            Some(cur)
        };
        let sc = self.heap.alloc(self.core.class, ObjKind::Class(ClassData { superclass: sup, is_singleton: true, attached: Some(Slot::from(v)), ..Default::default() }));
        self.heap.get_mut(o).class = sc;
        if self.heap.get(o).frozen { self.heap.get_mut(sc).frozen = true; }
        Ok(sc)
    }
    pub fn const_get(&self, class: ObjId, name: Sym) -> Option<Value> {
        let mut c = Some(class);
        while let Some(x) = c {
            let cd = self.heap.class(x);
            let tbl = match cd.iclass_of {
                Some(m) => &self.heap.class(m).consts,
                None => &cd.consts,
            };
            if let Some(v) = tbl.get(&name) {
                return Some(v.get());
            }
            c = cd.superclass;
        }
        None
    }
    pub fn obj_is_kind_of(&self, v: Value, class: ObjId) -> bool {
        let mut c = Some(self.class_of(v));
        while let Some(x) = c {
            let cd = self.heap.class(x);
            if x == class || cd.iclass_of == Some(class) {
                return true;
            }
            c = cd.superclass;
        }
        false
    }
    /// Count executions per opcode into [`Vm::op_counts`] (off by default). A host that shows
    /// statistics — the Playground's opcode histogram, the test runner's coverage line — turns
    /// this on before running; a host that does not keeps the instruction loop free of it.
    pub fn set_op_counting(&mut self, on: bool) { self.count_ops = on; }
    /// Whether [`Vm::set_op_counting`] is on.
    pub fn op_counting(&self) -> bool { self.count_ops }

    /// Method lookup along the superclass chain: the entry as it sits in the table, and the
    /// class that owns it (for `super`). A `Method::Undef` answers the lookup by stopping it,
    /// so it comes back as `Some` here and the two wrappers below turn it into `None`.
    fn find_method_entry(&self, class: ObjId, mid: Sym) -> Option<(&Method, ObjId)> {
        let mut c = Some(class);
        while let Some(x) = c {
            let cd = self.heap.class(x);
            if cd.origin.is_some() { c = cd.superclass; continue; } // own table lives in the origin
            let tbl = &self.heap.class(self.table_owner(x)).methods;
            if let Some(m) = tbl.get(&mid) { return Some((m, x)); }
            c = cd.superclass;
        }
        None
    }
    /// Method lookup that clones the `Method`. For the places that want to keep it (`alias`,
    /// `Method#unbind`, copying a table); the dispatch path uses [`Vm::find_method_ref`].
    pub fn find_method(&self, class: ObjId, mid: Sym) -> Option<(Method, ObjId)> {
        match self.find_method_entry(class, mid)? {
            (Method::Undef, _) => None,
            (m, x) => Some((m.clone(), x)),
        }
    }
    /// Method lookup for dispatch: a `Copy` answer, so nothing is cloned and nothing the
    /// caller holds needs dropping (`docs/plans/host-bridge-plan.md`, stage 2 candidate 3).
    pub fn find_method_ref(&self, class: ObjId, mid: Sym) -> Option<(MethodRef, ObjId)> {
        let (m, x) = self.find_method_entry(class, mid)?;
        Some((MethodRef::of(m)?, x))
    }
    /// [`Vm::find_method_ref`] through the method cache. The answer is the same; what the
    /// cache saves is walking the superclass chain and hashing the name at each node.
    /// Anything that could change the answer bumps `Heap::method_serial` (`Heap::class_mut`
    /// and allocating a class), which makes every line stale at once.
    fn find_method_cached(&mut self, class: ObjId, mid: Sym) -> Option<(MethodRef, ObjId)> {
        // the two ids are dense small integers, so the low bits of one would collide with
        // every method of the same class; mixing in a shifted copy spreads them
        let i = ((class.0 as usize).wrapping_mul(31) ^ (mid.0 as usize) ^ ((mid.0 as usize) << 5))
            & (METHOD_CACHE_LEN - 1);
        let serial = self.heap.method_serial;
        let line = self.method_cache[i];
        if line.serial == serial && line.class == class && line.mid == mid { return line.found; }
        let found = self.find_method_ref(class, mid);
        self.method_cache[i] = MethodCacheLine { serial, class, mid, found };
        found
    }
    /// The closure of a [`MethodRef::Closure`] found in `owner`. `find_method_ref` left the
    /// `Arc` where it was, so the call site asks for it here, and only when it has to run one.
    pub fn closure_of(&self, owner: ObjId, mid: Sym) -> Option<crate::object::NativeClosure> {
        match self.heap.class(self.table_owner(owner)).methods.get(&mid) {
            Some(Method::Closure(f)) => Some(f.clone()),
            _ => None,
        }
    }
    // ------------------------------------------------- the guard of the index opcodes

    /// The core class a slot belongs to (mruby `idx_op_class`).
    fn idx_slot_class(&self, slot: usize) -> ObjId {
        match slot {
            IDX_ARY_AREF | IDX_ARY_ASET => self.core.array,
            IDX_HASH_AREF | IDX_HASH_ASET => self.core.hash,
            _ => self.core.string,
        }
    }
    /// The operator a slot is about (mruby `idx_op_mid`).
    fn idx_slot_mid(&self, slot: usize) -> Sym {
        if slot < IDX_ARY_ASET { self.s.aref } else { self.s.aset }
    }
    /// Records what the operator resolves to now as the implementation the opcode may answer
    /// for (mruby `idx_op_arm`). Only a plain native qualifies: anything else means the
    /// operator was replaced by something an opcode cannot stand in for.
    ///
    /// CALLER'S PROMISE, as in the reference: for every argument form the opcode answers
    /// itself, the method now holding the name produces what the recorded one produced. A
    /// method that only widens the operator to an argument type the opcode sends rather than
    /// answers — `String#[]` taking a Regexp — can make it; one that changes the answer for an
    /// Integer, String or Range index cannot.
    #[cfg(feature = "regexp")]
    pub(crate) fn idx_op_rearm(&mut self, slot: usize) {
        let (base, mid) = (self.idx_slot_class(slot), self.idx_slot_mid(slot));
        self.idx_builtin[slot] = match self.find_method_ref(base, mid) {
            Some((MethodRef::Native(f), _)) => Some(f),
            _ => None,
        };
        self.idx_sync();
    }
    /// Records the builtin `[]` / `[]=` of each core class and arms its slot. Called once,
    /// after the natives are installed (mruby `mrb_idx_op_init`).
    pub(crate) fn idx_op_init(&mut self) {
        for slot in 0..IDX_SLOTS {
            let (base, mid) = (self.idx_slot_class(slot), self.idx_slot_mid(slot));
            self.idx_builtin[slot] = match self.find_method_ref(base, mid) {
                Some((MethodRef::Native(f), _)) => Some(f),
                _ => None,
            };
        }
        self.idx_sync();
    }
    /// Rechecks every slot against the implementation it recorded (mruby `idx_op_refresh`).
    /// Validity is the resolved method itself, not "was `[]` assigned to", which covers `def`,
    /// `alias_method`, `undef_method`, `remove_method` and `prepend` without enumerating them,
    /// and re-arms when an override is aliased back away.
    #[cold]
    #[inline(never)]
    fn idx_sync(&mut self) {
        self.idx_serial = self.heap.method_serial;
        for slot in 0..IDX_SLOTS {
            let builtin = match self.idx_builtin[slot] { Some(f) => f, None => { self.idx_class[slot] = None; continue } };
            let (base, mid) = (self.idx_slot_class(slot), self.idx_slot_mid(slot));
            self.idx_class[slot] = match self.find_method_ref(base, mid) {
                Some((MethodRef::Native(f), _)) if core::ptr::fn_addr_eq(f, builtin) => Some(base),
                _ => None,
            };
        }
    }
    /// Whether the opcode may answer `slot` for a receiver whose class is `cls`.
    #[inline]
    fn idx_armed(&mut self, slot: usize, cls: ObjId) -> bool {
        if self.idx_serial != self.heap.method_serial { self.idx_sync(); }
        self.idx_class[slot] == Some(cls)
    }
    /// The `[]` slot of an object's representation, for the three the opcodes answer.
    fn idx_kind(&self, o: ObjId) -> Option<usize> {
        match &self.heap.get(o).kind {
            ObjKind::Array(_) => Some(IDX_ARY_AREF),
            ObjKind::Hash(_) => Some(IDX_HASH_AREF),
            ObjKind::String(_) => Some(IDX_STR_AREF),
            _ => None,
        }
    }
    /// The index types the String branch answers (mruby: Integer, String and Range). A Regexp
    /// is none of the three, so it is sent and reaches the method mruby-regexp installed.
    fn str_index_p(&self, v: Value) -> bool {
        match v {
            Value::Int(_) => true,
            Value::Obj(o) => matches!(self.heap.get(o).kind, ObjKind::String(_) | ObjKind::Range { .. }),
            _ => false,
        }
    }

    pub fn respond_to(&self, v: Value, mid: Sym) -> bool {
        match self.find_method(self.class_of(v), mid) {
            // only a bare `fn` can stand for `mrb_notimplement()`; a Closure never does
            Some((Method::Native(f), _)) => !self.notimpl_fns.iter().any(|g| core::ptr::fn_addr_eq(*g, f)),
            Some(_) => true,
            None => false,
        }
    }

    // ------------------------------------------------------------------ loading

    /// Loads a RITE binary; returns the id of its top-level irep.
    pub fn load(&mut self, bin: &[u8]) -> VmResult<IrepId> {
        let rite = rite::parse(bin)?;
        let offset = self.ireps.len();
        for ir in &rite.ireps {
            let syms = ir.syms.iter().map(|s| match s {
                Some(b) => self.syms.intern(b),
                None => self.syms.intern(b""),
            }).collect();
            let lv = ir.lv.iter().map(|s| s.as_ref().map(|b| self.syms.intern(b))).collect();
            self.ireps.push(VmIrep {
                nlocals: ir.nlocals as usize,
                nregs: ir.nregs as usize,
                iseq: ir.iseq.clone(),
                catch: ir.catch.clone(),
                pool: ir.pool.clone(),
                syms,
                reps: ir.reps.iter().map(|r| r + offset).collect(),
                lv,
                lines: ir.lines.clone(),
                filename: ir.filename.as_ref().map(|f| String::from_utf8_lossy(f).into_owned()),
            });
        }
        Ok(rite.root + offset)
    }

    /// Loads and runs a binary to completion at the top level.
    pub fn load_and_run(&mut self, bin: &[u8]) -> VmResult<Value> {
        let irep = self.load(bin)?;
        self.run_irep(irep)
    }

    /// The default visibility a top-level `def` gets. mruby keeps it on the frame, and the
    /// **base** frame of a context — `c->cibase[0]`, made by `stack_init` (src/vm.c:136,
    /// `c->ci->vis = 1`) — is the one frame that starts out *private*; every frame `cipush`
    /// makes starts public (src/vm.c:868). `mrb_top_run` runs a program on that base frame
    /// when the context is idle and pushes an ordinary (public) frame when it is not, so a
    /// `def` written at the top of a program is private while the same `def` reached through
    /// a nested run — `eval("def a5; end")` — is public. Both were checked against the
    /// reference. A block written at the top level inherits it: the frame's env copies its
    /// visibility (`MRB_ENV_COPY_FLAGS_FROM_CI`), which `EnvData` here does too.
    fn top_vis(&self) -> Vis {
        if self.ci.is_empty() { Vis::Private } else { Vis::Public }
    }
    /// Runs a top-level irep with `self` = main.
    pub fn run_irep(&mut self, irep: IrepId) -> VmResult<Value> {
        let proc_ = self.heap.alloc(self.core.proc_, ObjKind::Proc(ProcData {
            irep, upper: None, env: None, target_class: Some(self.core.object), strict: false, scope: true, orphan: false, mid: None,
        }));
        let base = self.stack.len();
        let nregs = self.ireps[irep].nregs.max(4);
        self.stack.resize(base + nregs, Slot::NIL);
        self.stack[base] = Slot::from(Value::Obj(self.top_self));
        let depth = self.ci.len();
        self.ci.push(CallInfo { base, pc: 0, irep, proc_, n: 0, kw: false, mid: None, target_class: self.core.object, env: None, cci: Cci::Skip, vis: self.top_vis(), modfunc: false, vis_break: false });
        let r = self.run_loop(depth);
        self.stack.truncate(base);
        r
    }

    /// Prepares a top-level irep for stepped execution ([`Vm::step`]).
    pub fn start(&mut self, irep: IrepId) {
        let proc_ = self.heap.alloc(self.core.proc_, ObjKind::Proc(ProcData {
            irep, upper: None, env: None, target_class: Some(self.core.object), strict: false, scope: true, orphan: false, mid: None,
        }));
        let base = self.stack.len();
        let nregs = self.ireps[irep].nregs.max(4);
        self.stack.resize(base + nregs, Slot::NIL);
        self.stack[base] = Slot::from(Value::Obj(self.top_self));
        self.ci.push(CallInfo { base, pc: 0, irep, proc_, n: 0, kw: false, mid: None, target_class: self.core.object, env: None, cci: Cci::Skip, vis: self.top_vis(), modfunc: false, vis_break: false });
    }

    /// Executes at most `budget` instructions of a program started with
    /// [`Vm::start`]. This is the instruction-boundary suspension point
    /// (mruby's `RETURN_IF_TASK_STOPPED`).
    pub fn step(&mut self, budget: u64) -> VmResult<Step> {
        if self.ci.is_empty() {
            return Ok(Step::Finished(Value::Nil));
        }
        self.step_left = Some(budget);
        let r = self.run_loop_ctx(ROOT, 0);
        self.step_left = None;
        match r {
            Ok(v) if self.cur == ROOT && self.ci.is_empty() => { self.stack.clear(); Ok(Step::Finished(v)) }
            Ok(_) => Ok(Step::Paused),
            Err(e) => { self.reset_to_root(); Err(e) }
        }
    }

    // ------------------------------------------------------------------ calls from native code

    /// Calls a method on `recv` from native code.
    pub fn funcall(&mut self, recv: Value, mid: Sym, args: &[Value], blk: Value) -> VmResult<Value> {
        let cls = self.class_of(recv);
        match self.find_method_cached(cls, mid) {
            Some((MethodRef::Native(f), _)) => {
                // native -> native recursion (e.g. inspect of nested containers) also uses the host stack
                if self.native_depth >= NATIVE_DEPTH_MAX { return Err(self.raise(self.core.system_stack_error, "stack level too deep")); }
                self.native_mid = Some(mid);
                self.native_depth += 1;
                let direct = core::mem::replace(&mut self.direct_send, false);
                let r = self.call_native(f, recv, args, blk);
                self.direct_send = direct;
                self.native_depth -= 1;
                self.orphan_block_of_native(blk);
                r
            }
            Some((MethodRef::Closure, owner)) => {
                if self.native_depth >= NATIVE_DEPTH_MAX { return Err(self.raise(self.core.system_stack_error, "stack level too deep")); }
                let f = match self.closure_of(owner, mid) { Some(f) => f, None => return Err(VmError::Internal("closure vanished between lookup and call".into())) };
                self.native_mid = Some(mid);
                self.native_depth += 1;
                let direct = core::mem::replace(&mut self.direct_send, false);
                let r = self.call_closure(&f, recv, args, blk);
                self.direct_send = direct;
                self.native_depth -= 1;
                self.orphan_block_of_native(blk);
                r
            }
            Some((MethodRef::AttrReader(iv), _)) => Ok(recv.obj().map(|o| self.heap.ivar_get(o, iv)).unwrap_or(Value::Nil)),
            Some((MethodRef::AttrWriter(iv), _)) => {
                let v = args.first().copied().unwrap_or(Value::Nil);
                if let Some(o) = recv.obj() { self.heap.ivar_set(o, iv, v); }
                Ok(v)
            }
            Some((MethodRef::Ruby(p), owner)) => {
                // A native forwarding its own arguments (`send`, `Class#new`) keeps
                // the caller's keywords: the trailing Hash is the pending kdict.
                let kw = match (self.pending_kw, args.last()) { (Some(k), Some(l)) if !k.is_nil() && k == *l => Some(k), _ => None };
                let tc = if self.heap.proc_data(p).env.is_some() { None } else { Some(owner) };
                match kw {
                    Some(k) => self.call_proc_with(p, recv, &args[..args.len() - 1], Some(k), blk, Some(mid), tc),
                    None => self.call_proc_with(p, recv, args, None, blk, Some(mid), tc),
                }
            }
            None => {
                // a user-defined method_missing takes the call (the basic one only reports)
                let mm = self.s.method_missing;
                if let Some((m, owner)) = self.find_method(cls, mm) {
                    if !matches!(m, Method::Native(_)) {
                        let mut nargs = vec![Value::Sym(mid)];
                        nargs.extend_from_slice(args);
                        if let Method::Closure(f) = &m {
                            let f = f.clone();
                            return self.call_closure(&f, recv, &nargs, blk);
                        }
                        if let Method::Ruby(p) = m {
                            let kw = match (self.pending_kw, args.last()) { (Some(k), Some(l)) if !k.is_nil() && k == *l => Some(k), _ => None };
                            let pos = if kw.is_some() { &nargs[..nargs.len() - 1] } else { &nargs[..] };
                            let tc = if self.heap.proc_data(p).env.is_some() { None } else { Some(owner) };
                            return self.call_proc_with(p, recv, pos, kw, blk, Some(mm), tc);
                        }
                    }
                }
                let name = self.sym_name(mid);
                let desc = self.describe_for_error(recv);
                let e = self.no_method_error(mid, recv, &format!("undefined method '{name}' for {desc}"));
                if let VmError::Raise(Value::Obj(o)) = e { let av = self.ary_new(args.to_vec()); let k = self.intern("@args"); self.heap.ivar_set(o, k, av); }
                Err(e)
            }
        }
    }

    /// Calls a block/proc from native code.
    pub fn call_block(&mut self, blk: Value, args: &[Value]) -> VmResult<Value> {
        let p = match blk {
            Value::Obj(o) if matches!(self.heap.get(o).kind, ObjKind::Proc(_)) => o,
            Value::Nil => return Err(self.raise(self.core.local_jump_error, "no block given (yield)")),
            _ => return Err(self.raise_type("wrong type (expected Proc)")),
        };
        let pd = self.heap.proc_data(p);
        let self_ = match pd.env {
            Some(e) => self.env_get(e, 0),
            None => Value::Nil,
        };
        let tc = pd.target_class.unwrap_or(self.core.object);
        let mid = pd.env.and_then(|e| self.heap.env(e).mid);
        self.call_proc(p, self_, args, Value::Nil, mid, tc)
    }

    /// Calls a block with an explicit `self` (`instance_eval`, `class_eval`).
    pub fn call_block_with_self(&mut self, blk: Value, self_: Value, args: &[Value]) -> VmResult<Value> {
        self.call_block_with_self_kw(blk, self_, args, None)
    }
    /// [`Vm::call_block_with_self`] with a keyword Hash (`instance_exec(a: 1) { |**kw| }`).
    pub fn call_block_with_self_kw(&mut self, blk: Value, self_: Value, args: &[Value], kw: Option<Value>) -> VmResult<Value> {
        let p = match blk {
            Value::Obj(o) if matches!(self.heap.get(o).kind, ObjKind::Proc(_)) => o,
            _ => return Err(self.raise_type("wrong type (expected Proc)")),
        };
        // `mrb_singleton_class_ptr` is NULL for an Integer/Float/Symbol: the frame then has no
        // target class of its own and OP_CLASS falls back to the block's (its env's) class.
        let tc = match self_ {
            Value::Obj(o) if self.heap.is_class(o) => Some(o),
            Value::Int(_) | Value::Float(_) | Value::Sym(_) => None,
            _ => Some(self.singleton_class(self_)?),
        };
        self.pending_vis_break = true;
        let r = self.call_proc_with(p, self_, args, kw, Value::Nil, None, tc);
        self.pending_vis_break = false;
        r
    }

    /// [`Vm::call_block_with_self_kw`] for a native that returns the block's value as it is
    /// (`instance_exec`, `class_eval`): called by a SEND, the block runs in a frame of its own
    /// instead of a nested loop ([`Vm::exec_proc`]).
    pub(crate) fn exec_block_with_self(&mut self, blk: Value, self_: Value, args: &[Value], kw: Option<Value>) -> VmResult<Value> {
        let p = match blk {
            Value::Obj(o) if matches!(self.heap.get(o).kind, ObjKind::Proc(_)) => o,
            _ => return Err(self.raise_type("wrong type (expected Proc)")),
        };
        let tc = match self_ {
            Value::Obj(o) if self.heap.is_class(o) => Some(o),
            Value::Int(_) | Value::Float(_) | Value::Sym(_) => None,
            _ => Some(self.singleton_class(self_)?),
        };
        self.exec_proc(p, self_, args, kw, Value::Nil, None, tc, true)
    }

    /// [`Vm::call_block`] for a native that returns the block's value as it is (`Hash`'s default
    /// proc, `fetch`-like fallbacks): called by a SEND, the block runs in a frame of its own
    /// ([`Vm::exec_proc`]).
    pub(crate) fn exec_block(&mut self, blk: Value, args: &[Value]) -> VmResult<Value> {
        let p = match blk {
            Value::Obj(o) if matches!(self.heap.get(o).kind, ObjKind::Proc(_)) => o,
            Value::Nil => return Err(self.raise(self.core.local_jump_error, "no block given (yield)")),
            _ => return Err(self.raise_type("wrong type (expected Proc)")),
        };
        let pd = self.heap.proc_data(p);
        let (env, ptc) = (pd.env, pd.target_class);
        let self_ = match env { Some(e) => self.env_get(e, 0), None => Value::Nil };
        let mid = env.and_then(|e| self.heap.env(e).mid);
        // what `call_block` → `call_proc` picks
        let tc = if env.is_some() { None } else { Some(ptc.unwrap_or(self.core.object)) };
        self.exec_proc(p, self_, args, None, Value::Nil, mid, tc, false)
    }

    /// Makes the frame just pushed for the running native answer its receiver (R0) whatever it
    /// returns ([`Cci::KeepSelf`]).
    #[inline]
    pub(crate) fn keep_self(&mut self) {
        if let Some(c) = self.ci.last_mut() { c.cci = Cci::KeepSelf; }
    }

    /// `h[k]` missed (`OP_GETIDX`), and this is what `Hash#[]` answers then: a class that
    /// redefines `default` is answered by that method, nested as `Hash#[]` calls it; a default
    /// proc runs in a frame at `nbase`, the instruction's register, and this answers `None`;
    /// otherwise the plain default.
    fn hash_miss_at(&mut self, h: Value, o: ObjId, k: Value, nbase: usize) -> VmResult<Option<Value>> {
        let dm = self.s.default_;
        let cls = self.class_of(h);
        if let Some((MethodRef::Ruby(_) | MethodRef::Closure, _)) = self.find_method_cached(cls, dm) {
            return self.funcall(h, dm, &[k], Value::Nil).map(Some);
        }
        let p = match self.heap.ivar_get(o, self.s.default_proc) {
            Value::Obj(p) if matches!(self.heap.get(p).kind, ObjKind::Proc(_)) => p,
            _ => return Ok(Some(match &self.heap.get(o).kind { ObjKind::Hash(hd) => hd.default.get(), _ => Value::Nil })),
        };
        if self.ci.len() >= CALL_LEVEL_MAX { return Err(self.raise(self.core.system_stack_error, "stack level too deep")); }
        let pd = self.heap.proc_data(p);
        let (env, ptc) = (pd.env, pd.target_class);
        let self_ = match env { Some(e) => self.env_get(e, 0), None => Value::Nil };
        let mid = self.heap.proc_data(p).mid.or(env.and_then(|e| self.heap.env(e).mid));
        let tc = match env { Some(e) => self.heap.env(e).target_class.unwrap_or(self.core.object), None => ptc.unwrap_or(self.core.object) };
        self.push_frame_at(nbase, p, self_, &[h, k], None, Value::Nil, mid, tc, false, false);
        Ok(None)
    }

    /// Whether `recv.mid` is a method written in Ruby or a closure, through the method cache.
    pub(crate) fn user_method_p(&mut self, recv: Value, mid: Sym) -> bool {
        let cls = self.class_of(recv);
        matches!(self.find_method_cached(cls, mid), Some((MethodRef::Ruby(_) | MethodRef::Closure, _)))
    }

    /// `recv.mid` through the method cache.
    pub(crate) fn method_ref_of(&mut self, recv: Value, mid: Sym) -> Option<(MethodRef, ObjId)> {
        let cls = self.class_of(recv);
        self.find_method_cached(cls, mid)
    }

    /// [`Vm::call_block_with_self`] followed by answering `answer`, which is the block's self
    /// (`Class.new { }`, `Struct.new { }`): called by a SEND, the block runs in a frame of its
    /// own that answers its self whatever it returns ([`Cci::KeepSelf`]).
    pub(crate) fn exec_block_with_self_then(&mut self, blk: Value, self_: Value, args: &[Value], answer: Value) -> VmResult<Value> {
        if !self.direct_send {
            self.call_block_with_self(blk, self_, args)?;
            return Ok(answer);
        }
        // the block's frame answers its self, which is `answer`
        let depth = self.ci.len();
        self.exec_block_with_self(blk, self_, args, None)?;
        if self.ci.len() == depth + 1 && self_ == answer { self.keep_self(); }
        Ok(answer)
    }

    /// [`Vm::send_in_frame`] followed by answering `answer` (`Class#new` and its `initialize`).
    /// A call that runs to its end where it is (a native that pushes nothing) needs no frame to
    /// answer for it, and the one pushed for that is taken off again.
    pub(crate) fn send_in_frame_then(&mut self, answer: Value, recv: Value, mid: Sym, args: &[Value], kw: Option<Value>, blk: Value) -> VmResult<Value> {
        if !self.direct_send {
            self.funcall(recv, mid, args, blk)?;
            return Ok(answer);
        }
        let depth = self.ci.len();
        let name = self.native_mid.unwrap_or(mid);
        self.push_return_frame(answer, name)?;
        let r = self.send_in_frame(recv, mid, args, kw, blk);
        if self.ci.len() == depth + 1 {
            self.ci.pop();
            self.native_ret_reg -= 1;
        }
        r?;
        Ok(answer)
    }

    /// [`Vm::call_method_proc`] for `Method#call` (mruby `mcall` → `mrb_exec_irep`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_method_proc(&mut self, proc_: ObjId, self_: Value, args: &[Value], kw: Option<Value>, blk: Value, mid: Option<Sym>, target_class: ObjId) -> VmResult<Value> {
        let tc = if self.heap.proc_data(proc_).env.is_some() { None } else { Some(target_class) };
        self.exec_proc(proc_, self_, args, kw, blk, mid, tc, false)
    }

    /// Pushes a frame for `proc_` and runs it to completion (re-entrant
    /// execution; native code waits for the result).
    /// Runs a method body proc with keywords (`mrb_exec_irep` for `Method#call`).
    pub fn call_method_proc(&mut self, proc_: ObjId, self_: Value, args: &[Value], kw: Option<Value>, blk: Value, mid: Option<Sym>, target_class: ObjId) -> VmResult<Value> {
        let tc = if self.heap.proc_data(proc_).env.is_some() { None } else { Some(target_class) };
        self.call_proc_with(proc_, self_, args, kw, blk, mid, tc)
    }
    /// `mrb_cv_set` from native code.
    pub fn cvar_store(&mut self, class: ObjId, s: Sym, v: Value) -> VmResult<()> { self.cvar_set(class, s, v) }

    pub fn call_proc(&mut self, proc_: ObjId, self_: Value, args: &[Value], blk: Value, mid: Option<Sym>, target_class: ObjId) -> VmResult<Value> {
        let tc = if self.heap.proc_data(proc_).env.is_some() { None } else { Some(target_class) };
        self.call_proc_with(proc_, self_, args, None, blk, mid, tc)
    }

    /// `override_tc = Some(c)` forces the target class (instance_eval); `None` takes it from the env.
    fn call_proc_with(&mut self, proc_: ObjId, self_: Value, args: &[Value], kw: Option<Value>, blk: Value, mid: Option<Sym>, override_tc: Option<ObjId>) -> VmResult<Value> {
        // mruby: MRB_CALL_LEVEL_MAX (512) frames; here also a cap on host-stack re-entry.
        if self.ci.len() >= CALL_LEVEL_MAX || self.native_depth >= NATIVE_DEPTH_MAX {
            return Err(self.raise(self.core.system_stack_error, "stack level too deep"));
        }
        self.native_depth += 1;
        let r = self.call_proc_inner(proc_, self_, args, kw, blk, mid, override_tc);
        self.native_depth -= 1;
        r
    }

    fn call_proc_inner(&mut self, proc_: ObjId, self_: Value, args: &[Value], kw: Option<Value>, blk: Value, mid: Option<Sym>, override_tc: Option<ObjId>) -> VmResult<Value> {
        let pd = self.heap.proc_data(proc_);
        let irep = pd.irep;
        let env = pd.env;
        let base = self.stack.len();
        let nregs = self.ireps[irep].nregs.max(args.len() + 3).max(4);
        self.stack.resize(base + nregs, Slot::NIL);
        self.stack[base] = Slot::from(self_);
        let mut n = args.len();
        let mut next;
        if n >= 15 {
            let packed = self.ary_new(args.to_vec());
            self.stack[base + 1] = Slot::from(packed);
            n = 15;
            next = base + 2;
        } else {
            for (i, v) in args.iter().enumerate() { self.stack[base + 1 + i] = Slot::from(*v); }
            next = base + 1 + args.len();
        }
        if let Some(k) = kw { self.stack[next] = Slot::from(k); next += 1; }
        self.stack[next] = Slot::from(blk);
        let depth = self.ci.len();
        let tc = match (override_tc, env) {
            (Some(tc), _) => tc,
            (None, Some(e)) => self.heap.env(e).target_class.unwrap_or(self.core.object),
            (None, None) => self.heap.proc_data(proc_).target_class.unwrap_or(self.core.object),
        };
        let vis_break = core::mem::take(&mut self.pending_vis_break);
        let mid = self.heap.proc_data(proc_).mid.or(mid);
        self.ci.push(CallInfo { base, pc: 0, irep, proc_, n: n as u8, kw: kw.is_some(), mid, target_class: tc, env: None, cci: Cci::Skip, vis: Vis::Public, modfunc: false, vis_break });
        let r = self.run_loop(depth);
        self.stack.truncate(base);
        r
    }

    // ------------------------------------------------------------------ calls that stay in the frame

    /// Whether the native that is running was called by a SEND of the running frame (mruby: the
    /// frame's `cci == CINFO_NONE`). Such a native can leave the rest of its work to a frame of
    /// its own instead of running it in a nested loop: what it returns lands in
    /// `native_ret_reg`, and the instruction loop carries on at whatever frame is on top when it
    /// does. A frame pushed that way is an ordinary one — no native boundary — so a task can be
    /// parked in it (`docs/design/fibers.md`, "Native boundaries").
    #[inline]
    pub(crate) fn in_frame(&self) -> bool { self.direct_send }

    /// mruby `mrb_exec_irep` (src/vm.c): runs `proc_` for the native that is running. Called by a
    /// SEND ([`Vm::in_frame`]), the proc becomes a frame of its own where the SEND's result goes
    /// and this returns `self_` (that frame's R0, which is what the SEND then writes there): the
    /// native must return this value as it is and do nothing after. From anywhere else it runs
    /// nested, as [`Vm::call_proc_with`] always did (mruby: `cci != CINFO_NONE`).
    ///
    /// `override_tc` and the frame's `mid` are chosen as [`Vm::call_proc_inner`] chooses them, so
    /// the two ways of running the proc differ in nothing but the boundary.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_proc(&mut self, proc_: ObjId, self_: Value, args: &[Value], kw: Option<Value>, blk: Value, mid: Option<Sym>, override_tc: Option<ObjId>, vis_break: bool) -> VmResult<Value> {
        if !self.direct_send {
            self.pending_vis_break = vis_break;
            let r = self.call_proc_with(proc_, self_, args, kw, blk, mid, override_tc);
            self.pending_vis_break = false;
            return r;
        }
        if self.ci.len() >= CALL_LEVEL_MAX { return Err(self.raise(self.core.system_stack_error, "stack level too deep")); }
        let env = self.heap.proc_data(proc_).env;
        let tc = match (override_tc, env) {
            (Some(tc), _) => tc,
            (None, Some(e)) => self.heap.env(e).target_class.unwrap_or(self.core.object),
            (None, None) => self.heap.proc_data(proc_).target_class.unwrap_or(self.core.object),
        };
        let mid = self.heap.proc_data(proc_).mid.or(mid);
        let nbase = self.native_ret_reg;
        self.push_frame_at(nbase, proc_, self_, args, kw, blk, mid, tc, vis_break, false);
        // one frame per native: a second call from the same native would land on this one
        self.direct_send = false;
        Ok(self_)
    }

    /// Lays out a frame for `proc_` at `nbase` (R0 = `self_`, then the arguments as
    /// [`Vm::relay_args`] writes them) and pushes it as an ordinary frame.
    #[allow(clippy::too_many_arguments)]
    fn push_frame_at(&mut self, nbase: usize, proc_: ObjId, self_: Value, args: &[Value], kw: Option<Value>, blk: Value, mid: Option<Sym>, tc: ObjId, vis_break: bool, pack: bool) {
        let irep = self.heap.proc_data(proc_).irep;
        let (n, kwf) = if pack || args.len() >= 15 {
            // one Array for the positional arguments: `relay_args` makes it
            let c = self.relay_args(nbase, args.to_vec(), kw, blk, pack);
            (c & 0xf, (c >> 4) == 15)
        } else {
            // the layout `relay_args` writes, without the Vec it takes
            let end = nbase + args.len() + usize::from(kw.is_some()) + 2;
            if self.stack.len() < end { self.stack.resize(end, Slot::NIL); }
            for (i, v) in args.iter().enumerate() { self.stack[nbase + 1 + i] = Slot::from(*v); }
            let mut next = nbase + 1 + args.len();
            if let Some(k) = kw { self.stack[next] = Slot::from(k); next += 1; }
            self.stack[next] = Slot::from(blk);
            (args.len(), kw.is_some())
        };
        let used = (if n == 15 { 1 } else { n }) + (kwf as usize) + 2;
        let nregs = self.ireps[irep].nregs.max(used).max(4);
        if self.stack.len() < nbase + nregs { self.stack.resize(nbase + nregs, Slot::NIL); }
        for i in used..nregs { self.stack[nbase + i] = Slot::NIL; }
        self.stack[nbase] = Slot::from(self_);
        self.ci.push(CallInfo { base: nbase, pc: 0, irep, proc_, n: n as u8, kw: kwf, mid, target_class: tc, env: None, cci: Cci::None, vis: Vis::Public, modfunc: false, vis_break });
    }

    /// A native that calls its block again and again (`index { }`, `sort! { }`, `catch { }`),
    /// called by a SEND, leaves the loop to a frame of its own: R0 `recv`, R1 the block, R2
    /// `kind`, R3 0 (not started), then `state`. The frame runs one instruction, `OP_DEBUG`,
    /// which asks the native's step function (`builtins::array::loop_step`) what to do: call
    /// the block — in a frame above this one, whose value lands in [`LOOP_RESULT`] — and ask
    /// again when it returns, or answer. The block's frame is an ordinary one, so a task can
    /// wait inside it; the loop's state lives in registers, where the collector sees it.
    ///
    /// Not called by a SEND ([`Vm::in_frame`] false), the same frame runs in a nested loop,
    /// as a block called from native code always did, and this returns what the loop answers.
    pub(crate) fn push_loop_frame(&mut self, kind: i64, recv: Value, blk: Value, state: &[Value]) -> VmResult<Value> {
        if self.ci.len() >= CALL_LEVEL_MAX || self.native_depth >= NATIVE_DEPTH_MAX {
            return Err(self.raise(self.core.system_stack_error, "stack level too deep"));
        }
        let direct = self.direct_send;
        let nbase = if direct { self.native_ret_reg } else { self.stack.len() };
        let end = nbase + LOOP_RESULT + 1;
        if self.stack.len() < end { self.stack.resize(end, Slot::NIL); }
        self.stack[nbase] = Slot::from(recv);
        self.stack[nbase + 1] = Slot::from(blk);
        self.stack[nbase + 2] = Slot::from(Value::Int(kind));
        self.stack[nbase + 3] = Slot::from(Value::Int(0));
        for (i, v) in state.iter().enumerate() { self.stack[nbase + 4 + i] = Slot::from(*v); }
        self.stack[nbase + 4 + state.len()..=nbase + LOOP_RESULT].fill(Slot::NIL);
        let tc = self.ci.last().map(|c| c.target_class).unwrap_or(self.core.object);
        let mid = self.native_mid;
        let depth = self.ci.len();
        self.ci.push(CallInfo { base: nbase, pc: 0, irep: LOOP_IREP, proc_: self.loop_proc, n: 0, kw: false, mid, target_class: tc, env: None, cci: if direct { Cci::None } else { Cci::Skip }, vis: Vis::Public, modfunc: false, vis_break: false });
        if direct {
            self.direct_send = false;
            // the first step is taken here rather than by one more turn of the instruction loop
            let top = self.ci.len() - 1;
            return match crate::builtins::array::loop_step(self, nbase) {
                Ok(LoopNext::Call(n)) => { self.loop_call_block(top, nbase, n)?; Ok(recv) }
                Ok(LoopNext::Tail(n)) => { self.ci[top].pc = LOOP_TAIL_PC; self.loop_call_block(top, nbase, n)?; Ok(recv) }
                // over before the block was called: no frame is needed after all
                Ok(LoopNext::Done(v)) => { self.ci.pop(); Ok(v) }
                Err(e) => { self.ci.pop(); Err(e) }
            };
        }
        self.native_depth += 1;
        let r = self.run_loop(depth);
        self.native_depth -= 1;
        self.stack.truncate(nbase);
        r
    }

    /// Writes argument `i` of the block call a native loop's step asks for ([`LoopNext::Call`]).
    #[inline]
    pub(crate) fn loop_arg(&mut self, base: usize, i: usize, v: Value) {
        let at = base + LOOP_RESULT + 1 + i;
        if self.stack.len() <= at + 1 { self.stack.resize(at + 2, Slot::NIL); }
        self.stack[at] = Slot::from(v);
    }

    /// Pushes the frame of the loop frame `top`'s block with `n` arguments at [`LOOP_RESULT`], as
    /// `OP_BLKCALL` pushes one.
    fn loop_call_block(&mut self, top: usize, base: usize, n: usize) -> VmResult<()> {
        if self.ci.len() >= CALL_LEVEL_MAX { return Err(self.raise(self.core.system_stack_error, "stack level too deep")); }
        let p = match self.stack[base + 1].get() {
            Value::Obj(o) if matches!(self.heap.get(o).kind, ObjKind::Proc(_)) => o,
            _ => return Err(self.raise_type("wrong type (expected Proc)")),
        };
        let nbase = base + LOOP_RESULT;
        if self.stack.len() <= nbase + n + 1 { self.stack.resize(nbase + n + 2, Slot::NIL); }
        self.stack[nbase + n + 1] = Slot::NIL;
        let tc = self.ci[top].target_class;
        self.ci.push(CallInfo { base: nbase, pc: 0, irep: 0, proc_: p, n: n as u8, kw: false, mid: None, target_class: tc, env: None, cci: Cci::None, vis: Vis::Public, modfunc: false, vis_break: false });
        self.vm_call_proc(p, n + 2);
        Ok(())
    }

    /// Pushes the frame a SEND of `mid` would push for the Ruby method `p` found on `owner`
    /// (`op_send_vis`), where the running native's result goes. The native returns `recv`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_method_frame(&mut self, p: ObjId, owner: ObjId, recv: Value, mid: Sym, args: &[Value], kw: Option<Value>, blk: Value, pack: bool) -> VmResult<()> {
        if self.ci.len() >= CALL_LEVEL_MAX { return Err(self.raise(self.core.system_stack_error, "stack level too deep")); }
        let mid = self.heap.proc_data(p).mid.unwrap_or(mid);
        let nbase = self.native_ret_reg;
        self.push_frame_at(nbase, p, recv, args, kw, blk, Some(mid), owner, false, pack);
        self.direct_send = false;
        Ok(())
    }

    /// Pushes a frame that answers `value` when the frame pushed above it returns, and moves the
    /// running native's result register into it: what the native pushes next runs there, and
    /// its value is dropped for `value` (`Class#new` answers the object whatever `initialize`
    /// returns). mruby does this with a method written in bytecode (`new_iseq` of src/class.c);
    /// the frame here is the tail of such a method, one `OP_RETURN R0`. Where the frame pushed
    /// above has the value as its own R0, [`Cci::KeepSelf`] does the same without this frame;
    /// this one is for a native `initialize` that pushes a frame of another self.
    pub(crate) fn push_return_frame(&mut self, value: Value, mid: Sym) -> VmResult<()> {
        if self.ci.len() + 1 >= CALL_LEVEL_MAX { return Err(self.raise(self.core.system_stack_error, "stack level too deep")); }
        let p = self.ret_proc;
        let nbase = self.native_ret_reg;
        let tc = self.ci.last().map(|c| c.target_class).unwrap_or(self.core.object);
        self.push_frame_at(nbase, p, value, &[], None, Value::Nil, Some(mid), tc, false, false);
        self.native_ret_reg = nbase + 1;
        Ok(())
    }

    /// `recv.mid(*args, **kw, &blk)` as the SEND that called the running native would make it
    /// (mruby `send_method`): a Ruby method or a Ruby `method_missing` gets a frame where the
    /// native's result goes, and a native is called with the frame left as it is. From anywhere
    /// else it is [`Vm::funcall`]. `args` carries the keyword Hash last, as a native's own
    /// arguments do; `kw` says whether it is one.
    pub(crate) fn send_in_frame(&mut self, recv: Value, mid: Sym, args: &[Value], kw: Option<Value>, blk: Value) -> VmResult<Value> {
        if !self.direct_send { return self.funcall(recv, mid, args, blk); }
        let pos = if kw.is_some() { &args[..args.len() - 1] } else { args };
        let cls = self.class_of(recv);
        match self.find_method_cached(cls, mid) {
            Some((MethodRef::Ruby(p), owner)) => {
                self.push_method_frame(p, owner, recv, mid, pos, kw, blk, false)?;
                Ok(recv)
            }
            Some((MethodRef::Native(f), _)) => {
                if self.native_depth >= NATIVE_DEPTH_MAX { return Err(self.raise(self.core.system_stack_error, "stack level too deep")); }
                self.native_mid = Some(mid);
                self.native_depth += 1;
                let r = self.call_native(f, recv, args, blk);
                self.native_depth -= 1;
                r
            }
            None => {
                let mm = self.s.method_missing;
                if let Some((MethodRef::Ruby(p), owner)) = self.find_method_cached(cls, mm) {
                    let mut nargs = vec![Value::Sym(mid)];
                    nargs.extend_from_slice(pos);
                    self.push_method_frame(p, owner, recv, mm, &nargs, kw, blk, true)?;
                    return Ok(recv);
                }
                self.funcall(recv, mid, args, blk)
            }
            _ => self.funcall(recv, mid, args, blk),
        }
    }

    /// What the native boundary nearest the running code is, for the error a wait raises there
    /// (`can't wait inside Array#sort's call to a block`). A boundary is a frame a native
    /// started (`Cci::Skip`); the frame below it made the call that reached that native, and
    /// its `pc` still points past that instruction, whose receiver is still in its register.
    pub(crate) fn boundary_name(&mut self) -> String {
        let i = match (1..self.ci.len()).rev().find(|&i| self.ci[i].cci == Cci::Skip) { Some(i) => i, None => return "a native method".into() };
        let callee = {
            let ci = &self.ci[i];
            let pd = self.heap.proc_data(ci.proc_);
            match (pd.scope || pd.mid.is_some(), ci.mid) {
                (true, Some(m)) => format!("#{}", self.sym_name(m)),
                _ => "a block".into(),
            }
        };
        let caller = self.ci[i - 1];
        let who = match self.send_before(caller.irep, caller.pc) {
            Some((a, mid)) => {
                let recv = self.stack.get(caller.base + a).map(|s| s.get()).unwrap_or(Value::Nil);
                let name = self.sym_name(mid);
                match recv {
                    Value::Obj(o) if self.heap.is_class(o) => format!("{}.{name}", self.class_name(o)),
                    _ => {
                        // the method's own class or module (`Kernel#catch`, not `Object#catch`)
                        let cls = self.class_of(recv);
                        let owner = match self.find_method(cls, mid) { Some((_, o)) => self.heap.class(o).iclass_of.unwrap_or(o), None => self.real_class_of(recv) };
                        format!("{}#{name}", self.class_name(owner))
                    }
                }
            }
            None => "a native method".into(),
        };
        format!("{who}'s call to {callee}")
    }

    /// The call instruction that ends right before `pc` in `irep`, as (register, method name):
    /// a SEND of any kind, `super`, or one of the index opcodes.
    fn send_before(&self, irep: IrepId, pc: usize) -> Option<(usize, Sym)> {
        let code = &self.ireps.get(irep)?.iseq;
        let mut at = 0usize;
        let mut ext = 0u8;
        while at < pc {
            let start = at;
            let op = Op::from_u8(*code.get(at)?)?;
            at += 1;
            let (aw, bw) = (ext == 1 || ext == 3, ext == 2 || ext == 3);
            let rd = |at: &mut usize, wide: bool| -> Option<usize> {
                let v = if wide { ((*code.get(*at)? as usize) << 8) | *code.get(*at + 1)? as usize } else { *code.get(*at)? as usize };
                *at += if wide { 2 } else { 1 };
                Some(v)
            };
            let (mut a, mut b) = (0, 0);
            match op.operands() {
                Operands::Z => {}
                Operands::B => a = rd(&mut at, aw)?,
                Operands::BB => { a = rd(&mut at, aw)?; b = rd(&mut at, bw)?; }
                Operands::BBB => { a = rd(&mut at, aw)?; b = rd(&mut at, bw)?; at += 1; }
                Operands::BS => { a = rd(&mut at, aw)?; at += 2; }
                Operands::BSS => { a = rd(&mut at, aw)?; at += 4; }
                Operands::S => at += 2,
                Operands::W => at += 3,
            }
            ext = match op { Op::Ext1 => 1, Op::Ext2 => 2, Op::Ext3 => 3, _ => 0 };
            if at == pc && start < pc {
                let syms = &self.ireps[irep].syms;
                return match op {
                    Op::Send | Op::Send0 | Op::Sendb | Op::Ssend | Op::Ssend0 | Op::Ssendb => Some((a, *syms.get(b)?)),
                    Op::Getidx | Op::Getidx0 => Some((a, self.s.aref)),
                    Op::Setidx => Some((a, self.s.aset)),
                    Op::Super => None,
                    _ => None,
                };
            }
        }
        None
    }


    // ------------------------------------------------------------------ fibers (mruby-fiber)

    /// Calls a native method on behalf of a SEND instruction. Returns the value
    /// and whether the native switched fibers; in that case the value was
    /// delivered to the register the new context waits on (or, when the fiber
    /// that yielded had been resumed by native code, the run loop is told to
    /// return it) and the caller must not write it to its own register.
    fn call_native_direct(&mut self, f: crate::object::NativeFn, recv: Value, args: &[Value], blk: Value, ret_reg: usize) -> VmResult<(Value, bool)> {
        let ctx0 = self.cur;
        let direct = core::mem::replace(&mut self.direct_send, true);
        let reg0 = core::mem::replace(&mut self.native_ret_reg, ret_reg);
        let r = self.call_native(f, recv, args, blk);
        self.native_ret_reg = reg0;
        self.direct_send = direct;
        self.orphan_block_of_native(blk);
        let v = r?;
        if self.cur == ctx0 { return Ok((v, false)); }
        if self.loop_exit.is_some() { return Ok((v, true)); }
        self.deliver(v);
        Ok((v, true))
    }

    /// Runs a native with the collector held off (its Rust locals are not roots).
    #[inline]
    pub fn call_native(&mut self, f: crate::object::NativeFn, recv: Value, args: &[Value], blk: Value) -> VmResult<Value> {
        self.native_active += 1;
        let r = f(self, recv, args, blk);
        self.native_active -= 1;
        if self.task.native_sampling { crate::builtins::ext_task::count_native(self); }
        r
    }

    /// [`Vm::call_native`] for a closure method (`Vm::define_closure`). The `Arc` is cloned by
    /// the caller (method lookup returns the method by value), so the closure stays alive even
    /// if the method it came from is redefined while it runs.
    #[inline]
    pub fn call_closure(&mut self, f: &crate::object::NativeClosure, recv: Value, args: &[Value], blk: Value) -> VmResult<Value> {
        self.native_active += 1;
        let r = (f.f)(self, recv, args, blk);
        self.native_active -= 1;
        if self.task.native_sampling { crate::builtins::ext_task::count_native(self); }
        r
    }

    /// [`Vm::call_native_direct`](Vm::call_native) for a closure method.
    fn call_closure_direct(&mut self, f: &crate::object::NativeClosure, recv: Value, args: &[Value], blk: Value, ret_reg: usize) -> VmResult<(Value, bool)> {
        let ctx0 = self.cur;
        let direct = core::mem::replace(&mut self.direct_send, true);
        let reg0 = core::mem::replace(&mut self.native_ret_reg, ret_reg);
        let r = self.call_closure(f, recv, args, blk);
        self.native_ret_reg = reg0;
        self.direct_send = direct;
        self.orphan_block_of_native(blk);
        let v = r?;
        if self.cur == ctx0 { return Ok((v, false)); }
        if self.loop_exit.is_some() { return Ok((v, true)); }
        self.deliver(v);
        Ok((v, true))
    }

    /// A block made by the running frame and passed to a native that has now
    /// returned loses its home (mruby `cipop` of the C frame: MRB_PROC_ORPHAN),
    /// so `proc { break }.call` is a LocalJumpError.
    fn orphan_block_of_native(&mut self, blk: Value) {
        if let Value::Obj(b) = blk {
            if let ObjKind::Proc(pd) = &self.heap.get(b).kind {
                let caller_env = self.ci.last().and_then(|c| c.env);
                if !pd.strict && pd.env.is_some() && pd.env == caller_env {
                    if let ObjKind::Proc(pd) = &mut self.heap.get_mut(b).kind { pd.orphan = true; }
                }
            }
        }
    }

    /// `send`/`__send__` issued by a SEND: shifts the arguments down and
    /// dispatches the named method in the same frame (visibility ignored).
    fn op_send_redirect(&mut self, base: usize, a: usize, argc: usize, kw: bool, has_blk: bool, blk: Value) -> VmResult<()> {
        let (args, kd) = self.native_args(base + a, argc, kw);
        if args.is_empty() { return Err(self.argnum_error(0, "1+")); }
        let mid = match args[0] {
            Value::Sym(m) => m,
            v => match self.str_bytes(v) { Some(b) => { let n = String::from_utf8_lossy(b).into_owned(); self.intern(&n) } None => { let d = self.inspect_str(v)?; return Err(self.raise_type(&format!("{d} is not a symbol nor a string"))) } },
        };
        let rest: Vec<Value> = args[1..].to_vec();
        // mruby's `send_method` keeps the shape it was called with: `n == 15` stays 15
        // (`mrb_ary_subseq` of the packed Array), anything else is shifted down one
        // register. Unpacking here instead would hand `OP_ENTER` a frame it reads by its
        // fast path, which counts `ci->kw` as an argument — `[1,2,3].__send__(:each,
        // *[], **{}, &b)`, which is what `Enumerator#each` does, would be one too many.
        let c = self.relay_args(base + a, rest, kd, blk, argc == 15);
        self.op_send_vis(base, a, mid, c, has_blk, false, false)
    }

    /// Writes `args` (then the keyword Hash, then the block) back into the argument
    /// registers of the call at `nbase`, and answers the `c` operand that describes the
    /// new layout. Used by the two re-dispatches that change a call's arguments without
    /// leaving the frame: `send` drops its first argument ([`Vm::op_send_redirect`]) and
    /// `method_missing` gains one ([`Vm::op_send_vis`]). 15 or more positional arguments
    /// go into one Array, which is what `n == 15` means to `OP_ENTER`.
    ///
    /// `pack` puts them into that Array whatever their number. mruby's two re-dispatches
    /// differ exactly there: `send_method` (src/vm.c) shifts the registers down and leaves
    /// `ci->n` as it was, while `prepare_missing` goes through `mrb_args_pack_positional`,
    /// which always sets `ci->n = CALL_MAXARGS`. It is not a free choice of layout when the
    /// frame carries a keyword Hash, because `OP_ENTER`'s fast path — the one that counts
    /// `ci->kw` as an argument — is only taken while `argc < 15`.
    fn relay_args(&mut self, nbase: usize, args: Vec<Value>, kd: Option<Value>, blk: Value, pack: bool) -> usize {
        let (n, mut next) = if pack || args.len() >= 15 {
            let packed = self.ary_new(args);
            if self.stack.len() < nbase + 2 { self.stack.resize(nbase + 2, Slot::NIL); }
            self.stack[nbase + 1] = Slot::from(packed);
            (15usize, nbase + 2)
        } else {
            if self.stack.len() < nbase + 1 + args.len() { self.stack.resize(nbase + 1 + args.len(), Slot::NIL); }
            for (i, v) in args.iter().enumerate() { self.stack[nbase + 1 + i] = Slot::from(*v); }
            (args.len(), nbase + 1 + args.len())
        };
        let c = if let Some(k) = kd { if self.stack.len() <= next { self.stack.resize(next + 1, Slot::NIL); } self.stack[next] = Slot::from(k); next += 1; n | (15 << 4) } else { n };
        if self.stack.len() <= next { self.stack.resize(next + 1, Slot::NIL); }
        self.stack[next] = Slot::from(blk);
        c
    }

    /// Writes `v` into the register the current context is suspended in.
    pub(crate) fn deliver(&mut self, v: Value) {
        if let Some(reg) = self.contexts[self.cur].pending_reg.take() {
            if reg < self.stack.len() { self.stack[reg] = Slot::from(v); }
        }
    }

    /// Makes `to` the running context (`fiber_switch_context`).
    pub(crate) fn switch_context(&mut self, to: usize, kind: SwitchKind) {
        let from = self.cur;
        if from == to { return; }
        if self.trace.is_some() { self.record(TraceEvent::FiberSwitch { from, to, kind }); }
        core::mem::swap(&mut self.stack, &mut self.contexts[from].stack);
        core::mem::swap(&mut self.ci, &mut self.contexts[from].ci);
        core::mem::swap(&mut self.stack, &mut self.contexts[to].stack);
        core::mem::swap(&mut self.ci, &mut self.contexts[to].ci);
        self.contexts[to].status = FiberState::Running;
        self.cur = to;
    }

    /// Back to the root context with everything unwound (after an abort).
    pub fn reset_to_root(&mut self) {
        if self.cur != ROOT {
            self.ci.clear();
            self.stack.clear();
            self.contexts[self.cur].status = FiberState::Terminated;
            self.switch_context(ROOT, SwitchKind::Reset);
        }
        self.ci.clear();
        self.stack.clear();
        self.loop_exit = None;
    }

    /// Terminates the running fiber and switches to the context it returns to
    /// (`fiber_terminate`). Returns whether the fiber was running under a
    /// native resume, in which case the caller ends the nested run loop.
    fn fiber_terminate(&mut self) -> bool {
        let c = self.cur;
        let vmexec = core::mem::take(&mut self.contexts[c].vmexec);
        self.contexts[c].status = FiberState::Terminated;
        let prev = self.contexts[c].prev.take();
        self.switch_context(prev.unwrap_or(ROOT), SwitchKind::Terminate);
        self.contexts[c].stack = Vec::new();
        self.contexts[c].ci = Vec::new();
        vmexec
    }

    fn fiber_context(&mut self, fib: Value) -> VmResult<usize> {
        match fib.obj().map(|o| &self.heap.get(o).kind) {
            Some(ObjKind::Fiber(c)) if *c != usize::MAX => Ok(*c),
            Some(ObjKind::Fiber(_)) => Err(self.raise(self.core.fiber_error, "uninitialized Fiber")),
            _ => Err(self.raise_type("not a Fiber")),
        }
    }

    /// `Fiber#initialize`: gives the Fiber object a fresh context that will run `proc_`.
    pub fn fiber_init(&mut self, fib: Value, proc_: ObjId) -> VmResult<()> {
        let o = match fib { Value::Obj(o) => o, _ => return Err(self.raise_type("not a Fiber")) };
        if !matches!(self.heap.get(o).kind, ObjKind::Fiber(_)) { return Err(self.raise_type("not a Fiber")); }
        if let ObjKind::Fiber(c) = self.heap.get(o).kind { if c != usize::MAX { return Err(self.raise(self.core.runtime_error, "cannot initialize twice")); } }
        let mut ctx = Context::new(FiberState::Created);
        ctx.fib = Some(o);
        ctx.proc_ = Some(proc_);
        self.contexts.push(ctx);
        let id = self.contexts.len() - 1;
        if let ObjKind::Fiber(c) = &mut self.heap.get_mut(o).kind { *c = id; }
        Ok(())
    }

    /// `Fiber.current`: the Fiber object of the running context (made on first use for the root).
    pub fn fiber_current(&mut self) -> Value {
        if let Some(f) = self.contexts[self.cur].fib { return Value::Obj(f); }
        let f = self.heap.alloc(self.core.fiber, ObjKind::Fiber(self.cur));
        self.contexts[self.cur].fib = Some(f);
        Value::Obj(f)
    }

    pub fn fiber_state(&mut self, fib: Value) -> VmResult<FiberState> {
        let c = self.fiber_context(fib)?;
        Ok(self.contexts[c].status)
    }

    fn fiber_result(&mut self, args: &[Value]) -> Value {
        match args.len() { 0 => Value::Nil, 1 => args[0], _ => self.ary_new(args.to_vec()) }
    }

    /// `fiber_check_cfunc`: a context with a native frame on the host stack
    /// cannot be switched. The entry frame of a context (index 0: the fiber's
    /// block, or the top-level program of the root) does not count, like
    /// mruby's `cibase` in `task_across_c_boundary`.
    pub(crate) fn fiber_check_native(&self, ctx: usize) -> bool {
        let ci = if ctx == self.cur { &self.ci } else { &self.contexts[ctx].ci };
        ci.iter().skip(1).any(|c| c.cci == Cci::Skip)
    }

    /// `Fiber#resume` from native code (`mrb_fiber_resume`): runs the fiber in a
    /// nested loop until it yields or finishes, and returns that value.
    pub fn fiber_resume(&mut self, fib: Value, args: &[Value]) -> VmResult<Value> {
        self.fiber_switch(fib, args, true, true)
    }

    /// `Fiber#resume` / `Fiber#transfer` core (`fiber_switch`). With `vmexec` false the
    /// switch takes effect in the current run loop (the native returns and the
    /// loop continues in the new context); `resume` false is a transfer.
    pub fn fiber_switch(&mut self, fib: Value, args: &[Value], resume: bool, vmexec: bool) -> VmResult<Value> {
        let c = self.fiber_context(fib)?;
        let old = self.cur;
        if resume && c == old { return Err(self.raise(self.core.fiber_error, "attempt to resume the current fiber")); }
        let status = self.contexts[c].status;
        match status {
            FiberState::Transferred if resume => return Err(self.raise(self.core.fiber_error, "resuming transferred fiber")),
            FiberState::Running | FiberState::Resumed => return Err(self.raise(self.core.fiber_error, "double resume")),
            FiberState::Terminated => return Err(self.raise(self.core.fiber_error, "resuming dead fiber")),
            _ => {}
        }
        if self.fiber_check_native(c) { return Err(self.raise(self.core.fiber_error, "can't cross C function boundary")); }
        if resume {
            self.contexts[old].status = FiberState::Resumed;
            self.contexts[c].prev = Some(old);
        } else {
            self.contexts[old].status = FiberState::Transferred;
            self.contexts[c].prev = None;
        }
        if vmexec { self.contexts[c].vmexec = true; } else { self.contexts[old].pending_reg = Some(self.native_ret_reg); }
        self.switch_context(c, SwitchKind::Resume);
        let value;
        if status == FiberState::Created {
            let p = match self.contexts[c].proc_ { Some(p) => p, None => return Err(self.raise(self.core.fiber_error, "double resume (current)")) };
            let (irep, env, tc) = { let pd = self.heap.proc_data(p); (pd.irep, pd.env, pd.target_class) };
            let self_ = match env { Some(e) => self.env_get(e, 0), None => Value::Obj(self.top_self) };
            let nregs = self.ireps[irep].nregs.max(args.len() + 3).max(4);
            self.stack.clear();
            self.stack.resize(nregs, Slot::NIL);
            self.stack[0] = Slot::from(self_);
            let n = if args.len() >= 15 { let packed = self.ary_new(args.to_vec()); self.stack[1] = Slot::from(packed); 15 } else { for (i, v) in args.iter().enumerate() { self.stack[1 + i] = Slot::from(*v); } args.len() };
            let tc = tc.unwrap_or(self.core.object);
            self.ci.clear();
            self.ci.push(CallInfo { base: 0, pc: 0, irep, proc_: p, n: n as u8, kw: false, mid: None, target_class: tc, env: None, cci: Cci::None, vis: Vis::Public, modfunc: false, vis_break: false });
            value = self_;
        } else {
            value = self.fiber_result(args);
            if vmexec { self.deliver(value); }
        }
        if vmexec {
            let r = self.run_loop_ctx(c, 0);
            // the fiber yielded (loop_exit) or terminated: we are back in `old`
            debug_assert_eq!(self.cur, old);
            r
        } else {
            Ok(value)
        }
    }

    /// `Fiber.yield` (`mrb_fiber_yield`): switch back to the resumer. The value
    /// returned must be returned as-is by the native that called this.
    pub fn fiber_yield(&mut self, args: &[Value]) -> VmResult<Value> {
        let c = self.cur;
        let prev = match self.contexts[c].prev { Some(p) => p, None => return Err(self.raise(self.core.fiber_error, "attempt to yield on a not resumed fiber")) };
        if c == ROOT { return Err(self.raise(self.core.fiber_error, "can't yield from root fiber")); }
        if self.contexts[prev].status == FiberState::Transferred { return Err(self.raise(self.core.fiber_error, "attempt to yield on a not resumed fiber")); }
        if !self.direct_send || self.fiber_check_native(c) { return Err(self.raise(self.core.fiber_error, "can't cross C function boundary")); }
        let value = self.fiber_result(args);
        self.contexts[c].status = FiberState::Suspended;
        self.contexts[c].pending_reg = Some(self.native_ret_reg);
        self.contexts[c].prev = None;
        let vmexec = core::mem::take(&mut self.contexts[c].vmexec);
        self.switch_context(prev, SwitchKind::Yield);
        if vmexec { self.loop_exit = Some(value); }
        Ok(value)
    }

    /// `Fiber#transfer`.
    pub fn fiber_transfer(&mut self, fib: Value, args: &[Value]) -> VmResult<Value> {
        let c = self.fiber_context(fib)?;
        // fiber_check_cfunc_recursive: no native frame anywhere on the chain of resumers
        if !self.direct_send { return Err(self.raise(self.core.fiber_error, "can't cross C function boundary")); }
        let mut x = Some(self.cur);
        while let Some(i) = x {
            if self.fiber_check_native(i) || self.contexts[i].vmexec { return Err(self.raise(self.core.fiber_error, "can't cross C function boundary")); }
            if i == ROOT { break; }
            x = self.contexts[i].prev;
        }
        if self.contexts[c].status == FiberState::Resumed { return Err(self.raise(self.core.fiber_error, "attempt to transfer to a resuming fiber")); }
        if c == ROOT {
            let value = self.fiber_result(args);
            let cur = self.cur;
            if cur == ROOT { return Ok(value); }
            self.contexts[cur].status = FiberState::Transferred;
            self.contexts[cur].pending_reg = Some(self.native_ret_reg);
            self.switch_context(ROOT, SwitchKind::Reset);
            return Ok(value);
        }
        if c == self.cur { return Ok(self.fiber_result(args)); }
        self.fiber_switch(fib, args, false, false)
    }

    /// `mrb_get_backtrace` as `caller` sees it: one `file:line:in method` entry per Ruby
    /// frame of the running context, innermost first. Frames without debug info are left
    /// out, as the reference leaves them out. `native` names the native being run: it
    /// comes first, located at the frame that called it (the reference's C frames are
    /// located at the nearest Ruby frame below them the same way).
    pub fn backtrace(&self, native: Option<Sym>) -> Vec<String> {
        let loc = |ci: &CallInfo| -> Option<String> {
            let ir = self.ireps.get(ci.irep)?;
            if ir.lines.is_empty() { return None; }
            let file = ir.filename.as_deref().unwrap_or("(unknown)");
            // `ci.pc` is past the instruction being executed
            Some(match ir.line_of(ci.pc.saturating_sub(1)) { Some(l) => format!("{file}:{l}"), None => format!("{file}:0") })
        };
        let mut out = Vec::new();
        if let Some(m) = native {
            if let Some(top) = self.ci.iter().rev().find_map(|ci| loc(ci)) {
                out.push(format!("{top}:in {}", self.syms.name_str(m)));
            }
        }
        for ci in self.ci.iter().rev() {
            let Some(mut s) = loc(ci) else { continue };
            if let Some(m) = ci.mid { s.push_str(":in "); s.push_str(&self.syms.name_str(m)); }
            out.push(s);
        }
        out
    }

    /// Where an exception was raised, kept as the frames rather than as text (`mrb_keep_backtrace`):
    /// a program that uses exceptions for control raises far more often than it reads
    /// `Exception#backtrace`, so the strings are built only when they are asked for. The record is
    /// a flat Array of `[irep, pc, mid]` triples, innermost frame first; `mid` is `-1` for a frame
    /// with no method name (a block). An exception that carries one already keeps it: a re-raise
    /// does not move where it came from.
    fn keep_backtrace(&mut self, exc: ObjId) {
        let k = self.intern("@__bt");
        if self.heap.get(exc).ivars.iter().any(|(n, _)| *n == k) { return; }
        let mut flat: Vec<Value> = Vec::with_capacity(self.ci.len() * 3);
        for ci in self.ci.iter().rev() {
            flat.push(Value::Int(ci.irep as i64));
            flat.push(Value::Int(ci.pc as i64));
            flat.push(Value::Int(ci.mid.map(|m| m.0 as i64).unwrap_or(-1)));
        }
        let a = self.ary_new(flat);
        self.heap.ivar_set(exc, k, a);
    }

    /// The text of the record `keep_backtrace` made, in the format `caller` uses. Frames the
    /// build kept no line numbers for are left out, as they are there.
    pub fn backtrace_text(&self, flat: &[i64]) -> Vec<String> {
        let mut out = Vec::new();
        for f in flat.chunks(3) {
            let [irep, pc, mid] = *f else { continue };
            let Some(ir) = self.ireps.get(irep as usize) else { continue };
            if ir.lines.is_empty() { continue; }
            let file = ir.filename.as_deref().unwrap_or("(unknown)");
            // `pc` is past the instruction that raised
            let line = ir.line_of((pc as usize).saturating_sub(1)).unwrap_or(0);
            let mut s = format!("{file}:{line}");
            if mid >= 0 { s.push_str(":in "); s.push_str(&self.syms.name_str(crate::symbol::Sym(mid as u32))); }
            out.push(s);
        }
        out
    }

    /// Source line of the instruction that will run **next** (`None` without debug info).
    ///
    /// `ci.pc` is past the instruction being executed, so this is the line of the innermost
    /// frame's *next* instruction — the line a debugger stopped at an instruction boundary
    /// highlights, and what the playground's stepper reads. For the line an error names,
    /// which from inside a native is the line of the call, see [`Vm::backtrace_line`].
    pub fn next_line(&self) -> Option<u32> {
        let ci = self.ci.last()?;
        self.ireps[ci.irep].line_of(ci.pc)
    }

    /// The line the first frame of [`Vm::backtrace`] carries: where the instruction now
    /// running is. From inside a native — a `define_fn` or `define_closure` method, which
    /// pushes no frame of its own — that is the line of the call, so a native that records
    /// this and a native that raises name the same line.
    ///
    /// Not [`Vm::next_line`], which is one instruction later. The two differ whenever the
    /// call is the last instruction of its line, which is the usual case for a statement on a
    /// line of its own: `unit :metre, symbol: "m"` on line 1 of a file gives `Some(1)` here
    /// and `Some(2)` there.
    ///
    /// `None` where no frame below has debug information — the frames `backtrace` leaves out.
    pub fn backtrace_line(&self) -> Option<u32> {
        for ci in self.ci.iter().rev() {
            let Some(ir) = self.ireps.get(ci.irep) else { continue };
            if ir.lines.is_empty() { continue; }
            // the `:0` `backtrace` would print for a frame it cannot place is `None` here
            return ir.line_of(ci.pc.saturating_sub(1));
        }
        None
    }

    // ------------------------------------------------------------------ garbage collection

    /// Keeps `id` alive until [`Vm::gc_unregister`] (`mrb_gc_register`): for objects a
    /// host holds across calls into the VM, which the collector cannot otherwise see.
    pub fn gc_register(&mut self, id: ObjId) {
        self.gc_registered.push(id);
    }
    /// Drops one registration made by [`Vm::gc_register`].
    pub fn gc_unregister(&mut self, id: ObjId) {
        if let Some(i) = self.gc_registered.iter().rposition(|x| *x == id) { self.gc_registered.swap_remove(i); }
    }
    /// Stress mode (mruby `MRB_GC_STRESS`): every allocation makes a collection due.
    /// Installs the host the VM asks for compilation and files (`src/host.rs`). Without one,
    /// `eval` raises NotImplementedError.
    pub fn set_host(&mut self, host: alloc::boxed::Box<dyn crate::host::Host + Send + Sync>) {
        self.host = Some(host);
    }

    pub fn set_gc_stress(&mut self, on: bool) {
        self.gc_stress = on;
        self.heap.alloc_threshold = if on { 1 } else { GC_MIN_INTERVAL.max(self.heap.allocated_since_gc + 1) };
    }

    /// Records that the innermost frame is being removed while unwinding.
    #[inline]
    fn unwound(&mut self, by: UnwindBy) {
        if self.trace.is_some() {
            let i = self.ci.len() - 1;
            let mid = self.ci[i].mid;
            self.record(TraceEvent::FrameUnwound { frame: i, mid, by });
        }
    }

    /// Appends an event while `set_trace(true)` (the caller checks `trace.is_some()` first,
    /// so nothing is built when recording is off).
    #[inline]
    fn record(&mut self, e: TraceEvent) {
        if let Some(t) = &mut self.trace { t.push(e); }
    }

    /// The due collection, at an instruction boundary. Postponed (it stays due)
    /// while a native is on the host stack or `GC.disable` is in effect.
    #[cold]
    #[inline(never)]
    fn gc_maybe(&mut self) {
        if self.native_active == 0 && !self.gc_disabled { self.gc_collect(); }
    }

    /// `GC.start`. Called from a SEND, the native `GC.start` is the only one on the
    /// host stack and every register is in the Vm, so it collects at once; under
    /// another native (`funcall`) it only makes the collection due.
    pub fn gc_start(&mut self) {
        if self.gc_disabled { return; }
        if self.native_active <= 1 { self.gc_collect(); } else { self.heap.gc_pending = true; }
    }

    /// Mark & sweep (stop the world). The caller guarantees that no Rust frame
    /// holds a value that is not reachable from the roots.
    pub fn gc_collect(&mut self) {
        let t0 = self.gc_clock.map(|c| c());
        let mut work: Vec<ObjId> = Vec::new();
        let mut ctxs: Vec<usize> = Vec::new();
        let mut windows: Vec<(usize, usize, usize)> = Vec::new();
        let mut ctx_marked = vec![false; self.contexts.len()];
        self.gc_mark_roots(&mut work, &mut ctxs);
        loop {
            self.heap.mark_drain(&mut work, &mut ctxs, &mut windows);
            if let Some(c) = ctxs.pop() {
                if !ctx_marked[c] {
                    ctx_marked[c] = true;
                    self.gc_mark_context(c, &mut work);
                }
            } else if let Some((c, base, len)) = windows.pop() {
                if !ctx_marked[c] {
                    let st = if c == self.cur { &self.stack } else { &self.contexts[c].stack };
                    let end = (base + len).min(st.len());
                    if base < end { self.heap.mark_slots(&st[base..end], &mut work); }
                }
            } else {
                break;
            }
        }
        // The scheduler's dormant queue is weak (`Vm::gc_mark_roots`): a task that finished and
        // that nothing else names is garbage, and dropping it here, with the marking done and
        // before the sweep, is what keeps a host that restarts scripts from paying a Task object
        // (with its result, its name and its queue) per restart for the life of the VM. One that
        // something still holds — a local, a `gc_register`ed handle, another task joining it —
        // was marked and stays, so `Task.list`, `Task#status` and `Task#value` answer for every
        // task a program can still reach, as the reference's do.
        {
            let heap = &self.heap;
            self.task.queues[0].retain(|o| heap.is_marked(*o));
        }
        // A context nothing reached (its Fiber object is garbage) can never run
        // again. The environments of its frames that are still reachable (a
        // block captured there) take their values off the stack first (mruby
        // `mrb_env_detach_all` in the sweep), then the stack and frames go.
        // The index is not reused.
        let mut detached: Vec<(ObjId, usize)> = Vec::new();
        for c in 0..self.contexts.len() {
            if ctx_marked[c] { continue; }
            let ctx = &self.contexts[c];
            if ctx.status == FiberState::Terminated && ctx.stack.is_empty() && ctx.ci.is_empty() && ctx.fib.is_none() && ctx.proc_.is_none() { continue; }
            for f in &ctx.ci {
                let Some(e) = f.env else { continue };
                if !self.heap.is_marked(e) { continue; }
                let (attached, base, len) = { let ed = self.heap.env(e); (ed.attached, ed.base, ed.len) };
                if !attached { continue; }
                let end = (base + len).min(ctx.stack.len());
                let vals = if base < end { ctx.stack[base..end].to_vec() } else { Vec::new() };
                let ed = self.heap.env_mut(e);
                ed.values = vals;
                ed.attached = false;
                detached.push((e, len));
            }
            self.contexts[c] = Context::new(FiberState::Terminated);
        }
        if self.trace.is_some() {
            for (env, len) in detached { self.record(TraceEvent::EnvDetach { env, len, reason: DetachReason::ContextSwept }); }
        }
        let (before_live, allocated_since) = (self.heap.live_count(), self.heap.allocated_since_gc);
        let swept = self.heap.sweep();
        let live = self.heap.live_count();
        if self.trace.is_some() { self.record(TraceEvent::GcCollect { before_live, after_live: live, swept, allocated_since }); }
        self.live_after_gc = live;
        self.heap.allocated_since_gc = 0;
        self.heap.malloc_increase = 0;
        self.heap.gc_pending = false;
        // `interval_ratio` 200 (the default) lets the heap double: `live` more allocations
        let ratio = self.gc_interval_ratio.max(100) as usize;
        self.heap.alloc_threshold = if self.gc_stress { 1 } else { (live * (ratio - 100) / 100).max(GC_MIN_INTERVAL) };
        self.gc_count += 1;
        if let (Some(t0), Some(c)) = (t0, self.gc_clock) { self.gc_time_ns += c().saturating_sub(t0); }
        // Last, with the collection over and the VM whole again: the host's free hook. It is
        // called here rather than from the sweep so that it never runs while the heap is half
        // rebuilt, and it is handed numbers rather than a `&mut Vm` so that it cannot re-enter
        // (`Vm::set_on_free`). The list is drained even with no hook set, so that a hook set
        // later does not hear about objects freed before it.
        if !self.heap.freed_data.is_empty() {
            let freed = core::mem::take(&mut self.heap.freed_data);
            // A store of the VM's own (`crate::host_store`) drops what the handle named first:
            // the host's hook is then told about an object whose value is already gone, which
            // is the order a hook that keeps a table of its own would want.
            if !self.host_stores.is_empty() {
                for (tag, handle) in freed.iter().copied() { self.host_store_free(tag, handle); }
            }
            if let Some(hook) = &self.on_free {
                for (tag, handle) in freed { hook(tag, handle); }
            }
        }
    }

    /// The root set (see `docs/design/gc.md`). Contexts to scan go to `ctxs`.
    fn gc_mark_roots(&mut self, work: &mut Vec<ObjId>, ctxs: &mut Vec<usize>) {
        let h = &mut self.heap;
        // the running context: its stack and frames are the Vm's
        ctxs.push(self.cur);
        // the root context, and the chain of contexts waiting for a resumed fiber
        ctxs.push(ROOT);
        for (i, c) in self.contexts.iter().enumerate() {
            if matches!(c.status, FiberState::Running | FiberState::Resumed) || c.vmexec { ctxs.push(i); }
        }
        for s in self.globals.values() { h.mark_value(s.get(), work); }
        for v in [self.exc, self.loop_exit, self.pending_kw].into_iter().flatten() { h.mark_value(v, work); }
        for id in self.core.ids() { h.mark_id(id, work); }
        h.mark_id(self.top_self, work);
        h.mark_id(self.call_proc, work);
        h.mark_id(self.ret_proc, work);
        h.mark_id(self.loop_proc, work);
        for id in &self.inspect_guard { h.mark_id(*id, work); }
        for (x, y) in &self.eq_guard { h.mark_id(*x, work); h.mark_id(*y, work); }
        for id in &self.gc_registered { h.mark_id(*id, work); }
        // the scheduler's queues own their tasks: one the program dropped is still going to run
        // (`mrb_task_mark_all`). The dormant queue is the exception — a finished task is going
        // to run no more, so it is held weakly and dropped from the queue once the collection
        // finds nothing else naming it (`Vm::gc_collect`, `docs/design/gems.md`).
        for q in &self.task.queues[1..] {
            for id in q { h.mark_id(*id, work); }
        }
        for t in [self.task.running, self.task.main].into_iter().flatten() { h.mark_id(t, work); }
    }

    /// Marks what a context holds: registers, frames, its Fiber and block.
    fn gc_mark_context(&mut self, c: usize, work: &mut Vec<ObjId>) {
        fn mark_ci(h: &mut Heap, ci: &[CallInfo], work: &mut Vec<ObjId>) {
            for f in ci {
                h.mark_id(f.proc_, work);
                h.mark_id(f.target_class, work);
                if let Some(e) = f.env { h.mark_id(e, work); }
            }
        }
        // Registers are roots up to the end of the top frame's window, as mruby's
        // `mark_context_stack` marks `ci->stack + nregs`: the size the push gave the frame
        // (its irep's nregs, at least the self/arguments/keywords/block slots, at least 4),
        // every slot of which the push cleared or filled. What lies above is left over
        // from returned frames and may name objects freed long ago; it holds nothing the
        // program can reach (`ObjectSpace.count_objects` sees the freed objects as the
        // reference does).
        fn live_end(ireps: &[VmIrep], ci: &[CallInfo], len: usize) -> usize {
            match ci.last() {
                Some(f) => {
                    let nregs = ireps.get(f.irep).map(|ir| ir.nregs).unwrap_or(0);
                    let npos = if f.n == 15 { 1 } else { f.n as usize };
                    let used = npos + usize::from(f.kw) + 2;
                    (f.base + nregs.max(used).max(4)).min(len)
                }
                None => len,
            }
        }
        // The slots above are cleared, as `mark_context_stack` does: when the frame returns
        // its caller's window covers them again, and a stale reference there must not name
        // an object this collection frees.
        let h = &mut self.heap;
        if c == self.cur {
            let end = live_end(&self.ireps, &self.ci, self.stack.len());
            h.mark_slots(&self.stack[..end], work);
            mark_ci(h, &self.ci, work);
            for s in &mut self.stack[end..] { *s = Slot::NIL; }
        }
        let ctx = &mut self.contexts[c];
        let end = live_end(&self.ireps, &ctx.ci, ctx.stack.len());
        h.mark_slots(&ctx.stack[..end], work);
        mark_ci(h, &ctx.ci, work);
        for s in &mut ctx.stack[end..] { *s = Slot::NIL; }
        for id in [ctx.fib, ctx.proc_].into_iter().flatten() { h.mark_id(id, work); }
    }

    // ------------------------------------------------------------------ environments

    /// The register stack of a context: the running one is in `self.stack`.
    fn stack_of(&self, ctx: usize) -> &Vec<Slot> {
        if ctx == self.cur { &self.stack } else { &self.contexts[ctx].stack }
    }
    fn stack_of_mut(&mut self, ctx: usize) -> &mut Vec<Slot> {
        if ctx == self.cur { &mut self.stack } else { &mut self.contexts[ctx].stack }
    }
    fn env_get(&self, env: ObjId, idx: usize) -> Value {
        let e = self.heap.env(env);
        if e.attached { self.stack_of(e.ctx).get(e.base + idx).map(|s| s.get()).unwrap_or(Value::Nil) } else { e.values.get(idx).map(|s| s.get()).unwrap_or(Value::Nil) }
    }
    fn env_set(&mut self, env: ObjId, idx: usize, v: Value) {
        let (attached, base, ctx) = { let e = self.heap.env(env); (e.attached, e.base, e.ctx) };
        if attached {
            let st = self.stack_of_mut(ctx);
            if base + idx < st.len() { st[base + idx] = Slot::from(v); }
        } else {
            let e = self.heap.env_mut(env);
            if idx < e.values.len() { e.values[idx] = Slot::from(v); }
        }
    }
    /// Takes the values of every environment of `ctx`'s frames off its stack, so the context can
    /// be dropped while a block written in it lives on (`mrb_env_detach_all`, which the sweep of
    /// an unreachable context does too). A task the scheduler closes or terminates goes this way.
    pub(crate) fn detach_context_envs(&mut self, ctx: usize) {
        if ctx == self.cur || ctx >= self.contexts.len() { return; }
        let envs: Vec<ObjId> = self.contexts[ctx].ci.iter().filter_map(|f| f.env).collect();
        for e in envs {
            let (attached, base, len) = { let ed = self.heap.env(e); (ed.attached, ed.base, ed.len) };
            if !attached { continue; }
            let end = (base + len).min(self.contexts[ctx].stack.len());
            let vals = if base < end { self.contexts[ctx].stack[base..end].to_vec() } else { Vec::new() };
            let ed = self.heap.env_mut(e);
            ed.values = vals;
            ed.attached = false;
        }
    }

    /// Reads a slot of an environment whether it is still on the stack or detached.
    pub fn env_value(&self, env: ObjId, idx: usize) -> Value { self.env_get(env, idx) }
    pub fn cvar_class_of(&self, proc_: ObjId) -> ObjId { self.cvar_class(proc_) }
    pub fn cvar_lookup(&self, class: ObjId, s: Sym) -> Option<Value> { self.cvar_get(class, s) }
    /// Constant lookup in a frame's lexical scope without raising.
    pub fn const_lookup_noraise(&self, ci: &CallInfo, s: Sym) -> Option<Value> {
        if let Some(v) = self.const_get(ci.target_class, s) { return Some(v); }
        let mut p = Some(ci.proc_);
        while let Some(pid) = p {
            let pd = self.heap.proc_data(pid);
            if let Some(tc) = pd.target_class { if let Some(v) = self.const_get(tc, s) { return Some(v); } }
            p = pd.upper;
        }
        self.const_get(self.core.object, s)
    }
    /// `uvenv`: the environment `up` procs above the current one.
    fn uvenv(&self, up: usize) -> Option<ObjId> {
        let ci = self.ci.last()?;
        let mut p = ci.proc_;
        for _ in 0..up {
            p = self.heap.proc_data(p).upper?;
        }
        self.heap.proc_data(p).env
    }
    /// Ensures the current frame has an environment (mruby `closure_setup`).
    fn frame_env(&mut self) -> ObjId {
        let i = self.ci.len() - 1;
        self.frame_env_at(i)
    }

    /// The environment of frame `i`, made now if it has none.
    fn frame_env_at(&mut self, i: usize) -> ObjId {
        if let Some(e) = self.ci[i].env {
            return e;
        }
        let ci = &self.ci[i];
        let len = self.ireps[ci.irep].nlocals;
        let bidx = Self::frame_bidx(ci);
        let e = self.heap.alloc(self.core.object, ObjKind::Env(EnvData {
            ctx: self.cur, base: ci.base, len, bidx, attached: true, values: Vec::new(), mid: ci.mid, target_class: Some(ci.target_class),
            vis: ci.vis, modfunc: ci.modfunc, vis_break: ci.vis_break, svar: None, svar_fwd: None,
        }));
        self.ci[i].env = Some(e);
        if self.trace.is_some() {
            let (ctx, base, mid) = (self.cur, self.ci[i].base, self.ci[i].mid);
            self.record(TraceEvent::EnvCreate { env: e, ctx, frame: i, base, len, mid });
        }
        e
    }
    /// The scope `$~` belongs to, as the environment that holds it (`svar_owner`): the running
    /// frame's own where it is a method or a class body, and otherwise the scope the block was
    /// written in, which the chain of `upper` procs leads to. A native has no frame of its own
    /// here, as a C frame has no slot of its own there, so the walk starts at the caller.
    /// `create` says a scope frame with no environment yet gets one, which only a write needs.
    fn svar_env(&mut self, create: bool) -> Option<ObjId> {
        // the walk may cross into the context a scope still stands on, while the root redirect
        // below stays the running context's
        let mut ctx = self.cur;
        let mut i = self.ci.len().checked_sub(1)?;
        let mut root_redirect = false;
        // the bottom frame of the context is never examined: what nothing above it claims is its
        // own, which is how a fiber keeps its special variables to itself
        while i > 0 {
            let p = self.ci_of(ctx)[i].proc_;
            if self.heap.proc_data(p).scope {
                return if create { Some(self.frame_env_at_ctx(ctx, i)) } else { self.ci_of(ctx)[i].env };
            }
            let Some((env, scopeless)) = self.scope_env_of(p) else {
                // a frame with no scope of its own is as transparent as a native one, and reads
                // and writes pass through to the scope below (`svar_scopeless_frame_p`)
                i -= 1;
                continue;
            };
            // a forward already says where a frame with no scope of its own sent its special
            // variables, so following one settles the descent below
            let fwd = self.follow_svar_fwd(env);
            let scopeless = scopeless && fwd == env;
            let env = fwd;
            // the scope the running context's own root block was written in resolves to the
            // context's own bottom frame (CRuby's root-lep redirect)
            let root = self.ci[0].proc_;
            if !self.heap.proc_data(root).scope
                && self.scope_env_of(root).map(|(e, _)| self.follow_svar_fwd(e)) == Some(env)
            {
                root_redirect = true;
                break;
            }
            // where that scope is itself a frame with no scope of its own, the walk goes on
            // below it, on whichever context that frame still stands
            if scopeless {
                let ectx = self.heap.env(env).ctx;
                if let Some(s) = self.frame_of_env(ectx, env) {
                    if s > 0 { ctx = ectx; i = s - 1; continue; }
                }
            }
            return Some(env);
        }
        if root_redirect { ctx = self.cur; }
        if create { Some(self.frame_env_at_ctx(ctx, 0)) } else { self.ci_of(ctx)[0].env }
    }

    /// The frames of a context, the running one's being held in `self.ci`.
    fn ci_of(&self, ctx: usize) -> &[CallInfo] {
        if ctx == self.cur { &self.ci } else { &self.contexts[ctx].ci }
    }

    /// The env of frame `i` of `ctx`, made now if it has none.
    fn frame_env_at_ctx(&mut self, ctx: usize, i: usize) -> ObjId {
        if ctx == self.cur { return self.frame_env_at(i); }
        if let Some(e) = self.contexts[ctx].ci[i].env { return e; }
        let ci = &self.contexts[ctx].ci[i];
        let (irep, base, mid, tc) = (ci.irep, ci.base, ci.mid, ci.target_class);
        let (vis, modfunc, vis_break) = (ci.vis, ci.modfunc, ci.vis_break);
        let bidx = Self::frame_bidx(ci);
        let len = self.ireps[irep].nlocals;
        let e = self.heap.alloc(self.core.object, ObjKind::Env(EnvData {
            ctx, base, len, bidx, attached: true, values: Vec::new(), mid, target_class: Some(tc),
            vis, modfunc, vis_break, svar: None, svar_fwd: None,
        }));
        self.contexts[ctx].ci[i].env = Some(e);
        e
    }

    /// The env of the scope a block was written in (`svar_scope_env`): its own env, or the one
    /// the outermost enclosing block captured where the block sits inside others. The flag says
    /// that scope is itself a frame with no scope of its own, which the walk goes on below.
    fn scope_env_of(&self, p: ObjId) -> Option<(ObjId, bool)> {
        let mut pd = self.heap.proc_data(p);
        let mut env = pd.env?;
        loop {
            let Some(up) = pd.upper else { return Some((env, false)) };
            let upd = self.heap.proc_data(up);
            if upd.scope { return Some((env, false)); }
            let Some(upenv) = upd.env else { return Some((env, true)) };
            env = upenv;
            pd = upd;
        }
    }

    /// Where an env's special variables have been sent on, following the chain a nested load
    /// leaves behind (one hop per load).
    fn follow_svar_fwd(&self, mut e: ObjId) -> ObjId {
        for _ in 0..64 {
            match self.heap.env(e).svar_fwd { Some(f) => e = f, None => break }
        }
        e
    }

    /// The frame of `ctx` that `env` belongs to, where it is still on that context's stack.
    fn frame_of_env(&self, ctx: usize, env: ObjId) -> Option<usize> {
        self.ci_of(ctx).iter().rposition(|c| c.env == Some(env))
    }

    /// One special variable as the scope owning it holds it (`mrb_vm_svar_get`). A read makes
    /// no container: a scope that was never written to answers nil.
    pub fn svar_get_key(&mut self, key: usize) -> Value {
        match self.svar_env(false) {
            Some(e) => match &self.heap.env(e).svar {
                Some(slots) => slots[key].get(),
                None => Value::Nil,
            },
            None => Value::Nil,
        }
    }

    /// Publishes one special variable into the scope owning it (`mrb_vm_svar_set`). A nil write
    /// into a scope that holds no container leaves it without one, which is the reference's lazy
    /// allocation (`svar_slot_ensure`).
    pub fn svar_set_key(&mut self, key: usize, v: Value) {
        if v.is_nil() {
            let has = self.svar_env(false).map(|e| self.heap.env(e).svar.is_some()).unwrap_or(false);
            if !has { return; }
        }
        if let Some(e) = self.svar_env(true) {
            if let ObjKind::Env(ed) = &mut self.heap.get_mut(e).kind {
                let slots = ed.svar.get_or_insert([Slot::from(Value::Nil); crate::object::SVAR_KEYS]);
                slots[key] = Slot::from(v);
            }
        }
    }

    /// `$~` as the scope owning it holds it (`mrb_vm_svar_get`).
    pub fn svar_get(&mut self) -> Value { self.svar_get_key(crate::object::SVAR_BACKREF) }

    /// Publishes `$~` into the scope owning it (`mrb_vm_svar_set`).
    pub fn svar_set(&mut self, v: Value) { self.svar_set_key(crate::object::SVAR_BACKREF, v) }

    /// Whether the frame that called the running native carries a special-variable container,
    /// which is the only Ruby-visible face the lazy allocation has (`svar_container_p` of the
    /// gem's `test/backref_scope.c`).
    pub fn svar_container_p(&self) -> bool {
        match self.ci.last().and_then(|ci| ci.env) {
            Some(e) => self.heap.env(e).svar.is_some(),
            None => false,
        }
    }

    /// Whether the env a proc closed over carries the container, and what sits in the slot
    /// (`__env_svar?` / `__env_svar_slot` of `mrbgems/mruby-test/env.c`).
    pub fn proc_env_svar(&self, p: ObjId) -> Option<bool> {
        let env = self.heap.proc_data(p).env?;
        Some(self.heap.env(env).svar.is_some())
    }

    /// The environment of the frame that called the running native, made now if it has none
    /// (`mrb_vm_ci_env` / `mrb_env_new` of the reference's `create_proc_from_string`).
    /// A native has no frame of its own here, so that is the current frame.
    pub(crate) fn caller_env(&mut self) -> ObjId {
        self.frame_env()
    }

    /// Runs an `eval` string's Proc in the caller's scope (`eval_irep`): no arguments, no
    /// block, visibility back to the default, and the target class the caller's unless the
    /// form says otherwise (`instance_eval`, `class_eval`).
    ///
    /// Called by a SEND the string runs in a frame of its own, as the reference's `eval_irep`
    /// hands it to `mrb_exec_irep` ([`Vm::exec_proc`]); the natives that call this return its
    /// value as it is.
    pub(crate) fn run_eval(&mut self, proc_: ObjId, self_: Value, override_tc: Option<ObjId>) -> VmResult<Value> {
        let mid = self.ci.last().and_then(|ci| ci.mid);
        let tc = override_tc.or_else(|| self.heap.proc_data(proc_).target_class);
        self.exec_proc(proc_, self_, &[], None, Value::Nil, mid, tc, true)
    }

    /// [`Vm::run_eval`] in a nested loop whoever calls it (a host's `mrb_load_string`).
    pub(crate) fn run_eval_nested(&mut self, proc_: ObjId, self_: Value, override_tc: Option<ObjId>) -> VmResult<Value> {
        let direct = core::mem::replace(&mut self.direct_send, false);
        let r = self.run_eval(proc_, self_, override_tc);
        self.direct_send = direct;
        r
    }

    /// Pops the current frame, detaching its environment (`cipop`).
    fn pop_frame(&mut self) -> CallInfo {
        let ci = self.ci.pop().expect("pop on empty callinfo");
        // A block created by the caller and passed to this frame loses its home
        // when the frame returns (mruby `cipop`: MRB_PROC_ORPHAN).
        let bidx = ci.base + Self::frame_bidx(&ci);
        if let Some(Value::Obj(b)) = self.stack.get(bidx).map(|s| s.get()) {
            if let ObjKind::Proc(pd) = &self.heap.get(b).kind {
                let caller_env = self.ci.last().and_then(|c| c.env);
                if !pd.strict && pd.env.is_some() && pd.env == caller_env {
                    if let ObjKind::Proc(pd) = &mut self.heap.get_mut(b).kind { pd.orphan = true; }
                }
            }
        }
        if let Some(e) = ci.env {
            let (base, len) = { let ed = self.heap.env(e); (ed.base, ed.len) };
            let end = (base + len).min(self.stack.len());
            let vals = self.stack[base..end].to_vec();
            let ed = self.heap.env_mut(e);
            ed.values = vals;
            ed.attached = false;
            if self.trace.is_some() { self.record(TraceEvent::EnvDetach { env: e, len, reason: DetachReason::FrameReturn }); }
        }
        // A frame with no scope of its own stays transparent after it returns: its escaped env
        // sends its special variables to the scope below, so a proc written inside a nested load
        // still reads and writes the scope that load ran against (`svar_env_adopt_owner`).
        if let Some(e) = ci.env {
            let p = ci.proc_;
            if !self.heap.proc_data(p).scope && self.scope_env_of(p).is_none() {
                if let Some(owner) = self.svar_env(true) {
                    if owner != e {
                        if let ObjKind::Env(ed) = &mut self.heap.get_mut(e).kind { ed.svar_fwd = Some(owner); }
                    }
                }
            }
        }
        // Orphan blocks whose env belonged to this frame? (`MRB_PROC_ORPHAN`) — not tracked yet.
        ci
    }

    // ------------------------------------------------------------------ the loop

    /// Runs until the frame at index `stop_depth` of the current context returns, and returns its value.
    fn run_loop(&mut self, stop_depth: usize) -> VmResult<Value> {
        self.run_loop_ctx(self.cur, stop_depth)
    }

    /// Runs until frame `stop_depth` of context `lc` returns. Frames of other
    /// contexts the loop is switched into (non-native fiber resume/yield) never
    /// end the loop; only a fiber's base frame terminating does, and then the
    /// loop carries on in the previous context.
    pub(crate) fn run_loop_ctx(&mut self, lc: usize, stop_depth: usize) -> VmResult<Value> {
        // A native started this loop (or the host did): whatever runs in it until one of its own
        // SENDs calls a native is not "called by a SEND of the running frame", and the register
        // `native_ret_reg` names belongs to a frame below (`Vm::in_frame`). The inline natives of
        // the index opcodes read it too, so it is cleared here rather than at every call.
        let direct = core::mem::replace(&mut self.direct_send, false);
        let r = self.run_loop_inner(lc, stop_depth);
        self.direct_send = direct;
        r
    }

    fn run_loop_inner(&mut self, lc: usize, stop_depth: usize) -> VmResult<Value> {
        let mut pending: Option<VmResult<Value>> = None;
        loop {
            let r = match pending.take() { Some(r) => r, None => self.exec_frames(stop_depth, lc) };
            match r {
                Ok(v) => return Ok(v),
                Err(VmError::Raise(exc)) => {
                    if let Value::Obj(o) = exc {
                        if matches!(self.heap.get(o).kind, ObjKind::Exception) {
                            let k = self.intern("@__raised");
                            self.heap.ivar_set(o, k, Value::True);
                            self.keep_backtrace(o);
                        }
                    }
                    // Unwind: look for a catch handler in frames >= stop_depth.
                    if self.handle_raise(exc, stop_depth, lc) {
                        continue;
                    }
                    return Err(VmError::Raise(exc));
                }
                Err(VmError::Unimplemented(what)) => {
                    // Surface as a Ruby NotImplementedError so scripts (and the
                    // mruby test suite) can rescue it and continue.
                    let exc = self.exc_new(self.core.not_implemented_error, &format!("not implemented in SabiRuby: {what}"));
                    pending = Some(Err(VmError::Raise(exc)));
                    continue;
                }
                Err(VmError::Break(brk)) => {
                    // A return/break came back through native code: keep unwinding here.
                    if self.cur == lc && self.ci.len() <= stop_depth { return Err(VmError::Break(brk)); }
                    match self.resume_break(brk, stop_depth, lc) {
                        Ok(Some(v)) => return Ok(v),
                        Ok(None) => continue,
                        Err(e) => { pending = Some(Err(e)); continue; }
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn catch_find(&self, irep: IrepId, pc: usize, ensure_only: bool) -> Option<rite::CatchHandler> {
        let pc = pc as u32;
        self.ireps[irep].catch.iter().rev().find(|h| pc > h.begin && pc <= h.end && (!ensure_only || h.kind == CatchType::Ensure)).copied()
    }

    pub(crate) fn break_new(&mut self, tag: BreakTag, ci_index: usize, value: Value) -> ObjId {
        // Reuse a pending break object of the same tag (mruby `prepare_tagged_break`).
        if let Some(Value::Obj(o)) = self.exc {
            if let ObjKind::Break { tag: t, .. } = self.heap.get(o).kind { if t == tag { return o; } }
        }
        self.heap.alloc(self.core.object, ObjKind::Break { tag, ci_index, value })
    }

    /// Enters the ensure handler `h` of the top frame with `brk` pending.
    fn enter_ensure(&mut self, h: rite::CatchHandler, brk: ObjId) {
        let top = self.ci.len() - 1;
        self.exc = Some(Value::Obj(brk));
        let (base, nregs) = { let ci = &self.ci[top]; (ci.base, self.ireps[ci.irep].nregs) };
        if self.stack.len() < base + nregs { self.stack.resize(base + nregs, Slot::NIL); }
        self.ci[top].pc = h.target as usize;
    }

    /// Continues a pending non-local exit (`L_BREAK` dispatch by tag).
    fn resume_break(&mut self, brk: ObjId, stop_depth: usize, lc: usize) -> VmResult<Option<Value>> {
        let (tag, idx, value) = match self.heap.get(brk).kind { ObjKind::Break { tag, ci_index, value } => (tag, ci_index, value), _ => return Err(VmError::Internal("not a break object".into())) };
        self.exc = Some(Value::Obj(brk));
        match tag {
            BreakTag::Break => self.unwind_return(idx, value, stop_depth, lc, UnwindBy::Return),
            BreakTag::BlockBreak => self.unwind_return(idx, value, stop_depth, lc, UnwindBy::Break),
            BreakTag::Jump => { let target = match value { Value::Int(t) => t as usize, _ => 0 }; self.jmpuw(target); Ok(None) }
        }
    }

    /// `OP_JMPUW`: jump to `target`, running any ensure body in between first.
    fn jmpuw(&mut self, target: usize) {
        let top = self.ci.len() - 1;
        let (irep, pc) = { let ci = &self.ci[top]; (ci.irep, ci.pc) };
        if let Some(h) = self.catch_find(irep, pc, true) {
            // avoid jumping from a handler into the same handler
            if (target as u32) < h.begin || (target as u32) > h.end {
                let brk = self.break_new(BreakTag::Jump, top, Value::Int(target as i64));
                self.enter_ensure(h, brk);
                return;
            }
        }
        self.exc = None;
        self.ci[top].pc = target;
    }

    /// Unwinds to frame `return_idx` (running ensure bodies on the way, mruby
    /// `L_RETURN`/`UNWIND_ENSURE`) and returns `v` from it. `Ok(Some(v))` means
    /// the interpreter loop must hand `v` to its native caller.
    fn unwind_return(&mut self, return_idx: usize, v: Value, stop_depth: usize, lc: usize, by: UnwindBy) -> VmResult<Option<Value>> {
        loop {
            let top = self.ci.len() - 1;
            let (irep, pc) = { let ci = &self.ci[top]; (ci.irep, ci.pc) };
            let tag = if by == UnwindBy::Break { BreakTag::BlockBreak } else { BreakTag::Break };
            if let Some(h) = self.catch_find(irep, pc, true) {
                let brk = self.break_new(tag, return_idx, v);
                self.enter_ensure(h, brk);
                return Ok(None);
            }
            if top == return_idx { break; }
            self.unwound(by);
            let popped = self.pop_frame();
            if popped.cci == Cci::Skip || (self.cur == lc && top <= stop_depth) {
                // crossing a native frame: let the native caller propagate it
                let brk = self.break_new(tag, return_idx, v);
                self.exc = None;
                return Err(VmError::Break(brk));
            }
        }
        self.exc = None;
        let popped = self.pop_frame();
        if self.ci.is_empty() && self.cur != ROOT {
            // the fiber's block returned (mruby: `ci == cibase` → fiber_terminate)
            let vmexec = self.fiber_terminate();
            if vmexec { return Ok(Some(v)); }
            self.deliver(v);
            return Ok(None);
        }
        if popped.cci != Cci::None || (self.cur == lc && return_idx <= stop_depth) {
            // a frame that answers its receiver has it in R0 already, which is the caller's R[a]
            if popped.cci == Cci::KeepSelf && !(self.cur == lc && return_idx <= stop_depth) {
                if by == UnwindBy::Break { self.stack[popped.base] = Slot::from(v); }
                return Ok(None);
            }
            return Ok(Some(v));
        }
        // the callee's R0 is the caller's R[a]
        self.stack[popped.base] = Slot::from(v);
        Ok(None)
    }

    /// Finds a rescue/ensure handler for `exc`; on success sets pc and `exc`
    /// and returns true. Otherwise pops frames down to `stop_depth` and returns false.
    fn handle_raise(&mut self, exc: Value, stop_depth: usize, lc: usize) -> bool {
        if self.trace.is_some() {
            let i = self.ci.len() - 1;
            let (irep, pc) = { let ci = &self.ci[i]; (ci.irep, ci.pc) };
            let class = self.class_name(self.real_class_of(exc));
            let line = self.ireps[irep].line_of(pc);
            self.record(TraceEvent::Raise { exc: exc.obj(), class, frame: i, irep, pc, line });
        }
        loop {
            let i = self.ci.len() - 1;
            let (irep, pc) = { let ci = &self.ci[i]; (ci.irep, ci.pc) };
            if self.trace.is_some() {
                let matched = self.catch_find(irep, pc, false).map(|h| CatchHandlerInfo {
                    ensure: h.kind == CatchType::Ensure, begin: h.begin, end: h.end, target: h.target });
                let line = self.ireps[irep].line_of(pc);
                self.record(TraceEvent::CatchLook { frame: i, irep, pc, line, matched });
            }
            if let Some(h) = self.catch_find(irep, pc, false) {
                self.ci[i].pc = h.target as usize;
                let nregs = self.ireps[irep].nregs;
                let base = self.ci[i].base;
                if self.stack.len() < base + nregs { self.stack.resize(base + nregs, Slot::NIL); }
                self.exc = Some(exc);
                return true;
            }
            if i == 0 && self.cur != ROOT {
                // uncaught in a fiber: it terminates and the exception continues in
                // the context that resumed it (mruby `L_FTOP`); a fiber resumed by
                // native code hands it to that native instead
                self.pop_frame();
                let vmexec = self.fiber_terminate();
                if vmexec { return false; }
                continue;
            }
            if self.cur == lc && i <= stop_depth {
                self.unwound(UnwindBy::Raise);
                self.pop_frame();
                return false;
            }
            self.unwound(UnwindBy::Raise);
            self.pop_frame();
        }
    }

    #[inline]
    fn read_b(&self, irep: IrepId, pc: &mut usize) -> u32 {
        let v = self.ireps[irep].iseq[*pc] as u32;
        *pc += 1;
        v
    }
    #[inline]
    fn read_s(&self, irep: IrepId, pc: &mut usize) -> u32 {
        let s = &self.ireps[irep].iseq;
        let v = ((s[*pc] as u32) << 8) | s[*pc + 1] as u32;
        *pc += 2;
        v
    }
    #[inline]
    fn read_w(&self, irep: IrepId, pc: &mut usize) -> u32 {
        let s = &self.ireps[irep].iseq;
        let v = ((s[*pc] as u32) << 16) | ((s[*pc + 1] as u32) << 8) | s[*pc + 2] as u32;
        *pc += 3;
        v
    }

    fn exec_frames(&mut self, stop_depth: usize, lc: usize) -> VmResult<Value> {
        let mut ext: u8 = 0;
        loop {
            // Four things can happen before an instruction runs: the tick a running task is
            // counted by, a switch the scheduler has asked for, the step budget running out,
            // and a collection. None of them is true in a program that makes no task, sets no
            // step budget and has not filled its heap, but asking four times meant reading
            // four fields from four corners of `Vm` and branching on each, on every single
            // instruction -- and `&mut self` reaches the called code, so none of the four can
            // stay in a register. One test of the four conditions (`|`, not `||`: the loads
            // issue together and one branch is spent on the answer) leaves the four blocks
            // below to re-test their own condition, which costs nothing when something really
            // is pending. Worth 9-10% of `vmo_dispatch` and `vmo_arith`; the probe that
            // measured the ceiling is in `../worklog/2026-09-16-perf3.md`.
            //
            // `tick_every` is *not* one of the four: it is the tick length and is non-zero by
            // default, so testing it here would make the guard always true. What says that no
            // task is running is `task.running`.
            if self.task.running.is_some()
                | self.task.switching
                | self.step_left.is_some()
                | self.heap.gc_pending
            {
                // mruby-task: the tick a timer interrupt gives the reference is the instruction
                // count here, and a switch the tick asked for is taken at the next instruction
                // boundary of a task's own frame (`RETURN_IF_TASK_STOPPED`)
                if self.task.tick_every != 0 && self.task.running.is_some() {
                    self.task.tick_left = self.task.tick_left.saturating_sub(1);
                    if self.task.tick_left == 0 {
                        crate::builtins::ext_task::tick(self);
                        if self.task.overrun {
                            if let Some(e) = crate::builtins::ext_task::overrun(self) { return Err(e); }
                        }
                    }
                }
                if self.task.switching && self.cur != ROOT && self.exc.is_none()
                    && !self.fiber_check_native(self.cur) && self.contexts[self.cur].vmexec
                {
                    // the task is left suspended at this instruction, as a `Fiber.yield` leaves a
                    // fiber, and the scheduler's nested loop ends here
                    self.task.switching = false;
                    let c = self.cur;
                    let prev = self.contexts[c].prev.take().unwrap_or(ROOT);
                    self.contexts[c].status = FiberState::Suspended;
                    self.contexts[c].vmexec = false;
                    self.switch_context(prev, SwitchKind::Yield);
                    return Ok(Value::Nil);
                }
                if let Some(left) = self.step_left {
                    // Suspend only in the outermost loop: a nested loop (native code
                    // waiting for a block) must run to completion, so the pause lands
                    // at the next instruction boundary of the top-level program.
                    if left == 0 && stop_depth == 0 && lc == ROOT {
                        return Ok(Value::Nil);
                    }
                    self.step_left = Some(left.saturating_sub(1));
                }
                // the only place the collector runs: every register and frame is in the Vm
                if self.heap.gc_pending { self.gc_maybe(); }
            }
            self.instructions += 1;
            // The frame is read field by field, not copied: `base`, `irep` and `pc` are what
            // every instruction needs, and the handful of instructions that want the rest
            // (`proc_`, `target_class`, `mid`) read `self.ci[top]` where they stand.
            let top = self.ci.len() - 1;
            let (base, irep, mut pc) = { let ci = &self.ci[top]; (ci.base, ci.irep, ci.pc) };
            let byte = self.ireps[irep].iseq[pc];
            let op = Op::from_u8(byte).ok_or_else(|| VmError::Internal(format!("bad opcode {byte}")))?;
            if self.count_ops { self.op_counts[op as usize] += 1; }
            pc += 1;
            let (mut a, mut b, mut c) = (0u32, 0u32, 0u32);
            let a_wide = ext == 1 || ext == 3;
            let b_wide = ext == 2 || ext == 3;
            match op.operands() {
                Operands::Z => {}
                Operands::B => a = if a_wide { self.read_s(irep, &mut pc) } else { self.read_b(irep, &mut pc) },
                Operands::BB => { a = if a_wide { self.read_s(irep, &mut pc) } else { self.read_b(irep, &mut pc) }; b = if b_wide { self.read_s(irep, &mut pc) } else { self.read_b(irep, &mut pc) }; }
                Operands::BBB => { a = if a_wide { self.read_s(irep, &mut pc) } else { self.read_b(irep, &mut pc) }; b = if b_wide { self.read_s(irep, &mut pc) } else { self.read_b(irep, &mut pc) }; c = self.read_b(irep, &mut pc); }
                Operands::BS => { a = if a_wide { self.read_s(irep, &mut pc) } else { self.read_b(irep, &mut pc) }; b = self.read_s(irep, &mut pc); }
                Operands::BSS => { a = if a_wide { self.read_s(irep, &mut pc) } else { self.read_b(irep, &mut pc) }; b = self.read_s(irep, &mut pc); c = self.read_s(irep, &mut pc); }
                Operands::S => a = self.read_s(irep, &mut pc),
                Operands::W => a = self.read_w(irep, &mut pc),
            }
            ext = 0;
            // pc now points at the next instruction (like mruby's DECODE_OPERANDS).
            self.ci[top].pc = pc;
            let (a, b, c) = (a as usize, b as usize, c as usize);
            macro_rules! reg { ($i:expr) => { self.stack[base + $i].get() } }
            macro_rules! setreg { ($i:expr, $v:expr) => { { let v = $v; self.stack[base + $i] = Slot::from(v); } } }
            // `OP_ADD` and its relatives answer two numbers where they stand, as mruby's
            // `OP_MATH` does (`TYPES2(MRB_TT_INTEGER, MRB_TT_INTEGER)` and the three float
            // cases); anything else, and an Integer result that leaves the range, goes to
            // `op_arith`. Reaching `op_arith` for every one of them meant asking which
            // operation this was by comparing `mid` against `plus`, `minus` and `mul` in turn
            // -- the opcode already says which, so those comparisons re-derived what the
            // dispatch had just decided.
            macro_rules! arith { ($int:ident, $f:tt, $mid:expr) => { {
                let r = match (self.stack[base + a].get(), self.stack[base + a + 1].get()) {
                    (Value::Int(p), Value::Int(q)) => p.$int(q).map(Value::Int),
                    (Value::Float(p), Value::Float(q)) => Some(Value::Float(p $f q)),
                    (Value::Int(p), Value::Float(q)) => Some(Value::Float(p as f64 $f q)),
                    (Value::Float(p), Value::Int(q)) => Some(Value::Float(p $f q as f64)),
                    _ => None,
                };
                match r { Some(v) => self.stack[base + a] = Slot::from(v), None => self.op_arith(base, a, $mid)? }
            } } }
            // The same for the five comparisons, for the two pairs that need no conversion.
            // A mixed Integer/Float pair stays in `op_compare`, which decides it exactly
            // (`int_float_cmp`) rather than by widening the Integer to `f64`.
            macro_rules! cmp { ($f:tt, $mid:expr) => { {
                match (self.stack[base + a].get(), self.stack[base + a + 1].get()) {
                    (Value::Int(p), Value::Int(q)) => self.stack[base + a] = Slot::from(Value::bool(p $f q)),
                    (Value::Float(p), Value::Float(q)) => self.stack[base + a] = Slot::from(Value::bool(p $f q)),
                    _ => self.op_compare(base, a, $mid)?,
                }
            } } }
            match op {
                Op::Nop => {}
                Op::Move => { setreg!(a, reg!(b)); }
                Op::Loadl => {
                    let v = match &self.ireps[irep].pool[b] {
                        Pool::Int(i) => Value::Int(*i),
                        Pool::Float(f) => Value::Float(*f),
                        Pool::Str(s) => { let s = s.clone(); self.str_new(&s) }
                        Pool::BigInt { base, digits } => {
                            // `mrb_bint_new_str`: a negative base means a negative number
                            let (neg, b) = (*base < 0, base.unsigned_abs() as u32);
                            let d = digits.clone();
                            match crate::bigint::BigInt::from_str(&d, b) {
                                Some(v) => self.bint_value(if neg { v.neg() } else { v }),
                                None => return Err(VmError::Rite("bad bigint literal".into())),
                            }
                        }
                    };
                    setreg!(a, v);
                }
                Op::Loadi8 => { setreg!(a, Value::Int(b as i64)); }
                Op::Loadineg => { setreg!(a, Value::Int(-(b as i64))); }
                Op::LoadiM1 => { setreg!(a, Value::Int(-1)); }
                Op::Loadi0 => { setreg!(a, Value::Int(0)); }
                Op::Loadi1 => { setreg!(a, Value::Int(1)); }
                Op::Loadi2 => { setreg!(a, Value::Int(2)); }
                Op::Loadi3 => { setreg!(a, Value::Int(3)); }
                Op::Loadi4 => { setreg!(a, Value::Int(4)); }
                Op::Loadi5 => { setreg!(a, Value::Int(5)); }
                Op::Loadi6 => { setreg!(a, Value::Int(6)); }
                Op::Loadi7 => { setreg!(a, Value::Int(7)); }
                Op::Loadi16 => { setreg!(a, Value::Int(b as u16 as i16 as i64)); }
                Op::Loadi32 => { setreg!(a, Value::Int((((b as u32) << 16) | c as u32) as i32 as i64)); }
                Op::Loadsym => { setreg!(a, Value::Sym(self.ireps[irep].syms[b])); }
                Op::Loadnil => { setreg!(a, Value::Nil); }
                Op::Loadself => { setreg!(a, reg!(0)); }
                Op::Loadtrue => { setreg!(a, Value::True); }
                Op::Loadfalse => { setreg!(a, Value::False); }
                Op::Getsv | Op::Setsv => { return Err(VmError::Unimplemented("special variables ($~, $_)".into())); }
                // `$~` is a virtual global (mruby-regexp's `mrb_gv_define_virtual`): it is not
                // in the globals table but in the scope that owns it, so that a method's match
                // stays out of its caller's `$~`
                Op::Getgv => {
                    let s = self.ireps[irep].syms[b];
                    let v = if Some(s) == self.s.backref { self.svar_get() }
                        else { self.globals.get(&s).map(|s| s.get()).unwrap_or(Value::Nil) };
                    setreg!(a, v);
                }
                Op::Setgv => {
                    let s = self.ireps[irep].syms[b];
                    let v = reg!(a);
                    if Some(s) == self.s.backref {
                        // the one place an arbitrary value reaches the slot (`backref_gv_set`)
                        if !v.is_nil() && self.class_of(v) != self.core.match_data {
                            let d = self.describe_for_error(v);
                            return Err(self.raise_type(&format!("wrong argument type {d} (expected MatchData)")));
                        }
                        self.svar_set(v);
                    } else {
                        self.globals.insert(s, Slot::from(v));
                    }
                }
                Op::Getiv => {
                    let s = self.ireps[irep].syms[b];
                    let v = match reg!(0) { Value::Obj(o) => self.heap.ivar_get(o, s), _ => Value::Nil };
                    setreg!(a, v);
                }
                Op::Setiv => {
                    let s = self.ireps[irep].syms[b];
                    let v = reg!(a);
                    match reg!(0) {
                        Value::Obj(o) => { if self.heap.get(o).frozen { let r = reg!(0); return Err(self.frozen_error(r)); } self.heap.ivar_set(o, s, v) }
                        _ => return Err(self.raise_type("can't set instance variable on an immediate")),
                    }
                }
                Op::Getcv => {
                    let s = self.ireps[irep].syms[b];
                    let cls = self.cvar_class(self.ci[top].proc_);
                    let v = match self.cvar_get(cls, s) {
                        Some(v) => v,
                        None => { let n = self.sym_name(s); let cn = self.class_name(cls); return Err(self.raise(self.core.name_error, &format!("uninitialized class variable {n} in {cn}"))); }
                    };
                    setreg!(a, v);
                }
                Op::Setcv => {
                    let s = self.ireps[irep].syms[b];
                    let v = reg!(a);
                    let cls = self.cvar_class(self.ci[top].proc_);
                    self.cvar_set(cls, s, v)?;
                }
                Op::Getconst => {
                    let s = self.ireps[irep].syms[b];
                    let (tc, pr) = { let ci = &self.ci[top]; (ci.target_class, ci.proc_) };
                    let v = self.const_lookup(tc, pr, s)?;
                    setreg!(a, v);
                }
                Op::Setconst => {
                    let s = self.ireps[irep].syms[b];
                    let v = reg!(a);
                    let tc = self.ci[top].target_class;
                    if self.heap.is_class(tc) {
                        if self.heap.get(tc).frozen { return Err(self.frozen_error(Value::Obj(tc))); }
                        self.heap.class_mut(tc).consts.insert(s, Slot::from(v));
                        // name anonymous classes on first assignment
                        if let Value::Obj(o) = v {
                            if self.heap.is_class(o) && self.heap.class(o).name.is_none() {
                                self.heap.class_mut(o).name = Some(s);
                                self.heap.class_mut(o).outer = Some(tc);
                            }
                        }
                        self.const_added(tc, s)?;
                    }
                }
                Op::Getmcnst => {
                    let s = self.ireps[irep].syms[b];
                    let base_v = reg!(a);
                    let cls = match base_v { Value::Obj(o) if self.heap.is_class(o) => o, _ => return Err(self.raise_type("not a class/module")) };
                    let v = match self.const_get(cls, s) {
                        Some(v) => v,
                        None => { let n = self.sym_name(s); let cn = self.class_name(cls); return Err(self.raise(self.core.name_error, &format!("uninitialized constant {cn}::{n}"))); }
                    };
                    setreg!(a, v);
                }
                Op::Setmcnst => {
                    let s = self.ireps[irep].syms[b];
                    let v = reg!(a);
                    match reg!(a + 1) {
                        Value::Obj(o) if self.heap.is_class(o) => {
                            self.heap.class_mut(o).consts.insert(s, Slot::from(v));
                            if let Value::Obj(c) = v { if self.heap.is_class(c) && self.heap.class(c).name.is_none() { self.heap.class_mut(c).name = Some(s); self.heap.class_mut(c).outer = Some(o); } }
                            self.const_added(o, s)?;
                        }
                        _ => return Err(self.raise_type("not a class/module")),
                    }
                }
                Op::Getupvar => {
                    let v = match self.uvenv(c) { Some(e) if b < self.heap.env(e).len => self.env_get(e, b), _ => Value::Nil };
                    setreg!(a, v);
                }
                Op::Setupvar => {
                    if let Some(e) = self.uvenv(c) {
                        if b < self.heap.env(e).len { let v = reg!(a); self.env_set(e, b, v); }
                    }
                }
                // The three index opcodes answer the common receivers themselves and *send*
                // the rest, in this frame, as the reference does (`L_SEND_SYM`): a `[]`
                // written in Ruby is then an ordinary frame, so it can `Fiber.yield` or park
                // a task on a queue out of it. `prepare_call` writes the nil block itself, so
                // the fallback only has to hand `op_send_vis` the registers it already reads.
                Op::Getidx => {
                    if self.op_getidx(base, a)? {
                        self.op_send_vis(base, a, self.s.aref, 1, false, false, false)?;
                        if let Some(v) = self.loop_exit.take() { return Ok(v); }
                    }
                }
                Op::Getidx0 => {
                    if self.op_getidx0(base, a, b)? {
                        self.op_send_vis(base, a, self.s.aref, 1, false, false, false)?;
                        if let Some(v) = self.loop_exit.take() { return Ok(v); }
                    }
                }
                Op::Setidx => {
                    if self.op_setidx(base, a)? {
                        self.op_send_vis(base, a, self.s.aset, 2, false, false, false)?;
                        if let Some(v) = self.loop_exit.take() { return Ok(v); }
                    }
                }
                Op::Jmp => { self.ci[top].pc = jump(pc, a); }
                Op::Jmpif => { if reg!(a).truthy() { self.ci[top].pc = jump(pc, b); } }
                Op::Jmpnot => { if !reg!(a).truthy() { self.ci[top].pc = jump(pc, b); } }
                Op::Jmpnil => { if reg!(a).is_nil() { self.ci[top].pc = jump(pc, b); } }
                Op::Jmpuw => { self.jmpuw(jump(pc, a)); }
                Op::Except => { setreg!(a, self.exc.take().unwrap_or(Value::Nil)); }
                Op::Rescue => {
                    let exc = reg!(a); let cls_v = reg!(b);
                    let cls = match cls_v { Value::Obj(o) if self.heap.is_class(o) => o, _ => return Err(self.raise_type("class or module required for rescue clause")) };
                    setreg!(b, Value::bool(self.obj_is_kind_of(exc, cls)));
                }
                Op::Raiseif => {
                    let exc = reg!(a);
                    match exc {
                        Value::Nil => { self.exc = None; }
                        Value::Obj(o) if matches!(self.heap.get(o).kind, ObjKind::Break { .. }) => {
                            if let Some(r) = self.resume_break(o, stop_depth, lc)? { return Ok(r); }
                        }
                        _ => return Err(VmError::Raise(exc)),
                    }
                }
                Op::Matcherr => { return Err(self.raise(self.core.no_matching_pattern_error, "pattern not matched")); }
                Op::Ssend | Op::Ssend0 | Op::Ssendb | Op::Send | Op::Send0 | Op::Sendb => {
                    let mid = self.ireps[irep].syms[b];
                    let argc = if matches!(op, Op::Send0 | Op::Ssend0) { 0 } else { c };
                    let has_blk = matches!(op, Op::Sendb | Op::Ssendb);
                    let explicit = matches!(op, Op::Send | Op::Send0 | Op::Sendb);
                    if !explicit { setreg!(a, reg!(0)); }
                    self.op_send_vis(base, a, mid, argc, has_blk, false, explicit)?;
                    if let Some(v) = self.loop_exit.take() { return Ok(v); }
                }
                Op::Super => {
                    let argc = b;
                    setreg!(a, reg!(0));
                    let mid = self.ci[top].mid.ok_or_else(|| self.raise(self.core.no_method_error, "super called outside of method"))?;
                    self.op_send(base, a, mid, argc, true, true)?;
                    if let Some(v) = self.loop_exit.take() { return Ok(v); }
                }
                Op::Call => {
                    // `Proc#call`: replace this frame (pushed by SEND) with the proc's body.
                    let p = match reg!(0) { Value::Obj(o) if matches!(self.heap.get(o).kind, ObjKind::Proc(_)) => o, _ => return Err(self.raise_type("wrong type (expected Proc)")) };
                    let n = self.ci[top].n as usize;
                    let nargs = (if n == 15 { 1 } else { n }) + (if self.ci[top].kw { 1 } else { 0 }) + 2;
                    self.vm_call_proc(p, nargs);
                }
                Op::Blkcall => {
                    // Direct block call: R[a] = R[a].call(R[a+1..a+b])
                    let p = match reg!(a) { Value::Obj(o) if matches!(self.heap.get(o).kind, ObjKind::Proc(_)) => o, _ => return Err(self.raise_type("wrong type (expected Proc)")) };
                    let nbase = base + a;
                    let (n, kw, _) = self.prepare_call(nbase, b, false)?;
                    let npos = if n == 15 { 1 } else { n };
                    self.ci.push(CallInfo { base: nbase, pc: 0, irep: 0, proc_: p, n: n as u8, kw, mid: None, target_class: self.ci[top].target_class, env: None, cci: Cci::None, vis: Vis::Public, modfunc: false, vis_break: false });
                    self.vm_call_proc(p, npos + (if kw { 1 } else { 0 }) + 2);
                }
                Op::Argary => { self.op_argary(base, a, b)?; }
                Op::Enter => { self.op_enter(a as u32)?; }
                Op::Karg => {
                    let k = Value::Sym(self.ireps[irep].syms[b]);
                    let v = match self.kidx_at(top).and_then(|ki| self.hash_delete(self.stack[ki].get(), k)) {
                        Some(v) => v,
                        None => { let n = self.sym_name(self.ireps[irep].syms[b]); return Err(self.raise_arg(&format!("missing keyword: {n}"))); }
                    };
                    setreg!(a, v);
                }
                Op::KeyP => {
                    let k = Value::Sym(self.ireps[irep].syms[b]);
                    let has = match self.kidx_at(top) { Some(ki) => self.hash_get(self.stack[ki].get(), k).is_some(), None => false };
                    setreg!(a, Value::bool(has));
                }
                Op::Keyend => {
                    if let Some(ki) = self.kidx_at(top) {
                        let first = match self.stack[ki].get().obj().map(|o| &self.heap.get(o).kind) { Some(ObjKind::Hash(hd)) => hd.entries().first().map(|e| e.0.get()), _ => None };
                        if let Some(k) = first { let d = match k { Value::Sym(s) => self.sym_name(s), v => self.inspect_str(v)? }; return Err(self.raise_arg(&format!("unknown keyword: {d}"))); }
                    }
                }
                Op::Return => { let v = reg!(a); if let Some(r) = self.op_return(v, stop_depth, lc)? { return Ok(r); } }
                Op::ReturnBlk => {
                    let v = reg!(a);
                    if let Some(r) = self.op_return_blk(v, stop_depth, lc)? { return Ok(r); }
                }
                Op::Retself => { let v = reg!(0); if let Some(r) = self.op_return(v, stop_depth, lc)? { return Ok(r); } }
                Op::Retnil => { if let Some(r) = self.op_return(Value::Nil, stop_depth, lc)? { return Ok(r); } }
                Op::Rettrue => { if let Some(r) = self.op_return(Value::True, stop_depth, lc)? { return Ok(r); } }
                Op::Retfalse => { if let Some(r) = self.op_return(Value::False, stop_depth, lc)? { return Ok(r); } }
                Op::Break => {
                    let v = reg!(a);
                    if let Some(r) = self.op_break(v, stop_depth, lc)? { return Ok(r); }
                }
                Op::Blkpush => { let v = self.op_blkpush(base, b)?; setreg!(a, v); }
                Op::Add => { arith!(checked_add, +, self.s.plus); }
                Op::Sub => { arith!(checked_sub, -, self.s.minus); }
                Op::Mul => { arith!(checked_mul, *, self.s.mul); }
                Op::Div => { self.op_arith(base, a, self.s.div)?; }
                Op::Addi => { setreg!(a + 1, Value::Int(b as i64)); arith!(checked_add, +, self.s.plus); }
                Op::Subi => { setreg!(a + 1, Value::Int(b as i64)); arith!(checked_sub, -, self.s.minus); }
                Op::Addilv | Op::Subilv => {
                    let mid = if matches!(op, Op::Addilv) { self.s.plus } else { self.s.minus };
                    match reg!(a) {
                        Value::Int(_) | Value::Float(_) => { setreg!(b, reg!(a)); setreg!(b + 1, Value::Int(c as i64)); self.op_arith(base, b, mid)?; let v = reg!(b); setreg!(a, v); }
                        recv => { let r = self.funcall(recv, mid, &[Value::Int(c as i64)], Value::Nil)?; setreg!(a, r); }
                    }
                }
                Op::Eq => { cmp!(==, self.s.eq); }
                Op::Lt => { cmp!(<, self.s.lt); }
                Op::Le => { cmp!(<=, self.s.le); }
                Op::Gt => { cmp!(>, self.s.gt); }
                Op::Ge => { cmp!(>=, self.s.ge); }
                Op::Array => { let v: Vec<Value> = values_of(&self.stack[base + a..base + a + b]); setreg!(a, self.ary_new(v)); }
                Op::Array2 => { let v: Vec<Value> = values_of(&self.stack[base + b..base + b + c]); setreg!(a, self.ary_new(v)); }
                Op::Arycat => {
                    // R[a] == nil means "start the argument accumulator": a fresh
                    // array (independent of R[a+1]) that later ARYPUSH/ARYCAT extend.
                    let (dst, src) = (reg!(a), reg!(a + 1));
                    let items = match src { Value::Nil => vec![], _ => self.to_array(src)? };
                    match dst {
                        Value::Nil => { setreg!(a, self.ary_new(items)); }
                        Value::Obj(o) if matches!(self.heap.get(o).kind, ObjKind::Array(_)) => { if let ObjKind::Array(v) = &mut self.heap.get_mut(o).kind { v.extend(slots_of(&items)); } }
                        _ => return Err(self.raise_type("not an array")),
                    }
                }
                Op::Arypush => {
                    let dst = reg!(a);
                    let items: Vec<Value> = values_of(&self.stack[base + a + 1..base + a + 1 + b]);
                    match dst { Value::Obj(o) => { if let ObjKind::Array(v) = &mut self.heap.get_mut(o).kind { v.extend(slots_of(&items)); } } _ => return Err(self.raise_type("not an array")) }
                }
                Op::Arysplat => { let v = reg!(a); let items = self.to_array(v)?; setreg!(a, self.ary_new(items)); }
                Op::Aref => {
                    let v = match reg!(b) { Value::Obj(o) => match self.heap.array(o) { Some(arr) => arr.get(c).map(|s| s.get()).unwrap_or(Value::Nil), None => reg!(b) }, other => if c == 0 { other } else { Value::Nil } };
                    setreg!(a, v);
                }
                Op::Aset => {
                    let v = reg!(a);
                    match reg!(b) { Value::Obj(o) => { if let ObjKind::Array(arr) = &mut self.heap.get_mut(o).kind { if arr.len() <= c { arr.resize(c + 1, Slot::NIL); } arr[c] = Slot::from(v); } } _ => return Err(self.raise_type("not an array")) }
                }
                Op::Apost => {
                    let src = reg!(a);
                    let items = match self.ary_vals(src) { Some(v) => v, None => vec![src] };
                    let pre = b; let post = c;
                    let len = items.len();
                    if len > pre + post {
                        let rest = items[pre..len - post].to_vec();
                        setreg!(a, self.ary_new(rest));
                        for i in 0..post { setreg!(a + 1 + i, items[len - post + i]); }
                    } else {
                        setreg!(a, self.ary_new(vec![]));
                        for i in 0..post { setreg!(a + 1 + i, items.get(pre + i).copied().unwrap_or(Value::Nil)); }
                    }
                }
                Op::Intern => { let v = reg!(a); let bytes = self.expect_str(v, "value")?; setreg!(a, Value::Sym(self.syms.intern(&bytes))); }
                Op::Symbol => {
                    let bytes = match &self.ireps[irep].pool[b] { Pool::Str(s) => s.clone(), _ => return Err(VmError::Internal("SYMBOL pool".into())) };
                    setreg!(a, Value::Sym(self.syms.intern(&bytes)));
                }
                Op::String => {
                    let bytes = match &self.ireps[irep].pool[b] { Pool::Str(s) => s.clone(), _ => return Err(VmError::Internal("STRING pool".into())) };
                    setreg!(a, self.str_new(&bytes));
                }
                Op::Strcat => {
                    let (dst, src) = (reg!(a), reg!(a + 1));
                    let bytes = self.as_string(src)?;
                    match dst { Value::Obj(o) => { if let ObjKind::String(s) = &mut self.heap.get_mut(o).kind { s.extend_from_slice(&bytes); } } _ => return Err(self.raise_type("not a string")) }
                }
                Op::Hash => {
                    let h = self.hash_new();
                    let pairs: Vec<(Value, Value)> = (0..b).map(|i| (self.stack[base + a + i * 2].get(), self.stack[base + a + i * 2 + 1].get())).collect();
                    for (k, v) in pairs { self.hash_set(h, k, v)?; }
                    setreg!(a, h);
                }
                Op::Hashadd => {
                    let h = reg!(a);
                    let pairs: Vec<(Value, Value)> = (0..b).map(|i| (self.stack[base + a + 1 + i * 2].get(), self.stack[base + a + 2 + i * 2].get())).collect();
                    for (k, v) in pairs { self.hash_set(h, k, v)?; }
                }
                Op::Hashcat => {
                    let (h, other) = (reg!(a), reg!(a + 1));
                    let entries = match other.obj().map(|o| &self.heap.get(o).kind) { Some(ObjKind::Hash(hd)) => hd.entries().to_vec(), _ => return Err(self.raise_type("not a hash")) };
                    for (k, v) in entries { self.hash_set(h, k.get(), v.get())?; }
                }
                Op::Lambda | Op::Block | Op::Method => {
                    let nirep = self.ireps[irep].reps[b];
                    let capture = !matches!(op, Op::Method);
                    let strict = matches!(op, Op::Lambda | Op::Method);
                    let env = if capture { Some(self.frame_env()) } else { None };
                    let p = self.heap.alloc(self.core.proc_, ObjKind::Proc(ProcData {
                        irep: nirep, upper: Some(self.ci[top].proc_), env, target_class: Some(self.ci[top].target_class), strict, scope: matches!(op, Op::Method), orphan: false, mid: None,
                    }));
                    setreg!(a, Value::Obj(p));
                }
                Op::RangeInc | Op::RangeExc => {
                    let (x, y) = (reg!(a), reg!(a + 1));
                    self.check_range_ends(x, y)?;
                    setreg!(a, self.range_new(x, y, matches!(op, Op::RangeExc)));
                }
                Op::Oclass => { setreg!(a, Value::Obj(self.core.object)); }
                Op::Class | Op::Module => {
                    let s = self.ireps[irep].syms[b];
                    let base_v = reg!(a);
                    let outer = match base_v { Value::Nil => self.heap.proc_data(self.ci[top].proc_).target_class.unwrap_or(self.core.object), Value::Obj(o) if self.heap.is_class(o) => o, _ => return Err(self.raise_type("not a class/module")) };
                    let existing = self.heap.class(outer).consts.get(&s).map(|s| s.get());
                    let is_module = matches!(op, Op::Module);
                    let given_sup = match if is_module { Value::Nil } else { reg!(a + 1) } {
                        Value::Nil => None,
                        Value::Obj(o) if self.heap.is_class(o) && !self.heap.class(o).is_module && !self.heap.class(o).is_singleton => Some(o),
                        other => { if is_module { None } else { let d = self.inspect_str(other)?; return Err(self.raise_type(&format!("superclass must be a Class ({d} given)"))); } }
                    };
                    let cls = match existing {
                        Some(Value::Obj(o)) if self.heap.is_class(o) && self.heap.class(o).is_module == is_module => {
                            if let Some(sup) = given_sup {
                                let real_sup = self.real_superclass(o);
                                if real_sup != Some(sup) { let n = self.sym_name(s); return Err(self.raise_type(&format!("superclass mismatch for Class {n}"))); }
                            }
                            o
                        }
                        Some(v) if !v.is_nil() => { let d = self.inspect_str(v)?; return Err(self.raise_type(&format!("{d} is not a {}", if is_module { "module" } else { "class" }))); }
                        _ => {
                            let sup = if is_module { None } else { Some(given_sup.unwrap_or(self.core.object)) };
                            let meta = if is_module { self.core.module } else { self.core.class };
                            let ncls = self.heap.alloc(meta, ObjKind::Class(ClassData { name: Some(s), superclass: sup, is_module, outer: Some(outer), ..Default::default() }));
                            self.heap.class_mut(outer).consts.insert(s, Slot::from(Value::Obj(ncls)));
                            // `setup_class` is `mrb_const_set` (src/class.c), so defining a
                            // class or a module fires `const_added` on the outer one — before
                            // `inherited`, which `mrb_vm_define_class` calls after it
                            self.const_added(outer, s)?;
                            if !is_module { self.singleton_class(Value::Obj(ncls))?; }
                            if let Some(sup) = sup { self.call_inherited(sup, ncls)?; }
                            ncls
                        }
                    };
                    setreg!(a, Value::Obj(cls));
                }
                Op::Exec => {
                    let nirep = self.ireps[irep].reps[b];
                    let cls = match reg!(a) { Value::Obj(o) if self.heap.is_class(o) => o, _ => return Err(self.raise_type("not a class/module")) };
                    let p = self.heap.alloc(self.core.proc_, ObjKind::Proc(ProcData { irep: nirep, upper: Some(self.ci[top].proc_), env: None, target_class: Some(cls), strict: false, scope: true, orphan: false, mid: None }));
                    let nbase = base + a;
                    let nregs = self.ireps[nirep].nregs.max(4);
                    if self.stack.len() < nbase + nregs { self.stack.resize(nbase + nregs, Slot::NIL); }
                    for i in 1..nregs { self.stack[nbase + i] = Slot::NIL; }
                    self.ci.push(CallInfo { base: nbase, pc: 0, irep: nirep, proc_: p, n: 0, kw: false, mid: None, target_class: cls, env: None, cci: Cci::None, vis: Vis::Public, modfunc: false, vis_break: false });
                }
                Op::Def => {
                    let s = self.ireps[irep].syms[b];
                    let target = match reg!(a) { Value::Obj(o) if self.heap.is_class(o) => o, _ => return Err(self.raise_type("not a class/module")) };
                    let p = match reg!(a + 1) { Value::Obj(o) if matches!(self.heap.get(o).kind, ObjKind::Proc(_)) => o, _ => return Err(self.raise_type("not a proc")) };
                    if let ObjKind::Proc(pd) = &mut self.heap.get_mut(p).kind { pd.target_class = Some(target); }
                    let (vis, modfunc) = if self.heap.class(target).is_singleton { (Vis::Public, false) } else { self.current_def_vis(target) };
                    self.def_method(target, s, Method::Ruby(p), if modfunc { Vis::Private } else { vis })?;
                    if modfunc {
                        let sc = self.singleton_class(Value::Obj(target))?;
                        self.def_method(sc, s, Method::Ruby(p), Vis::Public)?;
                    }
                    setreg!(a, Value::Sym(s));
                }
                Op::Tdef | Op::Sdef => {
                    let s = self.ireps[irep].syms[b];
                    let nirep = self.ireps[irep].reps[c];
                    let target = if matches!(op, Op::Tdef) { self.ci[top].target_class } else { let v = reg!(a); self.singleton_class(v)? };
                    let p = self.heap.alloc(self.core.proc_, ObjKind::Proc(ProcData { irep: nirep, upper: Some(self.ci[top].proc_), env: None, target_class: Some(target), strict: true, scope: true, orphan: false, mid: None }));
                    let (vis, modfunc) = if matches!(op, Op::Tdef) && !self.heap.class(target).is_singleton { self.current_def_vis(target) } else { (Vis::Public, false) };
                    self.def_method(target, s, Method::Ruby(p), if modfunc { Vis::Private } else { vis })?;
                    if modfunc { let sc = self.singleton_class(Value::Obj(target))?; self.def_method(sc, s, Method::Ruby(p), Vis::Public)?; }
                    setreg!(a, Value::Sym(s));
                }
                Op::Alias => {
                    let (new, old) = (self.ireps[irep].syms[a], self.ireps[irep].syms[b]);
                    let tc = self.ci[top].target_class;
                    self.alias_method(tc, new, old)?;
                }
                Op::Undef => { let s = self.ireps[irep].syms[a]; self.undef_method(self.ci[top].target_class, s)?; }
                Op::Sclass => { let v = reg!(a); setreg!(a, Value::Obj(self.singleton_class(v)?)); }
                Op::Tclass => { setreg!(a, Value::Obj(self.ci[top].target_class)); }
                Op::Debug => {
                    // one step of a native loop frame (`Vm::push_loop_frame`); anywhere else a no-op
                    if irep == LOOP_IREP {
                        self.ci[top].pc = 0;
                        match crate::builtins::array::loop_step(self, base)? {
                            LoopNext::Call(n) => self.loop_call_block(top, base, n)?,
                            LoopNext::Tail(n) => { self.ci[top].pc = LOOP_TAIL_PC; self.loop_call_block(top, base, n)?; }
                            LoopNext::Done(v) => { if let Some(r) = self.op_return(v, stop_depth, lc)? { return Ok(r); } }
                        }
                    }
                }
                Op::Err => {
                    let msg = match &self.ireps[irep].pool[a] { Pool::Str(s) => String::from_utf8_lossy(s).into_owned(), _ => "error".into() };
                    return Err(self.raise(self.core.local_jump_error, &msg));
                }
                Op::Ext1 => { ext = 1; }
                Op::Ext2 => { ext = 2; }
                Op::Ext3 => { ext = 3; }
                Op::Stop => {
                    // mruby returns regs[irep->nlocals] (the last expression's register).
                    let nlocals = self.ireps[irep].nlocals;
                    let v = self.stack.get(base + nlocals).map(|s| s.get()).unwrap_or(Value::Nil);
                    let _ = self.pop_frame();
                    return Ok(v);
                }
            }
        }
    }

    // ------------------------------------------------------------------ helpers used by the loop

    fn const_lookup(&mut self, target_class: ObjId, proc_: ObjId, s: Sym) -> VmResult<Value> {
        // 1. target class and its ancestors, 2. lexical scopes (upper procs), 3. Object
        let mut c = Some(target_class);
        if let Some(v) = c.and_then(|c| self.const_get(c, s)) { return Ok(v); }
        let mut p = Some(proc_);
        while let Some(pid) = p {
            let pd = self.heap.proc_data(pid);
            if let Some(tc) = pd.target_class { if let Some(v) = self.const_get(tc, s) { return Ok(v); } }
            p = pd.upper;
        }
        c = Some(self.core.object);
        if let Some(v) = c.and_then(|c| self.const_get(c, s)) { return Ok(v); }
        // `const_missing` hook on the target class (default raises NameError)
        let cm = self.intern("const_missing");
        let tc = Value::Obj(target_class);
        self.funcall(tc, cm, &[Value::Sym(s)], Value::Nil)
    }
    pub fn frozen_error(&mut self, v: Value) -> VmError {
        let c = self.describe_for_error(v);
        let i = self.inspect_str(v).unwrap_or_default();
        self.raise(self.core.frozen_error, &format!("can't modify frozen {c}: {i}"))
    }

    /// `mrb_vm_cv_get`: the lexically enclosing non-singleton class (walks `upper`).
    fn cvar_class(&self, proc_: ObjId) -> ObjId {
        let mut p = Some(proc_);
        while let Some(pid) = p {
            let pd = self.heap.proc_data(pid);
            if let Some(tc) = pd.target_class { if !self.heap.class(tc).is_singleton { return tc; } }
            p = pd.upper;
        }
        self.core.object
    }
    /// The superclass as Ruby sees it (skipping include classes and singletons).
    pub fn real_superclass(&self, c: ObjId) -> Option<ObjId> {
        let mut s = self.heap.class(c).superclass;
        while let Some(x) = s {
            let cd = self.heap.class(x);
            if cd.iclass_of.is_none() && !cd.is_singleton && cd.origin_of.is_none() { return Some(x); }
            s = cd.superclass;
        }
        None
    }
    /// `Class#inherited` hook.
    pub fn call_inherited(&mut self, sup: ObjId, sub: ObjId) -> VmResult<()> {
        let inh = self.intern("inherited");
        self.funcall(Value::Obj(sup), inh, &[Value::Obj(sub)], Value::Nil)?;
        Ok(())
    }
    /// `mrb_mod_cv_get`: walks the whole chain and the *last* table that has the
    /// variable wins (an included module's value shadows the class's own).
    fn cvar_get(&self, class: ObjId, s: Sym) -> Option<Value> {
        let mut found = None;
        let mut c = Some(class);
        while let Some(x) = c {
            let cd = self.heap.class(x);
            let owner = cd.iclass_of.unwrap_or(x);
            if let Some(v) = self.heap.class(owner).cvars.get(&s) { found = Some(v.get()); }
            c = cd.superclass;
        }
        found
    }
    /// `mrb_mod_cv_set`: the first table (from the class up) that has the variable, else the class itself.
    fn cvar_set(&mut self, class: ObjId, s: Sym, v: Value) -> VmResult<()> {
        let mut c = Some(class);
        while let Some(x) = c {
            let owner = self.heap.class(x).iclass_of.unwrap_or(x);
            if self.heap.class(owner).cvars.contains_key(&s) {
                if self.heap.get(owner).frozen { return Err(self.frozen_error(Value::Obj(owner))); }
                self.heap.class_mut(owner).cvars.insert(s, Slot::from(v));
                return Ok(());
            }
            c = self.heap.class(x).superclass;
        }
        if self.heap.get(class).frozen { return Err(self.frozen_error(Value::Obj(class))); }
        self.heap.class_mut(class).cvars.insert(s, Slot::from(v));
        Ok(())
    }

    /// `mrb_ary_splat`: Array as is; `to_a` if it answers (nil means "no conversion");
    /// a non-Array `to_a` result is a TypeError.
    pub fn to_array(&mut self, v: Value) -> VmResult<Vec<Value>> {
        if let Some(a) = self.ary_vals(v) { return Ok(a); }
        let to_a = self.intern("to_a");
        if !self.respond_to(v, to_a) { return Ok(vec![v]); }
        let r = self.funcall(v, to_a, &[], Value::Nil)?;
        if r.is_nil() { return Ok(vec![v]); }
        match self.ary(r) {
            Some(a) => Ok(values_of(a)),
            None => { let c = self.describe_for_error(v); let rc = self.describe_for_error(r); Err(self.raise_type(&format!("can't convert {c} to Array ({c}#to_a gives {rc})"))) }
        }
    }

    /// `mrb_obj_as_string`: String as is, otherwise `to_s`.
    pub fn as_string(&mut self, v: Value) -> VmResult<Vec<u8>> {
        if let Some(b) = self.str_bytes(v) { return Ok(b.to_vec()); }
        let r = self.funcall(v, self.s.to_s, &[], Value::Nil)?;
        match self.str_bytes(r) { Some(b) => Ok(b.to_vec()), None => Ok(b"".to_vec()) }
    }

    /// The identity `BasicObject#==` and `Object#eql?` go by: the same value, or two
    /// [`Vm::data_new`] objects naming the same host value. `equal?` is the plain identity and
    /// is not this.
    pub fn same_value(&self, a: Value, b: Value) -> bool {
        if a == b { return true; }
        match (self.data_of(a), self.data_of(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        }
    }
    /// `eql?`-style equality used for Hash keys.
    pub fn eql(&self, a: Value, b: Value) -> bool {
        match (a, b) {
            (Value::Obj(x), Value::Obj(y)) => {
                if x == y { return true; }
                match (&self.heap.get(x).kind, &self.heap.get(y).kind) {
                    (ObjKind::String(p), ObjKind::String(q)) => p == q,
                    // as `Object#eql?` does (`Vm::same_value`)
                    (ObjKind::Data { tag: t1, handle: h1 }, ObjKind::Data { tag: t2, handle: h2 }) => t1 == t2 && h1 == h2,
                    _ => false,
                }
            }
            _ => a == b,
        }
    }
    /// The hash code of a key: native for immediates and Strings, `hash` for other objects.
    pub fn key_hash(&mut self, k: Value) -> VmResult<i64> {
        match k {
            Value::Obj(o) if !matches!(self.heap.get(o).kind, ObjKind::String(_)) => {
                let hs = self.s.hash;
                // callers (HASH, keyword packing) hold the hash being built in a Rust local
                self.native_active += 1;
                let r = self.funcall(k, hs, &[], Value::Nil);
                self.native_active -= 1;
                match r? { Value::Int(i) => Ok(i), Value::Float(f) => Ok(f as i64), _ => Ok(self.value_hash(k)) }
            }
            _ => Ok(self.value_hash(k)),
        }
    }
    /// Key equality for lookups: `eql?` (natively for immediates and Strings).
    pub fn key_eql(&mut self, a: Value, b: Value) -> VmResult<bool> {
        match (a, b) {
            (Value::Obj(x), _) if !matches!(self.heap.get(x).kind, ObjKind::String(_)) => {
                let eql = self.s.eql;
                self.native_active += 1; // as in key_hash
                let r = self.funcall(a, eql, &[b], Value::Nil);
                self.native_active -= 1;
                Ok(r?.truthy())
            }
            _ => Ok(self.eql(a, b)),
        }
    }
    /// Makes the cached hashes match the entries (after wholesale edits of `entries`).
    ///
    /// Every lookup passes through here, and the usual answer is "nothing to do", so the
    /// question is asked before the keys are copied out: collecting them first made a
    /// lookup cost one copy of the whole hash, which is the O(n) the index of stage 2c did
    /// not remove (0.32 ns per entry per lookup, 323 ns on a hash of a thousand).
    fn hash_sync(&mut self, o: ObjId) -> VmResult<()> {
        if !matches!(&self.heap.get(o).kind, ObjKind::Hash(hd) if hd.hashes_stale()) { return Ok(()); }
        let keys: Vec<Value> = match &self.heap.get(o).kind { ObjKind::Hash(hd) => hd.entries().iter().map(|e| e.0.get()).collect(), _ => return Ok(()) };
        let mut hs = Vec::with_capacity(keys.len());
        for k in keys { hs.push(self.key_hash(k)?); }
        if let ObjKind::Hash(hd) = &mut self.heap.get_mut(o).kind { hd.set_hashes(hs); }
        Ok(())
    }
    /// Index of `k` in the hash (hash code first, then `eql?`).
    ///
    /// The search is still the linear one mruby's AR mode does, but it walks the cached hash
    /// codes as a slice under one borrow and only reaches into the heap again for a position
    /// whose code matched. Verifying a candidate has to leave the borrow — `eql?` can be
    /// Ruby — so the old shape took `&self.heap.get(o)` once per *element*, which is a
    /// bounds-checked index plus a match on `ObjKind` for every entry it walks past.
    pub fn hash_index(&mut self, h: Value, k: Value) -> VmResult<Option<usize>> {
        let o = match h.obj() { Some(o) => o, None => return Ok(None) };
        if !matches!(self.heap.get(o).kind, ObjKind::Hash(_)) { return Ok(None); }
        self.hash_sync(o)?;
        let kh = self.key_hash(k)?;
        self.hash_index_at(o, k, kh)
    }
    /// The candidate walk of [`Vm::hash_index`], for a caller that has already made the
    /// cached codes current and hashed the key. `hash_set` needs the code a second time to
    /// store it, and hashing a key can run Ruby, so it must not be asked for twice.
    fn hash_index_at(&mut self, o: ObjId, k: Value, kh: i64) -> VmResult<Option<usize>> {
        let mut cand = match &self.heap.get(o).kind { ObjKind::Hash(hd) => hd.first_candidate(kh), _ => None };
        while let Some((p, ek)) = cand {
            if self.key_eql(k, ek.get())? { return Ok(Some(p)); }
            cand = match &self.heap.get(o).kind { ObjKind::Hash(hd) => hd.next_candidate(p, kh), _ => None };
        }
        Ok(None)
    }
    pub fn hash_get(&mut self, h: Value, k: Value) -> Option<Value> {
        match self.hash_index(h, k) {
            Ok(Some(i)) => match &self.heap.get(h.obj().unwrap()).kind { ObjKind::Hash(hd) => hd.entries().get(i).map(|e| e.1.get()), _ => None },
            _ => None,
        }
    }
    /// `h[k] = v`.
    ///
    /// The key is hashed once and looked up as it was handed in. Only a key that is really
    /// being inserted is copied: an existing entry keeps the key it was stored with, so
    /// storing over one never has to allocate. A String key compares and hashes by its
    /// bytes (`Vm::eql`, `Vm::value_hash`), so the copy answers both the same way.
    pub fn hash_set(&mut self, h: Value, k: Value, v: Value) -> VmResult<()> {
        let o = match h.obj() { Some(o) => o, None => return Err(self.raise_type("not a hash")) };
        if self.heap.get(o).frozen { return Err(self.frozen_error(h)); }
        self.hash_sync(o)?;
        let kh = self.key_hash(k)?;
        match self.hash_index_at(o, k, kh)? {
            Some(i) => { if let ObjKind::Hash(hd) = &mut self.heap.get_mut(o).kind { hd.set_value_at(i, Slot::from(v)); } }
            None => {
                // an unfrozen String key is copied and the copy frozen; a frozen key is used as is
                let k = match k { Value::Obj(ko) if self.heap.string(ko).is_some() && !self.heap.get(ko).frozen => { let b = self.heap.string(ko).unwrap().to_vec(); let nk = Value::Obj(self.heap.alloc(self.core.string, ObjKind::String(b))); if let Some(no) = nk.obj() { self.heap.get_mut(no).frozen = true; } nk } _ => k };
                if let ObjKind::Hash(hd) = &mut self.heap.get_mut(o).kind { hd.push_entry(Slot::from(k), Slot::from(v), kh); }
            }
        }
        Ok(())
    }

    fn op_arith(&mut self, base: usize, a: usize, mid: Sym) -> VmResult<()> {
        let (x, y) = (self.stack[base + a].get(), self.stack[base + a + 1].get());
        let r = match (x, y) {
            (Value::Int(p), Value::Int(q)) => {
                let s = self.s;
                if mid == s.plus { p.checked_add(q).map(Value::Int) }
                else if mid == s.minus { p.checked_sub(q).map(Value::Int) }
                else if mid == s.mul { p.checked_mul(q).map(Value::Int) }
                else if q == 0 { return Err(self.raise(self.core.zero_division_error, "divided by 0")); }
                // `MRB_INT_MIN / -1` leaves the range: the wide path below answers it
                else if p == i64::MIN && q == -1 { None }
                else { Some(Value::Int(crate::builtins::numeric::div_floor(p, q))) }
            }
            (Value::Int(p), Value::Float(q)) => Some(float_op(mid, self.s, p as f64, q)),
            (Value::Float(p), Value::Int(q)) => Some(float_op(mid, self.s, p, q as f64)),
            (Value::Float(p), Value::Float(q)) => Some(float_op(mid, self.s, p, q)),
            _ => None,
        };
        match r {
            Some(v) => { self.stack[base + a] = Slot::from(v); Ok(()) }
            None => {
                // `L_INT_OVERFLOW`: the result left the `i64` range, so it is a wide integer
                if let (Value::Int(p), Value::Int(q)) = (x, y) {
                    let v = if mid == self.s.div { let b = crate::bigint::BigInt::from_i64(p).neg(); self.bint_value(b) }
                            else { crate::builtins::numeric::int_overflow_op(self, p, q, mid) };
                    self.stack[base + a] = Slot::from(v);
                    return Ok(());
                }
                self.op_send(base, a, mid, 1, false, false)
            }
        }
    }

    fn op_compare(&mut self, base: usize, a: usize, mid: Sym) -> VmResult<()> {
        let (x, y) = (self.stack[base + a].get(), self.stack[base + a + 1].get());
        let s = self.s;
        let num = |p: f64, q: f64| -> Value {
            Value::bool(if mid == s.eq { p == q } else if mid == s.lt { p < q } else if mid == s.le { p <= q } else if mid == s.gt { p > q } else { p >= q })
        };
        let ord = |o: Option<core::cmp::Ordering>| -> Value {
            Value::bool(match o { None => false, Some(o) => if mid == s.eq { o.is_eq() } else if mid == s.lt { o.is_lt() } else if mid == s.le { o.is_le() } else if mid == s.gt { o.is_gt() } else { o.is_ge() } })
        };
        let _ = &num;
        let r = match (x, y) {
            (Value::Int(p), Value::Int(q)) => Some(Value::bool(if mid == s.eq { p == q } else if mid == s.lt { p < q } else if mid == s.le { p <= q } else if mid == s.gt { p > q } else { p >= q })),
            (Value::Int(p), Value::Float(q)) => Some(ord(crate::builtins::numeric::int_float_cmp(p, q))),
            (Value::Float(p), Value::Int(q)) => Some(ord(crate::builtins::numeric::int_float_cmp(q, p).map(|o| o.reverse()))),
            (Value::Float(p), Value::Float(q)) => Some(ord(p.partial_cmp(&q))),
            _ if mid == s.eq => {
                // fast path (mruby `mrb_obj_eq` before the `==` send): identical
                // immediates or the same object; a NaN is never equal to itself
                if x == y { Some(Value::True) } else { None }
            }
            _ => None,
        };
        match r {
            Some(v) => { self.stack[base + a] = Slot::from(v); Ok(()) }
            None => self.op_send(base, a, mid, 1, false, false),
        }
    }

    /// Shared body of SEND/SSEND/SUPER: receiver at `R[a]`, args at `R[a+1..]`,
    /// block at `R[a+argc+1]` when `has_blk`.
    /// Lays out a call site (`c` = SEND's third operand): packs `nk` keyword pairs
    /// into one Hash right after the positional arguments, moves the block after
    /// it, and returns `(n, kw, blk)` for the new frame (`n` = 15 when packed).
    fn prepare_call(&mut self, nbase: usize, c: usize, has_blk: bool) -> VmResult<(usize, bool, Value)> {
        let n = c & 0xf;
        let nk = (c >> 4) & 0xf;
        let npos = if n == 15 { 1 } else { n };
        let bidx = nbase + npos + (if nk == 15 { 1 } else { nk * 2 }) + 1;
        if self.stack.len() <= bidx { self.stack.resize(bidx + 1, Slot::NIL); }
        let blk = if has_blk { let b = self.stack[bidx].get(); self.ensure_block(b)? } else { Value::Nil };
        let kw = nk > 0;
        if nk > 0 && nk != 15 {
            let kidx = nbase + npos + 1;
            let h = self.hash_new();
            for i in 0..nk { let (k, v) = (self.stack[kidx + i * 2].get(), self.stack[kidx + i * 2 + 1].get()); self.hash_set(h, k, v)?; }
            self.stack[kidx] = Slot::from(h);
        } else if nk == 15 {
            let h = self.stack[nbase + npos + 1].get();
            if !matches!(h.obj().map(|o| &self.heap.get(o).kind), Some(ObjKind::Hash(_))) { return Err(self.raise_type("keyword argument hash expected")); }
        }
        let new_bidx = nbase + npos + (if kw { 1 } else { 0 }) + 1;
        if self.stack.len() <= new_bidx { self.stack.resize(new_bidx + 1, Slot::NIL); }
        self.stack[new_bidx] = Slot::from(blk);
        Ok((n, kw, blk))
    }

    /// The positional arguments of a frame laid out by `prepare_call`, and the
    /// keyword Hash the frame carries (`ci->kw`) when it has one — **also when
    /// that Hash is empty**. mruby keeps an empty keyword Hash on the stack with
    /// `ci->kw` still set (`mrb_get_args` folds a keyword Hash into the positional
    /// arguments only `if (mrb_hash_size(kdict) > 0)`, and only then clears
    /// `ci->kw`), so a re-dispatch that rewrites the frame — `send`, `method_missing`
    /// — has to carry it along: `def one(x); end; send(:one, **{})` binds `x` to the
    /// empty Hash, because `OP_ENTER`'s fast path counts `ci->kw` as an argument.
    fn native_args(&self, nbase: usize, n: usize, kw: bool) -> (Vec<Value>, Option<Value>) {
        let args = self.send_args(nbase, n);
        let npos = if n == 15 { 1 } else { n };
        let kd = if kw { Some(self.stack[nbase + npos + 1].get()) } else { None };
        (args, kd)
    }

    /// The keyword Hash when it has entries — what mruby appends to the positional
    /// arguments of a function whose format has no `:` (and what it hands a block's
    /// `OP_ENTER` that takes no keywords). An empty one is left out of the arguments.
    fn kdict_nonempty(&self, kd: Option<Value>) -> Option<Value> {
        let h = kd?;
        match h.obj().map(|o| &self.heap.get(o).kind) { Some(ObjKind::Hash(hd)) if !hd.is_empty() => Some(h), _ => None }
    }

    /// What a native method sees: [`Vm::native_args`] with the keyword Hash appended
    /// when it has entries, and that same Hash as the second element (`pending_kw`).
    fn native_call_args(&self, nbase: usize, n: usize, kw: bool) -> (Vec<Value>, Option<Value>) {
        let (mut args, kd) = self.native_args(nbase, n, kw);
        let kd = self.kdict_nonempty(kd);
        if let Some(h) = kd { args.push(h); }
        (args, kd)
    }

    // ------------------------------------------------------ the inline index opcodes

    /// `OP_GETIDX` (`R[a] = R[a][R[a+1]]`), mruby `vm_op_getidx`. Answers the call here for an
    /// Array with an Integer index, a Hash, and a String with an Integer, String or Range
    /// index, while that class still carries the `[]` the opcode stands in for. Answers `true`
    /// when the caller has to send `[]` instead; the registers are already laid out for it
    /// (the receiver is in `R[a]` and the index in `R[a+1]`, which is what a one-argument call
    /// wants), so nothing has to be moved.
    fn op_getidx(&mut self, base: usize, a: usize) -> VmResult<bool> {
        let recv = self.stack[base + a].get();
        let o = match recv { Value::Obj(o) => o, _ => return Ok(true) };
        let slot = match self.idx_kind(o) { Some(s) => s, None => return Ok(true) };
        let cls = self.heap.get(o).class;
        if !self.idx_armed(slot, cls) { return Ok(true); }
        let idx = self.stack[base + a + 1].get();
        let v = match slot {
            IDX_ARY_AREF => match idx {
                Value::Int(i) => self.ary_entry(o, i),
                _ => return Ok(true),
            },
            // `mrb_hash_get`: a Hash without the key answers through `default`/`default_proc`.
            // A default proc is left to the send, where `Hash#[]` runs it in a frame of its own
            // (no native boundary, so a task can wait inside it); the rest is answered here.
            IDX_HASH_AREF => match self.hash_get(recv, idx) {
                Some(v) => v,
                // a default proc runs in a frame where this instruction's value goes
                None => match self.hash_miss_at(recv, o, idx, base + a)? { Some(v) => v, None => return Ok(false) },
            },
            _ => {
                if !self.str_index_p(idx) { return Ok(true); }
                self.call_native(crate::builtins::string::str_aref, recv, &[idx], Value::Nil)?
            }
        };
        self.stack[base + a] = Slot::from(v);
        Ok(false)
    }

    /// `OP_GETIDX0` (`R[a] = R[b][0]`), mruby `vm_op_getidx0`. The same three receivers with a
    /// literal 0. Before it asks for the send it writes the call's registers, which the
    /// operands do not already hold: the receiver into `R[a]` and the index into `R[a+1]`.
    fn op_getidx0(&mut self, base: usize, a: usize, b: usize) -> VmResult<bool> {
        let recv = self.stack[base + b].get();
        let fallback = |vm: &mut Vm| {
            if vm.stack.len() <= base + a + 1 { vm.stack.resize(base + a + 2, Slot::NIL); }
            vm.stack[base + a] = Slot::from(recv);
            vm.stack[base + a + 1] = Slot::from(Value::Int(0));
            Ok(true)
        };
        let o = match recv { Value::Obj(o) => o, _ => return fallback(self) };
        let slot = match self.idx_kind(o) { Some(s) => s, None => return fallback(self) };
        let cls = self.heap.get(o).class;
        if !self.idx_armed(slot, cls) { return fallback(self); }
        let v = match slot {
            IDX_ARY_AREF => self.heap.array(o).and_then(|l| l.first().map(|e| e.get())).unwrap_or(Value::Nil),
            IDX_HASH_AREF => match self.hash_get(recv, Value::Int(0)) {
                Some(v) => v,
                None => {
                    if self.stack.len() <= base + a + 1 { self.stack.resize(base + a + 2, Slot::NIL); }
                    match self.hash_miss_at(recv, o, Value::Int(0), base + a)? { Some(v) => v, None => return Ok(false) }
                }
            },
            _ => self.call_native(crate::builtins::string::str_aref, recv, &[Value::Int(0)], Value::Nil)?,
        };
        if self.stack.len() <= base + a { self.stack.resize(base + a + 1, Slot::NIL); }
        self.stack[base + a] = Slot::from(v);
        Ok(false)
    }

    /// `OP_SETIDX` (`R[a][R[a+1]] = R[a+2]`), mruby `vm_op_setidx`. What the fast path leaves
    /// in `R[a]` is the assigned value and what the send leaves there is whatever `[]=`
    /// answered; neither is read, because the compiler copies the right-hand side into the
    /// result register before it emits the opcode (`codegen.c`, "preserve the RHS as the
    /// result"), which is why `x[i] = v` is `v` whichever path ran.
    fn op_setidx(&mut self, base: usize, a: usize) -> VmResult<bool> {
        let recv = self.stack[base + a].get();
        let o = match recv { Value::Obj(o) => o, _ => return Ok(true) };
        let slot = match self.idx_kind(o) { Some(s) => s + IDX_ARY_ASET, None => return Ok(true) };
        let cls = self.heap.get(o).class;
        if !self.idx_armed(slot, cls) { return Ok(true); }
        let (idx, val) = (self.stack[base + a + 1].get(), self.stack[base + a + 2].get());
        match slot {
            IDX_ARY_ASET => {
                if !matches!(idx, Value::Int(_)) { return Ok(true); }
                self.call_native(crate::builtins::array::ary_aset, recv, &[idx, val], Value::Nil)?;
            }
            IDX_HASH_ASET => { self.call_native(crate::builtins::hash::hash_aset, recv, &[idx, val], Value::Nil)?; }
            _ => {
                // A replacement that is not a String is a TypeError rather than a store, and the
                // method is where it is raised: leaving it to the send keeps the `String#[]=`
                // frame the backtrace has always shown for it (the reference says the same).
                if !matches!(val, Value::Obj(v) if matches!(self.heap.get(v).kind, ObjKind::String(_))) { return Ok(true); }
                if !self.str_index_p(idx) { return Ok(true); }
                self.call_native(crate::builtins::string::str_aset, recv, &[idx, val], Value::Nil)?;
            }
        }
        self.stack[base + a] = Slot::from(val);
        Ok(false)
    }

    /// One element of an Array by index, counting a negative one from the end (`mrb_ary_entry`).
    fn ary_entry(&self, o: ObjId, i: i64) -> Value {
        let list = match self.heap.array(o) { Some(l) => l, None => return Value::Nil };
        let i = if i < 0 { i + list.len() as i64 } else { i };
        if i < 0 { Value::Nil } else { list.get(i as usize).map(|e| e.get()).unwrap_or(Value::Nil) }
    }

    fn op_send(&mut self, base: usize, a: usize, mid: Sym, c: usize, has_blk: bool, is_super: bool) -> VmResult<()> {
        self.op_send_vis(base, a, mid, c, has_blk, is_super, false)
    }
    /// `explicit`: the receiver was written (SEND/SEND0/SENDB), so private/protected are enforced.
    fn op_send_vis(&mut self, base: usize, a: usize, mid: Sym, c: usize, has_blk: bool, is_super: bool, explicit: bool) -> VmResult<()> {
        let recv = self.stack[base + a].get();
        let (argc, kw, blk) = self.prepare_call(base + a, c, has_blk)?;
        let start_class = if is_super {
            let owner = self.ci.last().unwrap().target_class;
            match self.heap.class(owner).superclass { Some(s) => s, None => return Err(self.raise(self.core.no_method_error, "super: no superclass method")) }
        } else { self.class_of(recv) };
        let found = self.find_method_cached(start_class, mid);
        let (m, owner) = match found {
            Some((m, owner)) => {
                if explicit && !is_super {
                    match self.method_vis(owner, mid) {
                        Vis::Public => {}
                        Vis::Private => { let name = self.sym_name(mid); let desc = self.describe_for_error(recv); return Err(self.no_method_error(mid, recv, &format!("private method '{name}' called for {desc}"))); }
                        Vis::Protected => {
                            let caller_self = self.stack[base].get();
                            let home = self.heap.class(owner).iclass_of.unwrap_or(owner);
                            if !self.obj_is_kind_of(caller_self, home) { let name = self.sym_name(mid); let desc = self.describe_for_error(recv); return Err(self.no_method_error(mid, recv, &format!("protected method '{name}' called for {desc}"))); }
                        }
                    }
                }
                (m, owner)
            }
            None => {
                // method_missing (user-defined) takes the call; the basic one reports the error
                let mm = self.s.method_missing;
                let mm_class = if is_super { self.class_of(recv) } else { start_class };
                if let Some((m, _owner)) = self.find_method(mm_class, mm) {
                    if !matches!(m, Method::Native(_)) {
                        // insert the method name as the first argument
                        let (args, kd) = self.native_args(base + a, argc, kw);
                        let mut nargs = vec![Value::Sym(mid)];
                        nargs.extend_from_slice(&args);
                        if matches!(m, Method::Ruby(_)) {
                            // A `method_missing` written in Ruby takes the call *in this
                            // frame*, the way `send` does (mruby's `prepare_missing` rewrites
                            // the frame it is already in and lets OP_SEND dispatch it). A
                            // nested run loop would put a native boundary around the body,
                            // and `Fiber.yield`, `break` and a blocking `Task::Queue#pop`
                            // inside it would all be refused. Sending `method_missing` as the
                            // name (not `mid`) is what makes a `super` in the body find the
                            // next `method_missing` up the chain, and going through
                            // `op_send_vis` with `explicit` false is what lets a private one
                            // answer, as it does in the reference and in CRuby.
                            let c = self.relay_args(base + a, nargs, kd, blk, true);
                            return self.op_send_vis(base, a, mm, c, has_blk, false, false);
                        }
                        let r = match m {
                            Method::Closure(f) => { let kd = self.kdict_nonempty(kd); if let Some(k) = kd { nargs.push(k); } let (v, sw) = self.call_closure_direct(&f, recv, &nargs, blk, base + a)?; if sw { return Ok(()); } v }
                            _ => Value::Nil,
                        };
                        self.stack[base + a] = Slot::from(r);
                        return Ok(());
                    }
                }
                let name = self.sym_name(mid);
                let desc = self.describe_for_error(recv);
                let args = self.native_call_args(base + a, argc, kw).0;
                let msg = if is_super { format!("no superclass method '{name}' for {desc}") } else { format!("undefined method '{name}' for {desc}") };
                let e = self.no_method_error(mid, recv, &msg);
                if let VmError::Raise(Value::Obj(o)) = e { let av = self.ary_new(args); let k = self.intern("@args"); self.heap.ivar_set(o, k, av); }
                return Err(e);
            }
        };
        match m {
            MethodRef::Native(f) => {
                self.native_mid = Some(mid);
                if core::ptr::fn_addr_eq(f, crate::builtins::object::send as crate::object::NativeFn) {
                    // `send`/`__send__` from bytecode re-dispatches in this frame
                    // (mruby `mrb_f_send` → `mrb_exec_irep`): no native boundary,
                    // so `Fiber.yield` and `break` inside the callee still work.
                    return self.op_send_redirect(base, a, argc, kw, has_blk, blk);
                }
                let (args, kd) = self.native_call_args(base + a, argc, kw);
                let saved = self.pending_kw.replace(kd.unwrap_or(Value::Nil));
                let r = self.call_native_direct(f, recv, &args, blk, base + a);
                self.pending_kw = saved;
                let (v, switched) = r?;
                if !switched { self.stack[base + a] = Slot::from(v); }
            }
            MethodRef::Closure => {
                self.native_mid = Some(mid);
                let f = match self.closure_of(owner, mid) { Some(f) => f, None => return Err(VmError::Internal("closure vanished between lookup and call".into())) };
                let (args, kd) = self.native_call_args(base + a, argc, kw);
                let saved = self.pending_kw.replace(kd.unwrap_or(Value::Nil));
                let r = self.call_closure_direct(&f, recv, &args, blk, base + a);
                self.pending_kw = saved;
                let (v, switched) = r?;
                if !switched { self.stack[base + a] = Slot::from(v); }
            }
            MethodRef::AttrReader(iv) => {
                let (args, _) = self.native_call_args(base + a, argc, kw);
                if !args.is_empty() { return Err(self.argnum_error(args.len(), "0")); }
                self.stack[base + a] = Slot::from(recv.obj().map(|o| self.heap.ivar_get(o, iv)).unwrap_or(Value::Nil));
            }
            MethodRef::AttrWriter(iv) => {
                let (args, _) = self.native_call_args(base + a, argc, kw);
                if args.len() != 1 { return Err(self.argnum_error(args.len(), "1")); }
                let v = args[0];
                match recv { Value::Obj(o) => self.heap.ivar_set(o, iv, v), _ => return Err(self.raise_type("can't set instance variable")) }
                self.stack[base + a] = Slot::from(v);
            }
            MethodRef::Ruby(p) => {
                if self.ci.len() >= CALL_LEVEL_MAX { return Err(self.raise(self.core.system_stack_error, "stack level too deep")); }
                let nbase = base + a;
                let nirep = self.heap.proc_data(p).irep;
                let nregs = self.ireps[nirep].nregs.max(argc + 2).max(4);
                if self.stack.len() < nbase + nregs { self.stack.resize(nbase + nregs, Slot::NIL); }
                let used = (if argc == 15 { 1 } else { argc }) + (if kw { 1 } else { 0 }) + 2;
                for i in used..nregs { self.stack[nbase + i] = Slot::NIL; }
                self.ci.push(CallInfo { base: nbase, pc: 0, irep: nirep, proc_: p, n: argc as u8, kw, mid: Some(self.heap.proc_data(p).mid.unwrap_or(mid)), target_class: owner, env: None, cci: Cci::None, vis: Vis::Public, modfunc: false, vis_break: false });
            }
        }
        Ok(())
    }

    /// `mrb_ci_bidx`: the register (relative to base) holding a frame's block.
    pub fn frame_bidx(ci: &CallInfo) -> usize {
        let n = ci.n as usize;
        (if n == 15 { 1 } else { n }) + (if ci.kw { 1 } else { 0 }) + 1
    }
    /// `mrb_ci_kidx`: the register holding the keyword Hash of frame `i`, if any.
    fn kidx_at(&self, i: usize) -> Option<usize> {
        let ci = &self.ci[i];
        if !ci.kw { return None; }
        let n = ci.n as usize;
        Some(ci.base + (if n == 15 { 1 } else { n }) + 1)
    }
    pub fn hash_delete(&mut self, h: Value, k: Value) -> Option<Value> {
        let o = h.obj()?;
        let pos = self.hash_index(h, k).ok()??;
        match &mut self.heap.get_mut(o).kind { ObjKind::Hash(hd) => Some(hd.remove_entry(pos).1.get()), _ => None }
    }

    /// Positional arguments of a SEND at `nbase` (`argc == 15` = packed array).
    fn send_args(&self, nbase: usize, argc: usize) -> Vec<Value> {
        if argc == 15 { self.ary_vals(self.stack[nbase + 1].get()).unwrap_or_default() } else { values_of(&self.stack[nbase + 1..nbase + 1 + argc]) }
    }

    fn ensure_block(&mut self, b: Value) -> VmResult<Value> {
        match b {
            Value::Nil => Ok(Value::Nil),
            Value::Obj(o) if matches!(self.heap.get(o).kind, ObjKind::Proc(_)) => Ok(b),
            _ => {
                let to_proc = self.intern("to_proc");
                if self.respond_to(b, to_proc) { return self.funcall(b, to_proc, &[], Value::Nil); }
                Err(self.raise_type("not a block"))
            }
        }
    }

    /// mruby `vm_call_proc`: make the top frame execute `p` (self/mid/target
    /// class come from the proc's environment). `nargs` = registers to keep
    /// (self + args + block); the rest of the frame is cleared.
    fn vm_call_proc(&mut self, p: ObjId, nargs: usize) {
        let top = self.ci.len() - 1;
        let base = self.ci[top].base;
        let pd = self.heap.proc_data(p);
        let (irep, env, ptc) = (pd.irep, pd.env, pd.target_class);
        let nregs = self.ireps[irep].nregs.max(4);
        if self.stack.len() < base + nregs { self.stack.resize(base + nregs, Slot::NIL); }
        for i in nargs..nregs { self.stack[base + i] = Slot::NIL; }
        let (mid, tc, self_) = match env {
            Some(e) => { let ed = self.heap.env(e); (ed.mid, ed.target_class.or(ptc).unwrap_or(self.core.object), self.env_get(e, 0)) }
            None => (self.ci[top].mid, ptc.unwrap_or(self.core.object), self.stack[base].get()),
        };
        let ci = &mut self.ci[top];
        ci.irep = irep; ci.proc_ = p; ci.pc = 0; ci.mid = mid; ci.target_class = tc; ci.env = None;
        self.stack[base] = Slot::from(self_);
    }

    fn op_enter(&mut self, spec: u32) -> VmResult<()> {
        let m1 = ((spec >> 18) & 0x1f) as usize;
        let o = ((spec >> 13) & 0x1f) as usize;
        let r = ((spec >> 12) & 1) as usize;
        let m2 = ((spec >> 7) & 0x1f) as usize;
        let k = ((spec >> 2) & 0x1f) as usize;
        let kdict_flag = (spec >> 1) & 1 == 1;
        let noblock = (spec >> 23) & 1 == 1;
        let kd = if k > 0 || kdict_flag { 1 } else { 0 };
        let top = self.ci.len() - 1;
        let (base, mut n, mut kw, proc_, irep) = { let ci = &self.ci[top]; (ci.base, ci.n as usize, ci.kw, ci.proc_, ci.irep) };
        let strict = self.heap.proc_data(proc_).strict;
        let nlocals = self.ireps[irep].nlocals;
        let len = m1 + o + r + m2;
        let nregs = self.ireps[irep].nregs.max(len + kd + 3);
        if self.stack.len() < base + nregs { self.stack.resize(base + nregs, Slot::NIL); }
        // fast path: only required parameters, no packed args
        if (spec & !0x7c0001) == 0 && n < 15 && strict {
            if n + (kw as usize) != m1 { return Err(self.argnum_error(n + kw as usize, &format!("{m1}"))); }
            for i in m1 + 2..nlocals { self.stack[base + i] = Slot::NIL; }
            self.ci[top].kw = false;
            return Ok(());
        }
        let npos = if n == 15 { 1 } else { n };
        let blk = self.stack[base + npos + (kw as usize) + 1].get();
        if noblock && !blk.is_nil() { return Err(self.raise_arg("no block accepted")); }
        let mut kdict = if kw { self.stack[base + npos + 1].get() } else { Value::Nil };
        if kd == 0 {
            let nonempty = match kdict.obj().map(|o| &self.heap.get(o).kind) { Some(ObjKind::Hash(hd)) => !hd.is_empty(), _ => false };
            if nonempty {
                // the keyword Hash becomes the last positional argument
                if n < 14 { n += 1; }
                else if n == 14 { let all: Vec<Value> = values_of(&self.stack[base + 1..base + 16]); self.stack[base + 1] = Slot::from(self.ary_new(all)); n = 15; }
                else if let Some(o) = self.stack[base + 1].get().obj() { if let ObjKind::Array(v) = &mut self.heap.get_mut(o).kind { v.push(Slot::from(kdict)); } }
            }
            kdict = Value::Nil;
            kw = false;
        }
        let mut argv: Vec<Value> = if n == 15 { self.ary_vals(self.stack[base + 1].get()).unwrap_or_default() } else { values_of(&self.stack[base + 1..base + 1 + n]) };
        let mut argc = argv.len();
        if strict {
            if argc < m1 + m2 || (r == 0 && argc > len) {
                let exp = if r == 0 && o == 0 { format!("{}", m1 + m2) } else if r == 0 { format!("{}..{}", m1 + m2, len) } else { format!("{}+", m1 + m2) };
                return Err(self.argnum_error(argc, &exp));
            }
        } else if len > 1 && argc == 1 {
            if let Some(arr) = self.ary_vals(argv[0]) { argv = arr; argc = argv.len(); }
        }
        let mut pc = self.ci[top].pc;
        if argc < len {
            let mlen = if argc < m1 + m2 { if m1 < argc { argc - m1 } else { 0 } } else { m2 };
            for i in 0..argc.saturating_sub(mlen).min(m1 + o) { self.stack[base + 1 + i] = Slot::from(argv[i]); }
            for i in argc..m1 { self.stack[base + 1 + i] = Slot::NIL; }
            for i in 0..mlen { self.stack[base + len - m2 + 1 + i] = Slot::from(argv[argc - mlen + i]); }
            for i in mlen..m2 { self.stack[base + len - m2 + 1 + i] = Slot::NIL; }
            if r == 1 { let rest = self.ary_new(vec![]); self.stack[base + m1 + o + 1] = Slot::from(rest); }
            if o > 0 && argc > m1 + m2 { pc += (argc - m1 - m2) * 3; }
        } else {
            for i in 0..m1 + o { self.stack[base + 1 + i] = Slot::from(argv[i]); }
            let mut rnum = 0;
            if r == 1 { rnum = argc - m1 - o - m2; let rest = self.ary_new(argv[m1 + o..m1 + o + rnum].to_vec()); self.stack[base + m1 + o + 1] = Slot::from(rest); }
            if m2 > 0 { for i in 0..m2 { self.stack[base + m1 + o + r + 1 + i] = Slot::from(argv[m1 + o + rnum + i]); } }
            pc += o * 3;
        }
        let kw_pos = len + kd;
        let blk_pos = kw_pos + 1;
        self.stack[base + blk_pos] = Slot::from(blk);
        if kd == 1 {
            if kdict.is_nil() { kdict = self.hash_new(); }
            self.stack[base + kw_pos] = Slot::from(kdict);
            kw = true;
        }
        for i in blk_pos + 1..nlocals { if base + i < self.stack.len() { self.stack[base + i] = Slot::NIL; } }
        self.ci[top].n = len as u8;
        self.ci[top].kw = kw;
        self.ci[top].pc = pc;
        Ok(())
    }

    fn op_blkpush(&mut self, base: usize, b: usize) -> VmResult<Value> {
        let m1 = (b >> 11) & 0x3f;
        let r = (b >> 10) & 1;
        let m2 = (b >> 5) & 0x1f;
        let kd = (b >> 4) & 1;
        let lv = b & 0xf;
        let offset = m1 + r + m2 + kd;
        let v = if lv == 0 { self.stack[base + 1 + offset].get() } else {
            match self.uvenv(lv - 1) { Some(e) if self.heap.env(e).len > offset + 1 => self.env_get(e, 1 + offset), _ => return Err(self.raise(self.core.local_jump_error, "unexpected yield")) }
        };
        if v.is_nil() { return Err(self.raise(self.core.local_jump_error, "unexpected yield")); }
        Ok(v)
    }

    /// Returns `Some(value)` when the loop should return to its caller.
    fn op_return(&mut self, v: Value, stop_depth: usize, lc: usize) -> VmResult<Option<Value>> {
        let top = self.ci.len() - 1;
        self.unwind_return(top, v, stop_depth, lc, UnwindBy::Return)
    }

    fn op_return_blk(&mut self, v: Value, stop_depth: usize, lc: usize) -> VmResult<Option<Value>> {
        let top = self.ci.len() - 1;
        let p = self.ci[top].proc_;
        let pd = self.heap.proc_data(p);
        if pd.env.is_none() || pd.strict { return self.op_return(v, stop_depth, lc); }
        // mruby `top_proc(proc, &env)`: walk `upper` until the method or lambda
        // that owns the locals. `env` ends as the environment captured by the
        // last block on the way, i.e. the environment *of the frame* running
        // that method/lambda (a frame's env is the one its blocks capture), so
        // it identifies the frame to return from.
        let mut cur = p;
        let mut env = pd.env;
        loop {
            let cd = self.heap.proc_data(cur);
            let up = match cd.upper { Some(u) => u, None => break };
            if cd.scope || cd.strict { break; }
            env = cd.env;
            cur = up;
        }
        let target_env = env;
        // a home frame in another fiber is not reachable (`dst->e.env->cxt == mrb->c`)
        if let Some(e) = target_env { let ed = self.heap.env(e); if ed.attached && ed.ctx != self.cur { return Err(self.raise(self.core.local_jump_error, "unexpected return")); } }
        let mut idx = None;
        for i in (0..self.ci.len()).rev() {
            if self.ci[i].env.is_some() && self.ci[i].env == target_env { idx = Some(i); break; }
        }
        match idx {
            Some(i) => self.unwind_return(i, v, stop_depth, lc, UnwindBy::Return),
            None => Err(self.raise(self.core.local_jump_error, "unexpected return")),
        }
    }

    fn op_break(&mut self, v: Value, stop_depth: usize, lc: usize) -> VmResult<Option<Value>> {
        let top = self.ci.len() - 1;
        let p = self.ci[top].proc_;
        let pd = self.heap.proc_data(p);
        if pd.strict { return self.op_return(v, stop_depth, lc); }
        let dst = match (pd.orphan, pd.env, pd.upper) { (false, Some(_), Some(u)) => u, _ => return Err(self.raise(self.core.local_jump_error, "break from proc-closure")) };
        // return from the frame whose *caller* runs `dst` (the method that received the block)
        let mut idx = None;
        for i in (1..self.ci.len()).rev() {
            if self.ci[i - 1].proc_ == dst { idx = Some(i); break; }
        }
        match idx {
            Some(i) => self.unwind_return(i, v, stop_depth, lc, UnwindBy::Break),
            None => Err(self.raise(self.core.local_jump_error, "break from proc-closure")),
        }
    }

    /// `OP_ARGARY`: rebuild the argument list of the enclosing method for `super` without arguments.
    fn op_argary(&mut self, base: usize, a: usize, b: usize) -> VmResult<()> {
        let m1 = (b >> 11) & 0x3f;
        let r = (b >> 10) & 1;
        let m2 = (b >> 5) & 0x1f;
        let kd = (b >> 4) & 1;
        let lv = b & 0xf;
        let ci = self.ci.last().unwrap().clone();
        if ci.mid.is_none() { return Err(self.raise(self.core.no_method_error, "super called outside of method")); }
        let get = |vm: &Vm, i: usize| -> VmResult<Value> {
            if lv == 0 { Ok(vm.stack.get(base + 1 + i).map(|s| s.get()).unwrap_or(Value::Nil)) } else {
                match vm.uvenv(lv - 1) {
                    Some(e) if vm.heap.env(e).len > m1 + r + m2 + 1 => Ok(vm.env_get(e, 1 + i)),
                    _ => Err(VmError::Raise(Value::Nil)), // replaced below
                }
            }
        };
        let mut args = Vec::with_capacity(m1 + m2 + 1);
        let fail = |vm: &mut Vm| vm.raise(vm.core.no_method_error, "super called outside of method");
        for i in 0..m1 { args.push(get(self, i).map_err(|_| fail(self))?); }
        if r == 1 {
            let rest = get(self, m1).map_err(|_| fail(self))?;
            if let Some(v) = self.ary_vals(rest) { args.extend(v); }
        }
        for i in 0..m2 { args.push(get(self, m1 + r + i).map_err(|_| fail(self))?); }
        let blk_or_kd = get(self, m1 + r + m2).map_err(|_| fail(self))?;
        let need = base + a + 3;
        if self.stack.len() < need { self.stack.resize(need, Slot::NIL); }
        self.stack[base + a] = Slot::from(self.ary_new(args));
        if kd == 1 {
            let blk = get(self, m1 + r + m2 + 1).map_err(|_| fail(self))?;
            self.stack[base + a + 1] = Slot::from(blk_or_kd);
            self.stack[base + a + 2] = Slot::from(blk);
        } else {
            self.stack[base + a + 1] = Slot::from(blk_or_kd);
        }
        Ok(())
    }

    // ------------------------------------------------------------------ error reporting

    /// mruby's `%T`: the receiver's class name (`NilClass` for nil, `Class` for a class).
    pub fn describe_for_error(&mut self, v: Value) -> String {
        let c = self.real_class_of(v);
        self.class_name(c)
    }
    /// mruby's `%Y`: `nil`/`true`/`false` for those, otherwise the class name.
    pub fn describe_for_type_error(&mut self, v: Value) -> String {
        match v {
            Value::Nil => "nil".into(),
            Value::True => "true".into(),
            Value::False => "false".into(),
            _ => self.describe_for_error(v),
        }
    }
    /// Human-readable description of an error (like mruby's `mrb_print_error`).
    pub fn describe_error(&mut self, e: &VmError) -> String {
        match e {
            VmError::Raise(exc) => {
                let cls = self.real_class_of(*exc);
                let cn = self.class_name(cls);
                let msg = self.exception_message(*exc);
                format!("{msg} ({cn})")
            }
            other => other.to_string(),
        }
    }
    pub fn exception_message(&mut self, exc: Value) -> String {
        if let Value::Obj(o) = exc {
            let m = self.heap.ivar_get(o, self.s.mesg);
            if let Some(b) = self.str_bytes(m) { return String::from_utf8_lossy(b).into_owned(); }
            let c = self.real_class_of(exc);
            return self.class_name(c);
        }
        "?".into()
    }
}

impl Default for Vm {
    fn default() -> Self { Vm::new() }
}

/// `MRB_CALL_LEVEL_MAX`.
pub const CALL_LEVEL_MAX: usize = 512;

/// The irep a native loop frame runs (`Vm::push_loop_frame`): `OP_DEBUG`, which the
/// instruction loop reads as "step the native loop of this frame", then `OP_RETURN` of
/// [`LOOP_RESULT`] ([`LoopNext::Tail`]). 0 is `call_proc`'s, 1 `ret_proc`'s.
pub(crate) const LOOP_IREP: IrepId = 2;
/// The register of a native loop frame where the block's frame sits, so where the block's value
/// lands. Below it: R0 the receiver, R1 the block, R2 the kind of loop, R3 how far it got
/// (0: not started, 1: a block's value is waiting in this register), and R4.. the loop's own
/// state — the most any loop keeps is `Array.new(n) { }`'s three (`builtins/array.rs`), so
/// R4..R6, which is what this number is.
pub(crate) const LOOP_RESULT: usize = 7;
/// Where the loop frame's `OP_RETURN` is ([`LoopNext::Tail`]).
const LOOP_TAIL_PC: usize = 4;

/// What the step of a native loop asks for next (`Vm::push_loop_frame`).
pub(crate) enum LoopNext {
    /// call the block with this many arguments, written above [`LOOP_RESULT`] (`Vm::loop_arg`)
    Call(usize),
    /// the loop is over, and the native answers this
    Done(Value),
    /// call the block with this many arguments, and answer what it answers (`catch`): the
    /// frame returns the block's value without another step
    Tail(usize),
}
/// Nested native -> VM re-entries allowed (each one uses host stack).
pub const NATIVE_DEPTH_MAX: u32 = 96;

#[inline]
fn jump(pc: usize, off: usize) -> usize {
    // 16-bit signed offset relative to the next instruction
    (pc as i64 + (off as u16 as i16) as i64) as usize
}

fn float_op(mid: Sym, s: Syms, p: f64, q: f64) -> Value {
    if mid == s.plus { Value::Float(p + q) } else if mid == s.minus { Value::Float(p - q) } else if mid == s.mul { Value::Float(p * q) } else { Value::Float(p / q) }
}

/// Renders an instruction listing in the style of `mrbc --verbose`.
pub fn dump(rite: &rite::Rite) -> String {
    let mut out = String::new();
    fn dump_irep(rite: &rite::Rite, i: usize, out: &mut String) {
        let ir = &rite.ireps[i];
        out.push_str(&format!("irep {} nregs={} nlocals={} pools={} syms={} reps={} ilen={}\n", i, ir.nregs, ir.nlocals, ir.pool.len(), ir.syms.len(), ir.reps.len(), ir.iseq.len()));
        for h in &ir.catch {
            out.push_str(&format!("catch type: {} begin: {:04} end: {:04} target: {:04}\n", match h.kind { CatchType::Rescue => "rescue", CatchType::Ensure => "ensure" }, h.begin, h.end, h.target));
        }
        let mut pc = 0;
        while pc < ir.iseq.len() {
            match ir.decode(pc) {
                Some((op, a, b, c, next)) => {
                    let lineno = match ir.line_of(pc) { Some(l) => format!("{l:5} "), None => "      ".into() };
                    let mut line = format!("{lineno}{:03} {}", pc, op.name());
                    match op.operands() {
                        Operands::Z => {}
                        Operands::B | Operands::S | Operands::W => line.push_str(&format!("\t{a}")),
                        Operands::BB | Operands::BS => line.push_str(&format!("\t{a}\t{b}")),
                        Operands::BBB | Operands::BSS => line.push_str(&format!("\t{a}\t{b}\t{c}")),
                    }
                    match op {
                        Op::Loadsym | Op::Getgv | Op::Setgv | Op::Getiv | Op::Setiv | Op::Getcv | Op::Setcv | Op::Getconst | Op::Setconst | Op::Getmcnst | Op::Setmcnst | Op::Send | Op::Sendb | Op::Send0 | Op::Ssend | Op::Ssendb | Op::Ssend0 | Op::Def | Op::Tdef | Op::Sdef | Op::Class | Op::Module => {
                            if let Some(Some(s)) = ir.syms.get(b as usize) { line.push_str(&format!("\t; :{}", String::from_utf8_lossy(s))); }
                        }
                        Op::String | Op::Loadl | Op::Symbol => {
                            if let Some(p) = ir.pool.get(b as usize) { line.push_str(&format!("\t; {p:?}")); }
                        }
                        Op::Jmp | Op::Jmpuw => line.push_str(&format!("\t; -> {}", jump(next, a as usize))),
                        Op::Jmpif | Op::Jmpnot | Op::Jmpnil => line.push_str(&format!("\t; -> {}", jump(next, b as usize))),
                        _ => {}
                    }
                    out.push_str(&line);
                    out.push('\n');
                    pc = next;
                }
                None => { out.push_str(&format!("  {:03} ??? 0x{:02x}\n", pc, ir.iseq[pc])); pc += 1; }
            }
        }
        for &r in &ir.reps {
            out.push('\n');
            dump_irep(rite, r, out);
        }
    }
    dump_irep(rite, rite.root, &mut out);
    out
}
