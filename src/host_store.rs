//! Where the host keeps the values Ruby only holds a handle to.
//!
//! [`Vm::data_new`] hands Ruby a `(tag, handle)` pair and never reads through it: what the
//! handle names is the host's business (`docs/design/gc.md`). [`HostStore<T>`] is the usual
//! answer to "whose business, exactly" — a slab of `T`s indexed by the handle, with the
//! numbers of the removed ones handed out again:
//!
//! ```
//! # fn main() -> Result<(), sabiruby::VmError> {
//! use sabiruby::host_store::HostStore;
//! let mut store = HostStore::new();
//! let a = store.insert("ann");
//! let b = store.insert("bob");
//! assert_eq!(store.get(a), Some(&"ann"));
//! assert_eq!(store.remove(b), Some("bob"));
//! assert_eq!(store.get(b), None);
//! assert_eq!(store.insert("cid"), b);   // bob's number, given out again
//! # Ok(()) }
//! ```
//!
//! A [`Vm`] carries one store per type, found by [`TypeId`] rather than by a name the host has
//! to keep ([`Vm::host_store`], [`Vm::install_host_store`]); [`Vm::set_host_state`] holds one
//! value only, which several kinds of host object would have to share. The VM does one thing
//! with a store it carries: when a `Data` object of that store's tag is collected, it drops
//! what the handle named, before the host's own hook ([`Vm::set_on_free`]) is told. That is
//! the whole reason the stores are in the VM rather than beside it — the free hook is handed
//! numbers and no `&mut Vm`, so a store outside would have to be shared through a lock, which
//! `no_std` has none of.
//!
//! [`RubyClass`] ties the three together: a Rust type, its Ruby class, and the store its
//! values live in. `#[derive(RubyClass)]` of the `sabiruby-macros` crate implements it, and
//! host code that registers a class by hand can implement it in four lines
//! (`docs/design/macros.md`).

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::{Any, TypeId};

use crate::error::{VmError, VmResult};
use crate::value::{ObjId, Value};
use crate::vm::Vm;

// ---------------------------------------------------------------- HostStore

/// A slab of values the host owns: `insert` gives out the handle Ruby carries, `get` and
/// `get_mut` read it back, `remove` gives the number up for reuse.
///
/// Handles are small consecutive numbers rather than addresses, which is what makes a stale
/// one harmless: it names the wrong entry or none at all, and never a freed allocation. That
/// also means a number is handed out again after a `remove`, so a handle kept behind the
/// store's back can come to name a later value — keep them only in Ruby objects, whose
/// collection is what removes the entry.
pub struct HostStore<T> {
    slots: Vec<Cell<T>>,
    free: Vec<u64>,
    live: usize,
}

/// What a handle's place in the slab holds: a value, nothing, or nothing *yet* — a value
/// [`HostStore::take`] lent out to a call and is waiting to have back.
enum Cell<T> {
    Empty,
    Full(T),
    Out,
}

impl<T> Default for HostStore<T> {
    fn default() -> Self { HostStore::new() }
}

impl<T> HostStore<T> {
    /// An empty store.
    pub const fn new() -> Self { HostStore { slots: Vec::new(), free: Vec::new(), live: 0 } }

    /// Puts a value in and answers with its handle.
    pub fn insert(&mut self, value: T) -> u64 {
        self.live += 1;
        match self.free.pop() {
            Some(h) => { self.slots[h as usize] = Cell::Full(value); h }
            None => { self.slots.push(Cell::Full(value)); (self.slots.len() - 1) as u64 }
        }
    }

    /// The value `handle` names, or `None` if nothing does.
    pub fn get(&self, handle: u64) -> Option<&T> {
        match self.slots.get(handle as usize) {
            Some(Cell::Full(t)) => Some(t),
            _ => None,
        }
    }

    /// The value `handle` names, to change in place.
    pub fn get_mut(&mut self, handle: u64) -> Option<&mut T> {
        match self.slots.get_mut(handle as usize) {
            Some(Cell::Full(t)) => Some(t),
            _ => None,
        }
    }

    /// Whether `handle` names a value.
    pub fn contains(&self, handle: u64) -> bool { self.get(handle).is_some() }

    /// Whether `handle`'s value is out on loan ([`HostStore::take`]), rather than absent. What
    /// tells "someone is in the middle of a call on this object" from "this handle is stale".
    pub fn is_out(&self, handle: u64) -> bool {
        matches!(self.slots.get(handle as usize), Some(Cell::Out))
    }

    /// Takes the value out and gives its number up for reuse. This is what the VM does for a
    /// store of its own when the Ruby object naming the value is collected.
    ///
    /// A value that is out on loan is not removed and its number not given up: the call that
    /// has it will put it back. (The collector cannot reach that case — the object whose
    /// method is running is a root — so the entry is not leaked in practice.)
    pub fn remove(&mut self, handle: u64) -> Option<T> {
        let slot = self.slots.get_mut(handle as usize)?;
        if !matches!(slot, Cell::Full(_)) { return None; }
        let value = match core::mem::replace(slot, Cell::Empty) { Cell::Full(t) => t, _ => unreachable!() };
        self.free.push(handle);
        self.live -= 1;
        Some(value)
    }

    /// Lends the value out, keeping its number reserved until it is
    /// [`restore`](HostStore::restore)d: the entry answers [`is_out`](HostStore::is_out) in
    /// between, so a second `take` of the same handle answers `None` rather than handing the
    /// same value out twice.
    ///
    /// This is how a method that is given the `&mut Vm` borrows its receiver: the value cannot
    /// stay borrowed from the store while the VM it is stored in is being used, so it moves
    /// out for the duration of the call and back afterwards.
    ///
    /// A value may also be taken and **never restored**, which closes the entry for good: the
    /// number stays reserved (it is not returned to the free list the way
    /// [`remove`](HostStore::remove) returns it), so nothing that still holds the handle can
    /// reach a later value through it — every further `take` answers `None`. `sabiruby-serde`'s
    /// `Declarations::take` uses it that way, to hand a finished table to the host while the
    /// natives that filled it still hold its number. What it costs is the one empty slot.
    pub fn take(&mut self, handle: u64) -> Option<T> {
        let slot = self.slots.get_mut(handle as usize)?;
        if !matches!(slot, Cell::Full(_)) { return None; }
        let value = match core::mem::replace(slot, Cell::Out) { Cell::Full(t) => t, _ => unreachable!() };
        self.live -= 1;
        Some(value)
    }

    /// Puts back what [`take`](HostStore::take) lent out. A handle that is not waiting for one
    /// drops the value instead of overwriting what is there.
    pub fn restore(&mut self, handle: u64, value: T) {
        if let Some(slot @ Cell::Out) = self.slots.get_mut(handle as usize) {
            *slot = Cell::Full(value);
            self.live += 1;
        }
    }

    /// How many values are in the store, not counting one that is out on loan.
    pub fn len(&self) -> usize { self.live }
    /// Whether the store holds nothing.
    pub fn is_empty(&self) -> bool { self.live == 0 }

    /// Every value and its handle, in handle order.
    pub fn iter(&self) -> impl Iterator<Item = (u64, &T)> {
        self.slots.iter().enumerate().filter_map(|(i, s)| match s {
            Cell::Full(t) => Some((i as u64, t)),
            _ => None,
        })
    }
    /// Every value and its handle, to change in place.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (u64, &mut T)> {
        self.slots.iter_mut().enumerate().filter_map(|(i, s)| match s {
            Cell::Full(t) => Some((i as u64, t)),
            _ => None,
        })
    }
}

impl<T> core::fmt::Debug for HostStore<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "HostStore<{}>({} live)", core::any::type_name::<T>(), self.live)
    }
}

/// A store the VM can drop an entry from without knowing what is in it: the one thing the VM
/// does with the stores it carries (see the module's documentation).
pub(crate) trait AnyStore: Any + Send + Sync {
    fn drop_handle(&mut self, handle: u64);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Send + Sync + 'static> AnyStore for HostStore<T> {
    fn drop_handle(&mut self, handle: u64) { self.remove(handle); }
    fn as_any(&self) -> &dyn Any { self }
    fn as_any_mut(&mut self) -> &mut dyn Any { self }
}

/// One type's store in a [`Vm`], with the `Data` tag its objects carry.
pub(crate) struct HostStoreEntry {
    pub(crate) tag: u32,
    pub(crate) type_id: TypeId,
    pub(crate) store: Box<dyn AnyStore>,
}

impl Vm {
    /// A `Data` tag no other type has taken, counting up from 1.
    ///
    /// A tag says what kind of thing a handle names ([`Vm::data_new`]). A host that gives out
    /// its own numbers by hand may take them from here too, so that the two schemes do not
    /// collide.
    pub fn next_data_tag(&mut self) -> u32 {
        self.next_tag += 1;
        self.next_tag - 1
    }

    /// Gives `T` a [`HostStore`] in this VM if it has none, and answers with the `Data` tag of
    /// its objects. Calling it again answers with the same tag and keeps what is stored.
    ///
    /// ```
    /// # fn main() -> Result<(), sabiruby::VmError> {
    /// struct Player { hp: i64 }
    /// let mut vm = sabiruby::Vm::with_mrblib()?;
    /// let tag = vm.install_host_store::<Player>();
    /// let h = vm.host_store_mut::<Player>().unwrap().insert(Player { hp: 100 });
    /// let class = vm.define_class("Player", vm.core.object);
    /// let obj = vm.data_new(class, tag, h);      // what Ruby holds
    /// assert_eq!(vm.data_of(obj), Some((tag, h)));
    /// # Ok(()) }
    /// ```
    pub fn install_host_store<T: Send + Sync + 'static>(&mut self) -> u32 {
        let id = TypeId::of::<T>();
        if let Some(e) = self.host_stores.iter().find(|e| e.type_id == id) { return e.tag; }
        let tag = self.next_data_tag();
        self.host_stores.push(HostStoreEntry { tag, type_id: id, store: Box::new(HostStore::<T>::new()) });
        tag
    }

    /// `T`'s store in this VM, when [`Vm::install_host_store`] has made one.
    pub fn host_store<T: Send + Sync + 'static>(&self) -> Option<&HostStore<T>> {
        let id = TypeId::of::<T>();
        self.host_stores.iter().find(|e| e.type_id == id)
            .and_then(|e| e.store.as_any().downcast_ref::<HostStore<T>>())
    }

    /// `T`'s store in this VM, to put values in and take them out.
    pub fn host_store_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut HostStore<T>> {
        let id = TypeId::of::<T>();
        self.host_stores.iter_mut().find(|e| e.type_id == id)
            .and_then(|e| e.store.as_any_mut().downcast_mut::<HostStore<T>>())
    }

    /// The `Data` tag of `T`'s objects, when it has a store here.
    pub fn host_store_tag<T: Send + Sync + 'static>(&self) -> Option<u32> {
        let id = TypeId::of::<T>();
        self.host_stores.iter().find(|e| e.type_id == id).map(|e| e.tag)
    }

    /// Drops what a collected `Data` object named, when its tag belongs to a store of ours.
    /// Called from `gc_collect`, before the host's own free hook.
    pub(crate) fn host_store_free(&mut self, tag: u32, handle: u64) {
        if let Some(e) = self.host_stores.iter_mut().find(|e| e.tag == tag) {
            e.store.drop_handle(handle);
        }
    }
}

// ---------------------------------------------------------------- RubyClass

/// A Rust type that is a Ruby class: its name, its [`HostStore`], and the conversions between
/// a value of it and the Ruby object that names one.
///
/// `#[derive(RubyClass)]` of the `sabiruby-macros` crate implements it, and generated method
/// bodies are written in terms of it; by hand it is four lines, the rest being provided:
///
/// ```
/// # fn main() -> Result<(), sabiruby::VmError> {
/// use sabiruby::host_store::RubyClass;
/// use sabiruby::{IntoRuby, Value, Vm};
///
/// struct Player { hp: i64 }
/// impl RubyClass for Player { const NAME: &'static str = "Player"; }
/// impl IntoRuby for Player {
///     fn into_ruby(self, vm: &mut Vm) -> Value { self.into_handle(vm) }
/// }
///
/// let mut vm = Vm::with_mrblib()?;
/// Player::register_class(&mut vm);
/// let player = Player { hp: 100 }.into_ruby(&mut vm);
/// assert_eq!(Player::borrow(&mut vm, player)?.hp, 100);
/// # Ok(()) }
/// ```
///
/// The store is the VM's, so the same type can be used from several VMs at once without
/// sharing anything between them, and a value is dropped when the Ruby object naming it is
/// collected.
pub trait RubyClass: Sized + Send + Sync + 'static {
    /// The name of the Ruby class, `"Player"` for a `Player`.
    const NAME: &'static str;

    /// The Ruby class, defined under `Object` on first use.
    fn register_class(vm: &mut Vm) -> ObjId {
        let object = vm.core.object;
        vm.define_class(Self::NAME, object)
    }

    /// The `Data` tag of this type's objects, giving it a store on first use.
    fn tag(vm: &mut Vm) -> u32 { vm.install_host_store::<Self>() }

    /// This type's store, made on first use.
    fn store(vm: &mut Vm) -> &mut HostStore<Self> {
        vm.install_host_store::<Self>();
        vm.host_store_mut::<Self>().expect("the store was just installed")
    }

    /// Puts a value in the store and answers with the Ruby object that names it. This is what
    /// a derived `IntoRuby` does, and so what a method answering with `Self` does.
    fn into_handle(self, vm: &mut Vm) -> Value {
        let class = Self::register_class(vm);
        let tag = Self::tag(vm);
        let handle = Self::store(vm).insert(self);
        vm.data_new(class, tag, handle)
    }

    /// The handle in `v`, which must be a `Data` object of this type's tag; anything else is a
    /// `TypeError` naming the class (a `Player` method reached through `instance_exec` on a
    /// String, say, or a handle of another host type).
    ///
    /// An instance of this very class with no handle in it — `Player.allocate`, which makes the
    /// object without running `initialize`, so nothing was ever put in the store — is the one
    /// case worth its own sentence, because "wrong argument type Player (expected Player)" reads
    /// like a bug in the VM rather than like the answer, which is that this `Player` has no Rust
    /// value behind it. mruby says the same thing in the same place (`mrb_data_check_type`,
    /// src/etc.c: `DATA_TYPE` is NULL for an allocated-but-uninitialized object and the message
    /// is `uninitialized %t (expected %s)`), and it names the object's own class, so a subclass
    /// of `Player` reads `uninitialized Ghost (expected Player)`.
    fn handle_of(vm: &mut Vm, v: Value) -> VmResult<u64> {
        let tag = Self::tag(vm);
        if let Some((t, h)) = vm.data_of(v) {
            if t == tag { return Ok(h); }
            let d = vm.describe_for_type_error(v);
            return Err(vm.raise_type(&alloc::format!("wrong argument type {d} (expected {})", Self::NAME)));
        }
        let class = Self::register_class(vm);
        let d = vm.describe_for_type_error(v);
        if vm.obj_is_kind_of(v, class) {
            let name = Self::NAME;
            return Err(vm.raise_type(&alloc::format!(
                "uninitialized {d} (expected {name}): the object has no {name} behind it — `{name}.allocate` makes one without running `initialize`")));
        }
        Err(vm.raise_type(&alloc::format!("wrong argument type {d} (expected {})", Self::NAME)))
    }

    /// The value `v` names, borrowed from the store.
    fn borrow(vm: &mut Vm, v: Value) -> VmResult<&Self> {
        let h = Self::handle_of(vm, v)?;
        if let Some(e) = missing::<Self>(vm, h, Self::NAME, vm.host_store::<Self>().map(|s| s.get(h).is_some())) { return Err(e); }
        Ok(vm.host_store::<Self>().and_then(|s| s.get(h)).expect("just checked"))
    }

    /// The value `v` names, to change in place.
    fn borrow_mut(vm: &mut Vm, v: Value) -> VmResult<&mut Self> {
        let h = Self::handle_of(vm, v)?;
        if let Some(e) = missing::<Self>(vm, h, Self::NAME, vm.host_store::<Self>().map(|s| s.get(h).is_some())) { return Err(e); }
        Ok(vm.host_store_mut::<Self>().and_then(|s| s.get_mut(h)).expect("just checked"))
    }

    /// Moves the value `v` names out of the store, for a method that is given the `&mut Vm`
    /// and so cannot keep a borrow of it. The handle stays reserved and the caller must
    /// [`give_back`](RubyClass::give_back) the value; until it does, another call on the same
    /// object raises rather than seeing a half-borrowed value.
    fn take_out(vm: &mut Vm, v: Value) -> VmResult<(u64, Self)> {
        let h = Self::handle_of(vm, v)?;
        match vm.host_store_mut::<Self>().and_then(|s| s.take(h)) {
            Some(t) => Ok((h, t)),
            None if vm.host_store::<Self>().is_some_and(|s| s.is_out(h)) => Err(in_use(vm, Self::NAME)),
            None => Err(stale(vm, Self::NAME)),
        }
    }

    /// Puts back what [`take_out`](RubyClass::take_out) moved out.
    fn give_back(vm: &mut Vm, handle: u64, value: Self) {
        if let Some(s) = vm.host_store_mut::<Self>() { s.restore(handle, value); }
    }
}

/// Why the handle names nothing: a call on the same object has the value out on loan, or the
/// host gave it up. `held` is whether the store answered with a value at all.
fn missing<T: RubyClass>(vm: &mut Vm, handle: u64, name: &str, held: Option<bool>) -> Option<VmError> {
    if held == Some(true) { return None; }
    if vm.host_store::<T>().is_some_and(|s| s.is_out(handle)) { return Some(in_use(vm, name)); }
    Some(stale(vm, name))
}

fn stale(vm: &mut Vm, name: &str) -> VmError {
    let c = vm.core.runtime_error;
    vm.raise(c, &alloc::format!("stale {name} handle: the host dropped the value"))
}

fn in_use(vm: &mut Vm, name: &str) -> VmError {
    let c = vm.core.runtime_error;
    vm.raise(c, &alloc::format!("{name} is already in use by a call on the same object"))
}
