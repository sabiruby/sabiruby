//! serde for [SabiRuby](https://crates.io/crates/sabiruby): a Rust value as a Ruby value and
//! back, and a Ruby `JSON` built on top of it.
//!
//! ```
//! # fn main() -> Result<(), sabiruby::VmError> {
//! use serde::{Deserialize, Serialize};
//! #[derive(Serialize, Deserialize, PartialEq, Debug)]
//! struct Config { name: String, retries: u32, verbose: bool }
//!
//! let mut vm = sabiruby::Vm::with_mrblib()?;
//! let cfg = Config { name: "a".into(), retries: 3, verbose: true };
//! let v = sabiruby_serde::to_value(&mut vm, &cfg)?;      // {"name" => "a", "retries" => 3, …}
//! let back: Config = sabiruby_serde::from_value(&mut vm, v)?;
//! assert_eq!(cfg, back);
//! # Ok(()) }
//! ```
//!
//! The VM itself does not depend on serde, and must not: it is `no_std` and its dependency
//! list is part of what it is for. This crate is where serde lives, and it is `no_std` too
//! (`alloc` only), so a bare-metal host can have it — the Ruby [`JSON`](json) class is behind
//! the default feature `json`, which is what pulls in `serde_json` and, through its
//! `preserve_order`, `std`.
//!
//! # The data model
//!
//! | serde | Ruby | back |
//! |---|---|---|
//! | `bool` | `true` / `false` | the same |
//! | `i8`…`i64`, `u8`…`u32` | Integer | an Integer that fits, else a `TypeError` |
//! | `u64`, `i128`, `u128` | Integer (wide when it must be) | the same |
//! | `f32` / `f64` | Float | Float, or an Integer widened |
//! | `char` | a one-character String | a String of exactly one character |
//! | `String`, `&str` | String | String or Symbol; not-UTF-8 is an error |
//! | bytes (`serde_bytes`) | String marked binary | any String, bytes unchanged |
//! | `None` / `Some(x)` | `nil` / `x` | `nil` is `None` |
//! | `()`, unit struct | `nil` | `nil` |
//! | seq, tuple, tuple struct | Array | Array |
//! | map | Hash (keys as they serialize; [`Options::symbol_map_keys`] for String keys as Symbols) | Hash |
//! | struct | Hash, String keys ([`Options::symbol_keys`] for Symbols) | Hash with String **or** Symbol keys |
//! | unit variant | Symbol (`:Red`) | Symbol or String |
//! | newtype variant | `{"Name" => value}` | a Hash of one entry |
//! | tuple variant | `{"Name" => [a, b]}` | a Hash of one entry |
//! | struct variant | `{"Name" => {"a" => …}}` | a Hash of one entry |
//!
//! Reading is deliberately the more forgiving direction: a struct's fields are found under
//! String or Symbol keys whichever way they were written, and a variant name may be a Symbol
//! or a String. Writing has one shape, which [`Options`] picks — [`Options::symbols`] is both
//! switches, the shape that reads back the way a Ruby data file wrote it.
//!
//! # Errors
//!
//! [`to_value`] and [`from_value`] answer with [`sabiruby::error::VmResult`]: a
//! failure is a raised Ruby exception, so a native that converts can `?` it like any other and
//! the script sees a normal `TypeError`. The type serde itself needs —
//! [`error::Error`], which can be built from a message with no VM in reach — is
//! converted at that boundary; [`error::Error::into_vm_error`] says how.
//!
//! # From a `define_fn` function
//!
//! [`Serde<T>`] is the wrapper that lets a typed function take and answer with serde types:
//!
//! ```
//! # fn main() -> Result<(), sabiruby::VmError> {
//! # use serde::{Deserialize, Serialize};
//! # #[derive(Serialize, Deserialize)] struct Config { name: String }
//! use sabiruby_serde::Serde;
//! let mut vm = sabiruby::Vm::with_mrblib()?;
//! let object = vm.core.object;
//! vm.define_fn(object, "configure", |cfg: Serde<Config>| cfg.0.name);
//! # Ok(()) }
//! ```
//!
//! # Declarations
//!
//! [`declare`] is the same conversion put to a particular use: data *written* in Ruby —
//! `unit :metre, symbol: "m", scale: 1.0`, a line per entry — collected into a
//! `Vec<(String, T)>` the host takes once the script has run, with every field serde refuses
//! raising at the line of that declaration. [`declare::expose`] is the way back: a host table
//! a script looks up by name.
//!
//! # GC
//!
//! The values built here are not collector roots, exactly as in
//! [`sabiruby::convert`]: nothing is collected while a native is on the
//! host stack or between runs, so a conversion is safe as it stands, but a `Value` a host
//! keeps across a call into the VM must be registered with
//! [`Vm::gc_register`](sabiruby::Vm::gc_register).

#![no_std]

extern crate alloc;

pub mod de;
pub mod declare;
pub mod error;
#[cfg(feature = "json")]
pub mod json;
pub mod ser;

use sabiruby::convert::{FromRuby, IntoRuby};
use sabiruby::error::{VmError, VmResult};
use sabiruby::value::Value;
use sabiruby::Vm;
use serde::{Deserialize, Serialize};

pub use error::Error;
#[cfg(feature = "json")]
pub use json::install_json;

/// How a Rust value is written as a Ruby one.
///
/// Make one with [`default`](Options::default) or one of the constructors below and set the
/// fields you want; the struct is `#[non_exhaustive]`, so a struct literal is not available
/// outside this crate and a field added here later is not a breaking change:
///
/// ```
/// let mut opts = sabiruby_serde::Options::symbol_keys();
/// opts.symbol_map_keys = true;                      // the same as `Options::symbols()`
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Options {
    /// Write struct fields and variant names as Symbols (`{name: "a"}`) rather than as Strings
    /// (`{"name" => "a"}`). Off by default, because a Hash with String keys is what `JSON` and
    /// most Ruby data has, and reading accepts either way round.
    pub symbol_keys: bool,
    /// Write a *map's* keys as Symbols where they serialize as Strings (`BTreeMap<String, _>`
    /// becomes `{iron_ore: 1}` rather than `{"iron_ore" => 1}`). A key that serializes as
    /// anything else — an Integer, an Array — is left as it is.
    ///
    /// A separate switch from [`symbol_keys`](Options::symbol_keys), and off by default, for a
    /// reason that is not symmetry: a struct's field names are finite and the type decides
    /// them, while a map's keys are runtime values with no bound on how many there are, and
    /// the VM's symbol table is never collected (`docs/design/gc.md`: "symbols — never
    /// collected"). Interning every key of every map is therefore something a host chooses,
    /// not something it gets by asking for Symbol field names.
    pub symbol_map_keys: bool,
}

impl Options {
    /// Struct fields and variant names as Symbols; a map's keys as they serialize.
    pub fn symbol_keys() -> Options { Options { symbol_keys: true, symbol_map_keys: false } }
    /// Both: struct fields, variant names, and a map's String keys as Symbols. What
    /// [`declare::expose`] answers with, and what reads back the way a Ruby data file wrote
    /// it — `recipe_of(:iron_plate)[:in][:iron_ore]` rather than `[:in]["iron_ore"]`.
    pub fn symbols() -> Options { Options { symbol_keys: true, symbol_map_keys: true } }
}

/// A `Serialize` value as a Ruby value, with String keys.
pub fn to_value<T: ?Sized + Serialize>(vm: &mut Vm, value: &T) -> VmResult<Value> {
    to_value_with(vm, value, Options::default())
}

/// [`to_value`] with the keys written as [`Options`] says.
pub fn to_value_with<T: ?Sized + Serialize>(vm: &mut Vm, value: &T, opts: Options) -> VmResult<Value> {
    let mut ser = ser::Serializer::new(&mut *vm, opts);
    match value.serialize(&mut ser) {
        Ok(v) => Ok(v),
        Err(e) => Err(e.into_vm_error(vm)),
    }
}

/// A Ruby value read as a `Deserialize` type.
pub fn from_value<T: for<'de> Deserialize<'de>>(vm: &mut Vm, value: Value) -> VmResult<T> {
    let d = de::Deserializer::new(&mut *vm, value);
    match T::deserialize(d) {
        Ok(t) => Ok(t),
        Err(e) => Err(e.into_vm_error(vm)),
    }
}

/// A serde type in a [`Vm::define_fn`](sabiruby::Vm::define_fn) signature:
/// `|cfg: Serde<Config>| …` takes a Ruby Hash as a `Config`, and answering with
/// `Serde(report)` gives Ruby the Hash.
///
/// # Why there is a wrapper rather than a blanket impl
///
/// The obvious shape — `impl<T: Serialize> IntoRuby for T` — cannot be written. The VM already
/// has `impl IntoRuby for i64`, `for String`, `for Vec<T>` and a dozen more, and `i64` is
/// `Serialize`; a blanket impl would overlap every one of them, which the coherence rules
/// forbid (and specialization, which would settle it, is not stable). The same goes for
/// `FromRuby`. A wrapper type this crate owns has no such overlap, and it says at the call
/// site which conversion is meant, which is worth something of its own: `String` through
/// `FromRuby` is the String's bytes, while `Serde<String>` is a JSON-ish value that happens to
/// be a string, and the two should not be spelled the same.
///
/// # What the answer side cannot do
///
/// [`IntoRuby::into_ruby`] answers with a `Value` and has no way to raise, but serializing can
/// fail — not in this crate's serializer, which only ever fails when the VM does, but in a
/// `Serialize` written by hand that calls `Error::custom`. When that happens, `Serde<T>`
/// answers with **the exception object** rather than with data, so the failure is visible
/// (`vm.is_exception(v)`, and in Ruby `raise v`) instead of silently becoming `nil`. A
/// function that wants the ordinary raise should answer with `VmResult<Value>` and call
/// [`to_value`] itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Serde<T>(pub T);

impl<T> core::ops::Deref for Serde<T> {
    type Target = T;
    fn deref(&self) -> &T { &self.0 }
}

impl<T: for<'de> Deserialize<'de>> FromRuby for Serde<T> {
    fn from_ruby(vm: &mut Vm, v: Value) -> VmResult<Serde<T>> {
        from_value(vm, v).map(Serde)
    }
}

impl<T: Serialize> IntoRuby for Serde<T> {
    fn into_ruby(self, vm: &mut Vm) -> Value {
        match to_value(vm, &self.0) {
            Ok(v) => v,
            // the conversion already built the exception; hand it over as the answer
            Err(VmError::Raise(e)) => e,
            Err(e) => { let m = alloc::format!("{e}"); vm.exc_new(vm.core.runtime_error, &m) }
        }
    }
}
