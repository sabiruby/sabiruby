# sabiruby-macros

Derive and attribute macros for [SabiRuby](https://crates.io/crates/sabiruby): a Rust struct
and its `impl` block as a Ruby class, without writing the registration by hand.

Add it through the VM crate's feature `macros` (one dependency, one `use`):

```toml
sabiruby = { version = "0.6", features = ["macros"] }
```

```rust
use sabiruby::{RubyClass, ruby_methods, Vm};

#[derive(RubyClass)]
struct Player { hp: i64 }

#[ruby_methods]
impl Player {
    fn new(hp: i64) -> Self { Player { hp } }        // Player.new(100)
    fn damage(&mut self, n: i64) { self.hp -= n; }   // player.damage(10)
    fn hp(&self) -> i64 { self.hp }                  // player.hp
}

Player::register(&mut vm)?;                          // the class, the store, the methods
```

The value stays in Rust — it lives in a `HostStore` the VM carries and Ruby holds a handle to
it, which the collector gives back when nothing refers to it any more. Arguments and answers
go through `FromRuby` and `IntoRuby`, so a type neither covers is a compile error.

`use sabiruby::{RubyClass, ruby_methods};` brings two things called `RubyClass`: this crate's
derive macro and the trait it implements, which live in different namespaces (as
`serde::Serialize` does). Depending on this crate directly works too, and expands to exactly the
same code:

```toml
sabiruby = "0.6"
sabiruby-macros = "0.1"
```

```rust
use sabiruby::host_store::RubyClass;            // the trait
use sabiruby_macros::{RubyClass, ruby_methods}; // the macros
```

This crate does not depend on `sabiruby`: what it generates names `::sabiruby::…` absolutely, so
it compiles the same whichever way it was reached. What is generated, and what it does not
cover, is
[`docs/design/macros.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/macros.md)
of the [repository](https://github.com/sabiruby/sabiruby); what changed in each release is its
[`CHANGELOG.md`](https://github.com/sabiruby/sabiruby/blob/main/CHANGELOG.md).
