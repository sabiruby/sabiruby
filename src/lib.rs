//! SabiRuby — a virtual machine for mruby 4.1 bytecode, in Rust (checked against mruby
//! 4.1.0-rc2; 4.1.0 itself is not released yet).
//!
//! The VM executes RITE 0400 binaries (`.mrb` files) produced by mruby 4.1's `mrbc`. This
//! crate is pure Rust and `no_std`; to compile Ruby source in the same program, add the
//! companion crate [`sabiruby-compiler`](https://crates.io/crates/sabiruby-compiler) (the
//! reference compiler built as C). The `sabiruby` command is the crate
//! [`sabiruby-cli`](https://crates.io/crates/sabiruby-cli). Behaviour is checked against the
//! reference mruby 4.1.0-rc2 (its own test suite passes 2344 of 2508 assertions; see the
//! repository's README for what is missing). The design follows the book *Deep dive into
//! mruby* (register layout, callinfo, catch handlers, environments) and replaces mruby's
//! C-side choices (boxing, tricolor GC, setjmp) with Rust-native ones.
//!
//! # Usage
//!
//! ```
//! # fn run(bytes: &[u8]) -> Result<(), sabiruby::VmError> {
//! let mut vm = sabiruby::Vm::with_mrblib()?; // core classes + mruby's mrblib
//! vm.load_and_run(bytes)?;                   // a RITE binary, run to completion
//! let out = vm.take_output();                // what puts/p/print wrote
//! # let _ = out; Ok(()) }
//! ```
//!
//! Stepped execution, e.g. once per frame of a game loop:
//!
//! ```
//! # fn run(bytes: &[u8]) -> Result<(), sabiruby::VmError> {
//! use sabiruby::Step;
//! let mut vm = sabiruby::Vm::with_mrblib()?;
//! let irep = vm.load(bytes)?;
//! vm.start(irep);
//! loop {
//!     match vm.step(10_000)? {        // at most 10 000 instructions
//!         Step::Paused => { /* next frame */ }
//!         Step::Finished(_) => break,
//!     }
//! }
//! # Ok(()) }
//! ```
//!
//! Native methods are `fn(&mut Vm, self, args, block) -> VmResult<Value>` registered with
//! [`Vm::define_method`]. A native may keep values in Rust locals while it runs; objects kept
//! across calls into the VM must be registered with [`Vm::gc_register`] (see the repository's
//! `docs/design/gc.md`).
//!
//! # Features
//!
//! The library is `no_std` + `alloc` (it builds for bare-metal targets and
//! `wasm32-unknown-unknown`). The default `std` feature only adds `std::error::Error` for
//! [`VmError`]; `default-features = false` gives the `no_std` library.
//!
//! `utf8` (also default) reads a String as a sequence of characters, as the reference does
//! when it is built with `MRB_UTF8_STRING`: `length`, `[]`, `index`, `chars`, `reverse` and
//! the case methods count and cut characters, while `bytesize`, `byteslice`, `byteindex` and
//! friends stay bytes. Dropping it gives the byte-string build the reference ships. Both are
//! checked against a reference image of their own (the repository's `docs/design/utf8.md`).
//!
//! `regexp` (also default) is mruby-regexp: `Regexp`, `MatchData`, the String and Symbol
//! methods whose pattern form the gem answers, and `regex-automata` under them. Without it the
//! crate does not depend on `regex-automata` at all and the constant `Regexp` does not exist,
//! which is the reference built without the gem.
//!
//! `macros` (not default) re-exports `#[derive(RubyClass)]` and `#[ruby_methods]` from
//! `sabiruby-macros` at this crate's root, so that
//! `sabiruby = { version = "0.5", features = ["macros"] }` and
//! `use sabiruby::{RubyClass, ruby_methods};` are all a host writes. The macros are a proc
//! macro — a compiler plugin built for the host — so the feature is off by default and the
//! `no_std` library builds for a target without one.
//!
//! # Stability
//!
//! 0.x: the API follows the VM's internals and will change between minor versions. Items
//! hidden from these docs are internal even where they are `pub`.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

/// This crate's version, e.g. `"0.3.0"`. What a host prints to say which VM is running.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The commit this VM was built from, e.g. `"1fe5f9d77099ee4389d8451b1369a0de0e2d544c"`, or
/// `"HEAD"` where the build had no git (a crates.io build, as the reference's own default is).
/// It is what Ruby reads as `MRUBY_REVISION`.
pub const REVISION: &str = match option_env!("SABIRUBY_REVISION") {
    Some(r) => r,
    None => "HEAD",
};

pub mod bigint;
pub mod convert;
pub mod error;
pub mod host;
pub mod host_store;
pub mod object;
pub mod opcode;
#[doc(hidden)]
#[cfg(feature = "regexp")]
pub mod regexp;
pub mod rite;
pub mod symbol;
pub mod value;
pub mod vm;
pub mod inspect;
#[doc(hidden)]
pub mod builtins;
/// Runner for mruby's own test suite (used by `sabiruby mrbtest` and the crate's tests).
#[doc(hidden)]
pub mod mrbtest;

pub use convert::{FromRuby, IntoRuby};
pub use host_store::{HostStore, RubyClass};
pub use error::VmError;
pub use host::{EvalOptions, Host};
pub use value::Value;
pub use vm::{RunLimits, Step, Timeslice, Vm};

/// `#[derive(RubyClass)]` — a Rust struct as a Ruby class — with the feature `macros`,
/// re-exported from [`sabiruby-macros`](https://crates.io/crates/sabiruby-macros).
///
/// This is the *macro*; [`RubyClass`](trait@RubyClass) just above is the trait it implements,
/// which is what a `borrow` needs in scope. A macro and a type live in different namespaces, so
/// one `use` brings both, the way `use serde::Serialize;` does.
///
/// ```
/// use sabiruby::{RubyClass, ruby_methods};
///
/// #[derive(RubyClass)]
/// struct Player { hp: i64 }
///
/// #[ruby_methods]
/// impl Player {
///     fn new(hp: i64) -> Self { Player { hp } }        // Player.new(100)
///     fn damage(&mut self, n: i64) { self.hp -= n; }   // player.damage(10)
///     fn hp(&self) -> i64 { self.hp }                  // player.hp
/// }
/// // Player::register(&mut vm)?;   the class, the store, the methods
/// ```
#[cfg(feature = "macros")]
pub use sabiruby_macros::RubyClass;

/// `#[ruby_methods]` — an inherent `impl` block as the class's methods, and the `register` that
/// installs them — with the feature `macros`, re-exported from `sabiruby-macros`. The example is
/// on [`RubyClass`](macro@RubyClass).
#[cfg(feature = "macros")]
pub use sabiruby_macros::ruby_methods;

/// mruby's core library written in Ruby (`mrblib/*.rb` of 4.1.0-rc), compiled
/// with the reference `mrbc`. Loaded by [`Vm::with_mrblib`].
pub const MRBLIB_MRB: &[u8] = include_bytes!("mrblib/core.mrb");
/// `mrbgems/mruby-enumerator/mrblib/enumerator.rb` (pure Ruby, needs Fiber), loaded after the core mrblib.
pub const MRBLIB_ENUMERATOR_MRB: &[u8] = include_bytes!("mrblib/enumerator.mrb");
/// The Ruby parts of the *-ext gems, in the reference's gembox order (`mrbgems/default.gembox`).
pub const MRBLIB_SPRINTF_MRB: &[u8] = include_bytes!("mrblib/sprintf.mrb");
pub const MRBLIB_ENUM_EXT_MRB: &[u8] = include_bytes!("mrblib/enum-ext.mrb");
pub const MRBLIB_STRING_EXT_MRB: &[u8] = include_bytes!("mrblib/string-ext.mrb");
pub const MRBLIB_ARRAY_EXT_MRB: &[u8] = include_bytes!("mrblib/array-ext.mrb");
pub const MRBLIB_HASH_EXT_MRB: &[u8] = include_bytes!("mrblib/hash-ext.mrb");
pub const MRBLIB_RANGE_EXT_MRB: &[u8] = include_bytes!("mrblib/range-ext.mrb");
pub const MRBLIB_PROC_EXT_MRB: &[u8] = include_bytes!("mrblib/proc-ext.mrb");
pub const MRBLIB_METHOD_MRB: &[u8] = include_bytes!("mrblib/method.mrb");
pub const MRBLIB_COMPAR_EXT_MRB: &[u8] = include_bytes!("mrblib/compar-ext.mrb");
pub const MRBLIB_ENUM_LAZY_MRB: &[u8] = include_bytes!("mrblib/enum-lazy.mrb");
pub const MRBLIB_ENUM_CHAIN_MRB: &[u8] = include_bytes!("mrblib/enum-chain.mrb");
pub const MRBLIB_SYMBOL_EXT_MRB: &[u8] = include_bytes!("mrblib/symbol-ext.mrb");
pub const MRBLIB_OBJECT_EXT_MRB: &[u8] = include_bytes!("mrblib/object-ext.mrb");
pub const MRBLIB_NUMERIC_EXT_MRB: &[u8] = include_bytes!("mrblib/numeric-ext.mrb");
pub const MRBLIB_CATCH_MRB: &[u8] = include_bytes!("mrblib/catch.mrb");
pub const MRBLIB_SET_MRB: &[u8] = include_bytes!("mrblib/set.mrb");
pub const MRBLIB_STRUCT_MRB: &[u8] = include_bytes!("mrblib/struct.mrb");
pub const MRBLIB_DATA_MRB: &[u8] = include_bytes!("mrblib/data.mrb");
pub const MRBLIB_TOPLEVEL_EXT_MRB: &[u8] = include_bytes!("mrblib/toplevel-ext.mrb");
pub const MRBLIB_RATIONAL_MRB: &[u8] = include_bytes!("mrblib/rational.mrb");
pub const MRBLIB_COMPLEX_MRB: &[u8] = include_bytes!("mrblib/complex.mrb");
/// mruby-regexp's Ruby part, embedded only with the feature `regexp` (`Cargo.toml`).
#[cfg(feature = "regexp")]
pub const MRBLIB_REGEXP_MRB: &[u8] = include_bytes!("mrblib/regexp.mrb");
/// mruby-task's `Task::Queue#push`/`#pop` (`mrbgems/mruby-task/mrblib/queue.rb`).
pub const MRBLIB_TASK_MRB: &[u8] = include_bytes!("mrblib/task.mrb");
/// `require`/`load`, which the reference has no equivalent of (`src/mrblib/require.rb`).
pub const MRBLIB_REQUIRE_MRB: &[u8] = include_bytes!("mrblib/require.mrb");
