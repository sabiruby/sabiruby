# The macros: a Rust type as a Ruby class

`sabiruby-macros` (`macros/`) turns a struct and its `impl` block into a Ruby class. It is a
layer of writing, not of mechanism: everything it generates could be written by hand with
`Vm::define_fn`, `Vm::data_new` and `HostStore` — the macros exist so that it does not have to
be, and so that the twenty lines of registration per class cannot be got subtly wrong.

```rust
use sabiruby::Vm;
use sabiruby_macros::{RubyClass, ruby_methods};

#[derive(RubyClass)]
struct Player { hp: i64 }

#[ruby_methods]
impl Player {
    fn new(hp: i64) -> Self { Player { hp } }        // Player.new(100)
    fn damage(&mut self, n: i64) { self.hp -= n; }   // player.damage(10)
    fn hp(&self) -> i64 { self.hp }                  // player.hp
}

Player::register(&mut vm)?;
```

The crate does **not** depend on `sabiruby`: what it generates names `::sabiruby::…`, absolutely,
so the expansion compiles wherever a crate called `sabiruby` is in the dependency graph. That
keeps the VM buildable without a proc-macro toolchain (it is `no_std`, and a proc macro is a
compiler plugin that runs on the host).

## How a host depends on it

Two ways, and the generated code is the same either way, because it names `::sabiruby::…` and
never assumes the macros arrived from any particular crate.

The short one, since 0.5.0 — the feature `macros` on the VM crate, which is serde's and
serde_derive's arrangement:

```toml
sabiruby = { version = "0.5", features = ["macros"] }
```

```rust
use sabiruby::{RubyClass, ruby_methods};
```

That single `use` brings two items called `RubyClass`: the **derive macro**, re-exported from
`sabiruby-macros`, and the **trait** it implements, which is the one a `borrow` or an
`into_handle` needs in scope. They do not collide because a macro and a type live in different
namespaces — the same reason `use serde::Serialize;` gives you the trait and the derive at once.

The long one, still supported, naming both crates:

```toml
sabiruby = "0.5"
sabiruby-macros = "0.1"
```

```rust
use sabiruby::host_store::RubyClass;          // the trait
use sabiruby_macros::{RubyClass, ruby_methods}; // the macros
```

The feature is **off by default**. A proc macro is compiled for the host machine, and the VM's
reason for existing is that it builds for targets that have no host toolchain in the picture at
all; a host that does not use the macros should not pay a `syn` build for them. With the feature
off, `sabiruby-macros` is not in the dependency graph (`dep:` syntax, so there is no implicit
feature either). `tools/check_no_std.sh` is unaffected in all three of its combinations: a proc
macro is a compile-time artefact and nothing of it reaches the target.

`macros/tests/reexport.rs` is the test of the short way and `macros/tests/player.rs` of the long
one; between them they say that the two spellings expand to the same thing.

## Host Object, not a wrapper

The value stays in Rust. `#[derive(RubyClass)]` gives the type a [`HostStore`](../../src/host_store.rs)
— a slab — in the VM, and Ruby holds a `Data` object carrying the pair `(tag, handle)`: which
kind of thing, and which one (`gc.md`, "the free hook"). The VM never reads through the
handle, so no raw pointer is involved and a stale one can at worst name the wrong entry in the
host's own table.

That choice decides the shape of everything else:

* A Ruby object is collected when nothing refers to it; the store entry goes with it, so the
  Rust value's lifetime is the Ruby object's. Nothing is reference-counted and nothing is
  shared between a Ruby object and Rust.
* `dup` and `clone` refuse (`TypeError`): copying a handle would give two Ruby objects one Rust
  value, and the collection of either would take it from both. Only the host knows how to
  copy one of its own values, so only the host can offer a method that does.
* `==`, `eql?` and `hash` go by `(tag, handle)`, so two objects naming the same value are one
  key in a Hash; `equal?` is still object identity.

## What each macro generates

`#[derive(RubyClass)]` writes two impls: `RubyClass` (whose only required item is the class's
name — `#[ruby(name = "Player")]` says it, otherwise it is the type's name) and `IntoRuby`,
which puts a value in the store and hands Ruby the object naming it. The second is why a
method can answer with `Self`, or with `Vec<Self>`, or with `Result<Self, String>`, with no
special case anywhere: they are all just `IntoRuby`.

`#[ruby_methods]` gives the `impl` block back unchanged (minus the `#[ruby(…)]` attributes,
which are its own) and adds `T::register(&mut vm)`, which defines the class, installs the store
and its tag, and calls `Vm::define_fn` once per method. It answers `VmResult<ObjId>`: asking a
class for its singleton class (where the class methods go) is a `VmResult`, and the generated
code has no business deciding on the host's behalf that the answer cannot fail. Reading a signature:

| in Rust | in Ruby |
|---|---|
| no `self` | a class method (`Player.new`, `Player.strongest`) |
| `&self` | an instance method that reads |
| `&mut self` | an instance method that writes |
| a leading `&mut Vm` (after `self`) | the call's context, not an argument |
| a trailing `Block` | the block the caller passed, not an argument |
| everything else | the arguments, in order, at most six |
| the return type | the answer, through `IntoRuby` |

`#[ruby(name = "alive?")]` on a `fn` gives it another name in Ruby (where `?` and `!` are part
of a name and `_` is not always what is wanted), and `#[ruby(skip)]` keeps a `fn` out of the
class while leaving it on the Rust side.

Everything about arguments and answers is left to `define_fn` (`convert.rs`, stage 4 of
`../plans/host-bridge-plan.md`): the conversions are `FromRuby` and `IntoRuby`, the number of
arguments is checked on every call with the reference's own wording, and `Method#arity` answers
with it. A type neither conversion covers is a compile error; the generated code asks for the
`FromRuby` bound a second time at the span of the parameter, so that the error points at the
parameter and names the type rather than at the attribute and the closure.

## Borrowing the receiver

The store is in the VM, which is what lets the collector drop entries and what keeps two host
types from fighting over the one free hook (`host_store.rs`, and the worklog of 2026-09-15).
The cost is that a value cannot stay borrowed from the store while the VM is in use, so there
are two shapes of generated body:

* **Without a `&mut Vm`**, the value is borrowed in place: `RubyClass::borrow` /
  `borrow_mut` hand out a `&T` or a `&mut T` for the duration of the call.
* **With a `&mut Vm`**, the value moves out of the store for the call (`take_out`) and back
  afterwards (`give_back`), its handle reserved in the meantime. A call that re-enters the
  same object while it is out — the method called back into Ruby, and that Ruby called a method
  on the same object — raises `RuntimeError: Player is already in use by a call on the same
  object` rather than seeing a half-built value. It is `RefCell`'s rule, with a Ruby exception
  where `RefCell` panics.

## What it does not cover

* **Optional, rest and keyword arguments.** A method that wants them takes the raw call with
  `Vm::define_closure`, as it would without the macros. A **block** it does take: a last
  parameter spelled `Block` is the caller's block (`None` when there was none), it is not
  counted in the arity, and it comes with the `&mut Vm` — `define_fn` has no shape without it,
  and calling the block needs it anyway (`Vm::call_block`). A method that takes the block and
  the `&mut Vm` has its receiver out of the store for the call, so a block that calls back into
  the same object raises rather than seeing it half-written, as any `&mut Vm` method does.
* **Generic types and generic `fn`s**, `async fn`, `unsafe fn`, and a method that takes `self`
  by value (it would leave the Ruby object naming nothing). All are refused with a message
  saying why.
* **More than six arguments**, which is as far as `define_fn`'s shapes go.
* **`FromRuby for T`**: a handle can be borrowed as `&T` or `&mut T`, but a `T` cannot be taken
  out of Ruby by value — the object naming it would be left stale. A method takes `&self`.
* **A returned reference** (`fn name(&self) -> &str`): the answer is built after the borrow
  ends, so it must be owned. `String`, not `&str`.
* **Modules and nesting**: the class is defined under `Object`. A class inside a module is
  defined by hand, as `Vm::define_class` is.
* **Inheritance**: `Player.allocate`, or a Ruby subclass's `allocate`, makes an ordinary object
  with no handle in it; calling a method on one is a `TypeError` saying so —
  `uninitialized Ghost (expected Player): the object has no Player behind it — `Player.allocate`
  makes one without running `initialize``, worded after mruby's `mrb_data_check_type`
  (`uninitialized %t (expected %s)`, src/etc.c, for an `RData` whose `DATA_TYPE` is still NULL).
  A receiver of another class keeps `wrong argument type String (expected Player)`. A host that
  wants Ruby subclasses of a host class defines `new` in Ruby over the host's own constructor.

## Where the pieces are

| file | what |
|---|---|
| `src/host_store.rs` | `HostStore<T>`, the per-type stores a `Vm` carries, `RubyClass` |
| `macros/src/expand.rs` | both macros, as ordinary code (a proc-macro crate exports only macros) |
| `macros/src/lib.rs` | the proc-macro entry points |
| `macros/tests/expand.rs` | what is generated, fixed as readable text |
| `macros/tests/player.rs` | what it does, driven from Ruby, through a direct `sabiruby-macros` dependency |
| `macros/tests/reexport.rs` | the same macros reached through `sabiruby`'s feature `macros` |
| `tests/host_store.rs` | the slab and the stores, without the macros |
