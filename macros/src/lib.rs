//! A Rust struct and its `impl` block as a Ruby class, for the
//! [SabiRuby](https://crates.io/crates/sabiruby) VM.
//!
//! ```
//! use sabiruby::Vm;
//! use sabiruby_macros::{RubyClass, ruby_methods};
//!
//! #[derive(RubyClass)]
//! struct Player { hp: i64 }
//!
//! #[ruby_methods]
//! impl Player {
//!     fn new(hp: i64) -> Self { Player { hp } }        // Player.new(100)
//!     fn damage(&mut self, n: i64) { self.hp -= n; }   // player.damage(10)
//!     fn hp(&self) -> i64 { self.hp }                  // player.hp
//! }
//!
//! # fn main() -> Result<(), sabiruby::VmError> {
//! let mut vm = Vm::with_mrblib()?;
//! Player::register(&mut vm)?;                          // the class, the store, the methods
//! # Ok(()) }
//! ```
//!
//! The value stays in Rust: it lives in a `HostStore` the
//! VM carries, and Ruby holds a `Data` object naming it, which the collector gives back when
//! nothing refers to it any more. Arguments and answers are converted by
//! `FromRuby` and `IntoRuby`, so a type neither
//! covers is a compile error rather than a surprise at run time.
//!
//! This crate does not depend on `sabiruby`: what it generates names `::sabiruby::…`,
//! absolutely, so it expands the same however it was reached. A host normally reaches it
//! through the VM crate's feature `macros` — `sabiruby = { version = "0.5", features =
//! ["macros"] }` and `use sabiruby::{RubyClass, ruby_methods};`, which is one dependency and
//! one `use` (serde's arrangement with serde_derive) — or names both crates, as the example
//! above does. What is generated, and what it does not cover, is `docs/design/macros.md` of
//! the repository.

mod expand;

use proc_macro::TokenStream;

/// Makes a Rust type a Ruby class: its name, the `Data` tag its objects carry, and the store
/// its values live in.
///
/// Generates `impl RubyClass for T` (whose `borrow` and `borrow_mut` are how a handle becomes
/// a `&T` or a `&mut T`) and `impl IntoRuby for T` (which puts a value in the store and hands
/// Ruby the object naming it, so that a method answering with `Self` needs no special case).
///
/// The Ruby class is named after the type; `#[ruby(name = "Player")]` on the type says
/// otherwise.
#[proc_macro_derive(RubyClass, attributes(ruby))]
pub fn derive_ruby_class(input: TokenStream) -> TokenStream {
    expand::derive_ruby_class(input.into()).into()
}

/// Registers the `fn`s of an inherent `impl` block as the Ruby class's methods, as
/// `T::register(&mut vm) -> VmResult<ObjId>`.
///
/// A `fn` with no `self` is a class method (`Player.new`), `&self` and `&mut self` are
/// instance methods, and a leading `&mut Vm` (after `self`) is the call's context rather than
/// an argument, as it is in `Vm::define_fn`. Everything else is an
/// argument, converted by `FromRuby`, and the answer by `IntoRuby`; `Method#arity` answers
/// with the number of arguments.
///
/// `#[ruby(name = "alive?")]` on a `fn` gives it another name in Ruby, and `#[ruby(skip)]`
/// keeps it out of the class.
#[proc_macro_attribute]
pub fn ruby_methods(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand::ruby_methods(attr.into(), item.into()).into()
}
