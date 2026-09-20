//! Declarations a script lays out, collected into a Rust table — and a Rust table read back
//! from Ruby.
//!
//! A host that wants its data *written* in Ruby rather than computed by it gives the VM a
//! method per kind of thing and lets a script call it:
//!
//! ```ruby
//! unit :metre, symbol: "m",  scale: 1.0
//! unit :inch,  symbol: "in", scale: 0.0254
//! ```
//!
//! [`Declarations<T>`] is that method's other half: the first argument is the name (a Symbol or
//! a String), the keyword arguments are the value — they arrive as a trailing Hash, which
//! [`from_value`](crate::from_value) reads as a `T` — and what the host gets back, once the
//! script has run, is a `Vec<(String, T)>` in the order the script declared them.
//!
//! ```
//! # fn main() -> Result<(), sabiruby::VmError> {
//! use sabiruby_serde::declare::Declarations;
//! use serde::Deserialize;
//!
//! #[derive(Deserialize)]
//! #[serde(deny_unknown_fields)]
//! struct Unit { symbol: String, scale: f64 }
//!
//! let mut vm = sabiruby::Vm::with_mrblib()?;
//! let units = Declarations::<Unit>::install(&mut vm).define(&mut vm, "unit");
//! # let src = sabiruby_compiler::compile(b"unit :metre, symbol: \"m\", scale: 1.0",
//! #     &sabiruby_compiler::Options { filename: "data.rb".into(), debug_info: true, ..Default::default() }).unwrap();
//! vm.load_and_run(&src)?;                       // the script above, compiled
//! let table: Vec<(String, Unit)> = units.take(&mut vm);
//! assert_eq!(table[0].0, "metre");
//! # Ok(()) }
//! ```
//!
//! # What is checked, and where the error says it is
//!
//! A declaration is deserialized inside the native, so everything serde refuses — a missing
//! field, a field of the wrong type, an unknown field under `#[serde(deny_unknown_fields)]` —
//! raises a `TypeError` at the line of that declaration, with the script's own file name:
//! `data.rb:2:in unit`. A name declared twice raises an `ArgumentError` at the second one
//! ([`OnDuplicate`] is what picks that), and the entry point that lets a later file amend an
//! earlier one is a *differently named* method ([`Declarations::define_replacing`]), so that
//! overwriting is something the script asks for rather than something it can do by accident.
//!
//! # Where the table lives while the script runs
//!
//! In the VM, in `T`'s [`HostStore`](sabiruby::host_store::HostStore) — one table per Rust
//! type per VM, found by `TypeId`. Not in
//! [`Vm::set_host_state`](sabiruby::Vm::set_host_state), which holds one value for the whole
//! VM and which an embedder (rubevy) may already be using; and not in a `static` or an
//! `Arc<Mutex<…>>`, which this crate, being `no_std`, has no lock for.
//!
//! The store is a slab meant for the values `Data` objects name, and this is that slab used
//! for one value that no `Data` object names: nothing is handed to Ruby, so nothing can be
//! collected, and the one thing the VM does with a store of its own — dropping an entry when
//! an object of its tag is collected — never fires, because no object carries the tag. What
//! it costs is that tag: one number of `Vm::next_data_tag`'s sequence per declared type.
//!
//! Nothing in the table is a Ruby value: names are `String`s and the entries are whatever
//! `T` deserializes into, so a collection has nothing to reach here and the table cannot
//! keep a Ruby object alive by accident. [`Declarations::take`] moves the whole table out of
//! the VM, and a declaration made after that raises rather than being silently dropped.

use alloc::borrow::ToOwned;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::marker::PhantomData;

use sabiruby::error::{VmError, VmResult};
use sabiruby::value::{ObjId, Value};
use sabiruby::Vm;
use serde::{Deserialize, Serialize};

use crate::{from_value, to_value_with, Options};

// ------------------------------------------------------------------ collecting

/// What a second declaration of a name already declared does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnDuplicate {
    /// Raise `ArgumentError`, at the line of the second declaration. What
    /// [`Declarations::define`] uses: a name means one thing, and two definitions of it are a
    /// mistake in the data unless the script says otherwise.
    Raise,
    /// Replace what is there, keeping the entry where it was in the order — a later file
    /// amending an earlier one. What [`Declarations::define_replacing`] uses.
    Replace,
}

/// The table a [`Declarations<T>`] fills: the entries in the order they were declared, and a
/// name index, so that "is this one already there?" and a look-up are not a scan of the lot.
struct Collected<T> {
    order: Vec<(String, T)>,
    at: BTreeMap<String, usize>,
}

/// A table of declarations being collected in a [`Vm`], and the methods that fill it.
///
/// It is a handle, not the table: `Copy`, and small enough to keep in a struct beside the VM.
/// The table itself is in the VM (see the [module's documentation](self)) until
/// [`take`](Declarations::take) moves it out.
pub struct Declarations<T> {
    handle: u64,
    // `fn() -> T` rather than `T`: the handle is `Send + Sync` whatever `T` is, and this
    // marker says the type is used, not owned.
    marker: PhantomData<fn() -> T>,
}

impl<T> Clone for Declarations<T> {
    fn clone(&self) -> Self { *self }
}
impl<T> Copy for Declarations<T> {}

impl<T> core::fmt::Debug for Declarations<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Declarations<{}>({})", core::any::type_name::<T>(), self.handle)
    }
}

impl<T: for<'de> Deserialize<'de> + Send + Sync + 'static> Declarations<T> {
    /// Makes the table in `vm`, with no method to fill it yet.
    ///
    /// Calling it twice for the same `T` makes a second, separate table; the methods of the
    /// first go on filling the first. A host that wants two tables of the same shape usually
    /// wants two Rust types instead, so that [`take`](Declarations::take) cannot be asked for
    /// the wrong one.
    pub fn install(vm: &mut Vm) -> Declarations<T> {
        vm.install_host_store::<Collected<T>>();
        let handle = vm
            .host_store_mut::<Collected<T>>()
            .expect("the store was just installed")
            .insert(Collected { order: Vec::new(), at: BTreeMap::new() });
        Declarations { handle, marker: PhantomData }
    }

    /// Defines `method` on `Object`, so a script calls it at the top level. A name declared
    /// twice raises `ArgumentError`.
    pub fn define(self, vm: &mut Vm, method: &str) -> Self {
        let object = vm.core.object;
        self.define_on(vm, object, method, OnDuplicate::Raise)
    }

    /// [`define`](Declarations::define) with the other answer to a name that is already
    /// there: this method replaces the entry, keeping its place in the order.
    ///
    /// It is a method of its own, and so a method of its own in Ruby, on purpose: the plain
    /// name refuses to overwrite, and a script that means to amend an earlier declaration says
    /// so by calling the other one.
    pub fn define_replacing(self, vm: &mut Vm, method: &str) -> Self {
        let object = vm.core.object;
        self.define_on(vm, object, method, OnDuplicate::Replace)
    }

    /// Both of the above, on a class or module of the host's choosing — a module's singleton
    /// class for `Data.unit …`, say, rather than a bare `unit …`.
    ///
    /// The method takes the name and, where the declaration has keyword arguments, the Hash
    /// they arrive in; it answers `nil`.
    pub fn define_on(self, vm: &mut Vm, class: ObjId, method: &str, on_duplicate: OnDuplicate) -> Self {
        let handle = self.handle;
        let called = method.to_owned();
        vm.define_closure(class, method, move |vm, _self_, args, _blk| {
            // 1 argument or 2: the keyword arguments arrive as a trailing Hash, and only when
            // there are any (`Vm::native_call_args`, src/vm.rs:4264)
            vm.check_argc(args, 1, 2)?;
            let name = name_of(vm, args[0])?;
            // the name is the identity of a declaration, so a duplicate is reported before
            // whatever is wrong inside the second one
            match table::<T>(vm, handle).map(|t| t.at.contains_key(&name)) {
                None => return Err(already_taken(vm, &called)),
                Some(true) if on_duplicate == OnDuplicate::Raise => {
                    return Err(vm.raise_arg(&format!("{called} {name:?} is already declared")));
                }
                _ => {}
            }
            let value: T = match args.get(1) {
                Some(kw) => from_value(vm, *kw)?,
                // no keyword arguments at all: the same as `{}`, so that a `T` whose fields
                // all have defaults needs no empty Hash in the script
                None => { let empty = vm.hash_new(); from_value(vm, empty)? }
            };
            // the table was there a moment ago and Ruby has no way to take it, but reading the
            // Hash can run Ruby (a key's `hash`/`eql?`), so this asks rather than asserts
            let t = match vm.host_store_mut::<Collected<T>>().and_then(|s| s.get_mut(handle)) {
                Some(t) => t,
                None => return Err(already_taken(vm, &called)),
            };
            match t.at.get(&name).copied() {
                Some(i) => t.order[i].1 = value,
                None => { t.at.insert(name.clone(), t.order.len()); t.order.push((name, value)); }
            }
            Ok(Value::Nil)
        });
        self
    }

    /// Moves the declarations out of the VM, in the order they were declared.
    ///
    /// Afterwards the VM holds none of them: the entries and the name index are gone, and a
    /// script that calls one of the methods raises `RuntimeError` rather than declaring into
    /// nothing. Asking twice answers the second time with an empty `Vec`.
    ///
    /// (The store keeps the *place* the table was in, which is what stops the number naming
    /// it from being handed out to a later table — see `HostStore::take`. What is left is one
    /// empty slot, holding nothing.)
    pub fn take(self, vm: &mut Vm) -> Vec<(String, T)> {
        match vm.host_store_mut::<Collected<T>>().and_then(|s| s.take(self.handle)) {
            Some(t) => t.order,
            None => Vec::new(),
        }
    }

    /// How many declarations have been collected so far; 0 once they have been taken.
    pub fn len(self, vm: &Vm) -> usize {
        table::<T>(vm, self.handle).map_or(0, |t| t.order.len())
    }

    /// Whether nothing has been declared (or the table has been taken).
    pub fn is_empty(self, vm: &Vm) -> bool { self.len(vm) == 0 }
}

fn table<T: Send + Sync + 'static>(vm: &Vm, handle: u64) -> Option<&Collected<T>> {
    vm.host_store::<Collected<T>>().and_then(|s| s.get(handle))
}

// -------------------------------------------------------------------- reading back

/// A Rust table a script can look up by name, put in a [`Vm`] by [`expose`].
pub struct Exposed<T> {
    handle: u64,
    marker: PhantomData<fn() -> T>,
}

impl<T> Clone for Exposed<T> {
    fn clone(&self) -> Self { *self }
}
impl<T> Copy for Exposed<T> {}

impl<T> core::fmt::Debug for Exposed<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Exposed<{}>({})", core::any::type_name::<T>(), self.handle)
    }
}

/// What [`expose`] keeps: the table, by name.
struct Table<T>(BTreeMap<String, T>);

/// Defines `method` on `Object`: a script calls it with a name (Symbol or String) and gets the
/// entry as a Hash with Symbol keys ([`Options::symbol_keys`]), or `nil` for a name the table
/// has not got.
///
/// This is the other direction from [`Declarations`], and deliberately a separate table: what
/// a script reads back is what the host decided to publish — after it has checked the
/// declarations, derived from them, or thrown some away — and it may be put in a different VM
/// from the one that declared them.
///
/// ```
/// # fn main() -> Result<(), sabiruby::VmError> {
/// use serde::Serialize;
/// #[derive(Serialize)]
/// struct Unit { symbol: String, scale: f64 }
///
/// let mut vm = sabiruby::Vm::with_mrblib()?;
/// sabiruby_serde::declare::expose(&mut vm, "unit_of", [
///     ("metre".to_string(), Unit { symbol: "m".into(), scale: 1.0 }),
/// ]);
/// # let src = sabiruby_compiler::compile(b"$s = unit_of(:metre)[:symbol]",
/// #     &sabiruby_compiler::Options { filename: "(doc)".into(), debug_info: true, ..Default::default() }).unwrap();
/// vm.load_and_run(&src)?;                    // `unit_of(:metre)[:symbol]` is "m"
/// # assert_eq!(vm.str_bytes(vm.global_get("$s")), Some(&b"m"[..]));
/// # Ok(()) }
/// ```
///
/// The table is a look-up by name, so the order the entries came in is not kept; the host has
/// that in whatever it built the table from. Two entries of the same name: the later wins.
pub fn expose<T, I>(vm: &mut Vm, method: &str, entries: I) -> Exposed<T>
where
    T: Serialize + Send + Sync + 'static,
    I: IntoIterator<Item = (String, T)>,
{
    let object = vm.core.object;
    expose_on(vm, object, method, entries)
}

/// [`expose`] on a class or module of the host's choosing.
pub fn expose_on<T, I>(vm: &mut Vm, class: ObjId, method: &str, entries: I) -> Exposed<T>
where
    T: Serialize + Send + Sync + 'static,
    I: IntoIterator<Item = (String, T)>,
{
    let map: BTreeMap<String, T> = entries.into_iter().collect();
    vm.install_host_store::<Table<T>>();
    let handle = vm
        .host_store_mut::<Table<T>>()
        .expect("the store was just installed")
        .insert(Table(map));
    let called = method.to_owned();
    vm.define_closure(class, method, move |vm, _self_, args, _blk| {
        vm.check_argc(args, 1, 1)?;
        let name = name_of(vm, args[0])?;
        // writing a value needs the `&mut Vm` that reading it out of the store is borrowing,
        // so the table is borrowed whole and given back, as `RubyClass::take_out` does for a
        // method's receiver. Nothing Ruby runs in between: serializing never calls back in.
        let t = match vm.host_store_mut::<Table<T>>().and_then(|s| s.take(handle)) {
            Some(t) => t,
            None => return Err(already_taken(vm, &called)),
        };
        let answer = match t.0.get(&name) {
            Some(v) => to_value_with(vm, v, Options::symbol_keys()),
            None => Ok(Value::Nil),
        };
        vm.host_store_mut::<Table<T>>().expect("the store is still there").restore(handle, t);
        answer
    });
    Exposed { handle, marker: PhantomData }
}

impl<T: Send + Sync + 'static> Exposed<T> {
    /// Moves the table out of the VM, by name. The method stays defined and raises
    /// `RuntimeError` from then on, as a taken [`Declarations`] does.
    pub fn take(self, vm: &mut Vm) -> Vec<(String, T)> {
        match vm.host_store_mut::<Table<T>>().and_then(|s| s.take(self.handle)) {
            Some(t) => t.0.into_iter().collect(),
            None => Vec::new(),
        }
    }

    /// How many entries the table has; 0 once it has been taken.
    pub fn len(self, vm: &Vm) -> usize {
        vm.host_store::<Table<T>>().and_then(|s| s.get(self.handle)).map_or(0, |t| t.0.len())
    }

    /// Whether the table is empty (or has been taken).
    pub fn is_empty(self, vm: &Vm) -> bool { self.len(vm) == 0 }
}

// -------------------------------------------------------------------------- shared

/// A declaration's name: a Symbol or a String, which is what a Ruby DSL writes. Anything else
/// is a `TypeError`, as is a String that is not UTF-8 — the same answer this crate gives
/// wherever it reads text (`docs/design/serde.md`, "The data model").
fn name_of(vm: &mut Vm, v: Value) -> VmResult<String> {
    if let Value::Sym(s) = v { return Ok(vm.sym_name(s)); }
    if let Some(bytes) = vm.str_bytes(v).map(|b| b.to_vec()) {
        return match String::from_utf8(bytes) {
            Ok(s) => Ok(s),
            Err(_) => Err(vm.raise_type("a name must be valid UTF-8")),
        };
    }
    let d = vm.describe_for_type_error(v);
    Err(vm.raise_type(&format!("wrong argument type {d} (expected Symbol or String)")))
}

/// The table is gone: the host took it while the VM was still able to call the method.
fn already_taken(vm: &mut Vm, method: &str) -> VmError {
    let c = vm.core.runtime_error;
    vm.raise(c, &format!("{method}: the host has already taken this table out of the VM"))
}
