# sabiruby-serde

serde for [SabiRuby](https://crates.io/crates/sabiruby): a Rust value as a Ruby value and back,
`declare` for data *written* in Ruby and collected into a Rust table, and a Ruby `JSON` built on
top of the same conversions.

```rust
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct Config { name: String, retries: u32, verbose: bool }

let cfg = Config { name: "a".into(), retries: 3, verbose: true };
let v = sabiruby_serde::to_value(&mut vm, &cfg)?;   // {"name" => "a", "retries" => 3, …}
let back: Config = sabiruby_serde::from_value(&mut vm, v)?;
```

`Serde<T>` puts a serde type into a typed method's signature:

```rust
vm.define_fn(object, "configure", |cfg: Serde<Config>| cfg.0.name);
```

`declare` is the other way round — data *written* in Ruby, collected into a Rust table:

```ruby
unit :metre, symbol: "m",  scale: 1.0
unit :inch,  symbol: "in", scale: 0.0254
```

```rust
let units = Declarations::<Unit>::install(&mut vm).define(&mut vm, "unit");
vm.load_and_run(&data_rb)?;
let table: Vec<(String, Unit)> = units.take(&mut vm);   // in the order they were declared
```

Every field serde refuses — missing, of the wrong type, unknown under `deny_unknown_fields` —
raises at the line of that declaration, in the script's own file; a name declared twice raises
an `ArgumentError` unless the script uses the method that says it means to overwrite.
`take_with_lines` hands back the line each declaration was on, for the checks that need two
declarations to see (a recipe naming an item nothing declared) and that serde cannot make. The
other direction, `expose`, lets a script look a host table up by name, answering with a Hash
whose keys are Symbols — `Options::symbol_map_keys` outside `declare`, and `Options::symbols()`
for that and a struct's field names together.

and `install_json` gives the VM a `JSON` class — `JSON.parse`, `JSON.generate`,
`JSON.pretty_generate`, `Object#to_json` — written in Rust on `serde_json`:

```rust
sabiruby_serde::install_json(&mut vm);
```

The VM itself never depends on serde: it is `no_std` and small on purpose, and everything here
is a layer above it. This crate is `no_std` + `alloc` too; the default feature `json` is what
brings in `serde_json` (and, through its `preserve_order`, `std`), so
`default-features = false` leaves the conversion layer alone.

The data model, the error mapping, the declarations and what differs from CRuby's JSON are
[`docs/design/serde.md`](https://github.com/sabiruby/sabiruby/blob/main/docs/design/serde.md)
of the repository; what changed in each release, and what a host has to change to move up, is
[`CHANGELOG.md`](https://github.com/sabiruby/sabiruby/blob/main/CHANGELOG.md).

MIT licensed, like the rest of SabiRuby.
