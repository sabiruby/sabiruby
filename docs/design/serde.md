# serde and JSON (`sabiruby-serde`)

The crate `serde/` (`sabiruby-serde`) turns a Rust value that is `Serialize` into a Ruby value
and back, and puts a Ruby `JSON` class on top of that. It is a separate crate on purpose: the
VM is `no_std` and its dependency list is part of what it offers, so `sabiruby` never depends
on `serde` or on `serde_json` (`cargo tree -p sabiruby -e normal` shows four crates:
hashbrown, foldhash, libm, regex-automata).

```rust
let v: Value = sabiruby_serde::to_value(&mut vm, &config)?;
let config: Config = sabiruby_serde::from_value(&mut vm, v)?;
```

Both take a `&mut Vm`, because every Ruby value that is not immediate is a handle into the
VM's heap: there is nothing to build a String or a Hash *with* otherwise.

The crate is itself `no_std` + alloc. The default feature `json` is what adds `serde_json`
(and, through its `preserve_order`, `std`); `default-features = false` leaves the conversion
layer alone, which builds for `thumbv7em-none-eabi`.

## The data model

| serde | Ruby | and back |
|---|---|---|
| `bool` | `true` / `false` | the same |
| `i8`…`i64`, `u8`…`u32` | Integer | an Integer that fits, else a `TypeError` |
| `u64`, `i128`, `u128` | Integer, wide when it must be | the same |
| `f32` / `f64` | Float | Float, or an Integer widened (`mrb_ensure_float_type`) |
| `char` | a one-character String | a String of exactly one character |
| `String`, `&str` | String | String **or** Symbol; not-UTF-8 is an error |
| bytes (`serialize_bytes`) | String marked binary | any String, bytes unchanged |
| `None` / `Some(x)` | `nil` / `x` | `nil` is `None` |
| `()`, unit struct | `nil` | `nil` |
| seq, tuple, tuple struct | Array | Array |
| map | Hash, keys as they serialize | Hash |
| struct | Hash with String keys | Hash with String **or** Symbol keys |
| unit variant | Symbol (`:Red`) | Symbol or String |
| newtype variant | `{"Name" => value}` | a Hash of one entry |
| tuple variant | `{"Name" => [a, b]}` | a Hash of one entry |
| struct variant | `{"Name" => {"a" => …}}` | a Hash of one entry |

`Options { symbol_keys: true }` (through `to_value_with`) writes struct fields and variant
names as Symbols instead. Writing has one shape; reading accepts both, which is the asymmetry
that makes a struct fit a Hash a script wrote either way.

Two consequences worth stating out loud. `Some(None)` and `None` are the same `nil` once they
are in Ruby, so `Option<Option<T>>` does not survive the round trip — that is the mapping, not
a bug. And a String that is not valid UTF-8 is bytes, not text: `deserialize_str` refuses it
rather than replacing characters, and `deserialize_any` hands it over as a byte buffer.

## Errors

serde builds its errors from a message alone (`Error::custom`), with no VM in reach, so the
crate has an error type of its own (`sabiruby_serde::Error`) and converts it into a raise at
the boundary. `to_value` and `from_value` answer with `VmResult`, so a native that converts
can `?` it like any other call:

| what failed | what Ruby sees |
|---|---|
| a type or a shape that does not fit, a missing field, an unknown variant | `TypeError` with serde's message |
| the VM raised while converting (a Ruby `hash`/`eql?` raised) | that exception, unchanged |
| `JSON.parse` on text that is not JSON | `JSON::ParserError` (`< StandardError`) with serde_json's message and position |
| `JSON.generate` on NaN, Infinity, an Integer too wide for JSON, or a structure 100 deep | `JSON::GeneratorError` (`< StandardError`) |

CRuby puts both classes under `JSON::JSONError`; here they are direct subclasses of
`StandardError`, which is what scripts actually depend on (`rescue JSON::ParserError`, and a
bare `rescue`).

## From `define_fn`

`Serde<T>` is the wrapper that puts a serde type into a typed signature:

```rust
vm.define_fn(object, "shift", |p: Serde<Point>| Serde(Point { x: p.0.x + 1, y: p.0.y + 1 }));
```

There is no blanket `impl<T: Serialize> IntoRuby for T`, and there cannot be: the VM already
has `IntoRuby` for `i64`, `String`, `Vec<T>` and a dozen more, `i64` is `Serialize`, and a
blanket impl would overlap every one of them. Coherence forbids that, and specialization,
which would settle it, is not stable. The wrapper also says at the call site which conversion
is meant — `String` through `FromRuby` is the String's bytes, `Serde<String>` is a value that
happens to be a string.

`IntoRuby::into_ruby` answers with a `Value` and has no way to raise, while serializing can
fail (never in this crate's serializer, which only fails when the VM does, but a hand-written
`Serialize` may call `Error::custom`). `Serde<T>` then answers with **the exception object**
rather than with `nil`, so the failure is visible instead of looking like missing data; a
function that wants the ordinary raise answers with `VmResult<Value>` and calls `to_value`
itself.

## Declarations (`declare`)

A host that wants its data *written* in Ruby rather than computed by it gives the VM a method
per kind of thing and lets a script call it, a line per entry:

```ruby
unit :metre, symbol: "m",  scale: 1.0
unit :inch,  symbol: "in", scale: 0.0254
```

```rust
let units = Declarations::<Unit>::install(&mut vm).define(&mut vm, "unit");
vm.load_and_run(&data_rb)?;                          // the script above
let table: Vec<(String, Unit)> = units.take(&mut vm);
```

The first argument is the name (Symbol or String), the keyword arguments are the value: they
reach a native as a trailing Hash, and only when there are any (`Vm::native_call_args`), so a
declaration with no keywords is read as `{}` — a `T` whose fields all have defaults needs no
empty Hash in the script. The method is defined with `Vm::define_closure` rather than
`define_fn` for that reason: `define_fn` checks a fixed number of arguments.

The deserialization happens inside the native, which is what puts the script's own file and
line on every complaint serde has: "missing field `scale` (TypeError)" at `data.rb:2`, and the
same for a field of the wrong type and for an unknown field under `#[serde(deny_unknown_fields)]`,
which is worth recommending in a data file — a misspelled key is otherwise silently nothing. A
name declared twice is an `ArgumentError` at the second declaration; the entry point that lets
a later file amend an earlier one is a *differently named method* (`define_replacing`), so that
overwriting is something a script asks for rather than something it does by accident. There is
no limit on how many declarations a table takes or how long a name may be.

`expose` is the way back — a host table a script looks up by name, answering with a Hash with
Symbol keys (`Options::symbol_keys`) or `nil`:

```rust
let units = expose(&mut vm, "unit_of", table);   // unit_of(:metre)[:symbol] is "m"
```

It is deliberately a *second* table rather than a window on the one being collected: what a
script reads back is what the host decided to publish, after it has checked the declarations
and derived from them, and it may be published to a different VM from the one that declared
them. That also keeps `take` honest — afterwards the VM holds none of the declarations.

**Where the table lives.** In the VM, in the type's `HostStore` (`Vm::install_host_store`),
one table per Rust type per VM. Not in `Vm::set_host_state`, which holds one value for the
whole VM and which an embedder (rubevy) may already be using — overwriting it would make that
host's own commands disappear; and not in a `static` or an `Arc<Mutex<…>>`, which this crate,
being `no_std`, has no lock for. A store is a slab meant for the values `Data` objects name,
and this is that slab holding one value that no `Data` object names: nothing is handed to
Ruby, so the one thing the VM does with a store of its own — dropping an entry when an object
of its tag is collected — never fires. What it costs is that tag, one number of
`Vm::next_data_tag`'s sequence per declared type.

Nothing in a table is a Ruby value — the names are `String`s and the entries are whatever `T`
deserializes into — so a collection has nothing to reach there and a table cannot keep a Ruby
object alive by accident. `take` moves the table out through `HostStore::take`, which leaves
the slab's place reserved rather than giving its number up for reuse: a declaration made
afterwards raises `RuntimeError` instead of filling a table nobody will read, and the number
can never come to name a later table. `serde/tests/declare.rs` measures the rest of that
claim: a VM whose declarations have been taken and collected has exactly as many live objects
as one that was given no declarations at all.

## Why JSON lives here

JSON is a serde format, and `serde_json` already is the parser and the writer — mruby/edge's
`mruby-serde-json` reaches the same conclusion. Putting it in the VM would mean either a
hand-written parser in `no_std` or `serde_json` in the VM's dependency list, and the VM's
`JSON` would then be there for hosts that never asked for it. So it is a call the host makes:

```rust
sabiruby_serde::install_json(&mut vm);   // JSON.parse, .generate, .pretty_generate, Object#to_json
```

`JSON.parse` goes through `serde_json::Value` and this crate's own `to_value`, so the mapping
needs no second table. `JSON.generate` does **not** go the other way through `from_value`:
generating is not deserializing — a key is whatever `to_s` says (`{1 => 2}` is `{"1":2}`, as
CRuby does it), and an object of any other class is its `to_s`. That walk is written out in
`json.rs`.

The reference has an unofficial mruby-json gem; compatibility with it is not a goal. What is
checked is CRuby: `serde/tests/custom/json_roundtrip.rb` runs on both, and its `.expected` is
CRuby's output.

Known differences from CRuby's JSON:

* keyword arguments (`symbolize_names:`, `max_nesting:`, `object_class:`) are not taken —
  `Vm::define_fn` does not cover keyword arguments;
* an Integer wider than 64 bits raises `JSON::GeneratorError` instead of being written out
  (serde_json's `Number` stops there without its `arbitrary_precision` feature), and an
  integer literal that wide in the text parses as a Float;
* `JSON::ParserError` and `JSON::GeneratorError` sit directly under `StandardError`.

## GC

A value built here is not a collector root, exactly as in `src/convert.rs`. Nothing is
collected while a native is on the host stack, and nothing is collected outside the
instruction loop at all (`docs/design/gc.md`), so a conversion is safe as it stands; a `Value`
a host keeps across a call into the VM has to be registered with `Vm::gc_register`.

## What the VM gained for this

Two entry points, in the shape stage 3b of `host-bridge-plan.md` gave the others:

* `Vm::hash_entries(v) -> Option<Vec<(Value, Value)>>` — a Hash's entries in insertion order,
  the counterpart of `ary_vals`. Without it a host can only read a Hash key by key, and it has
  no way to get the keys. (`Vm::hash_keys(v) -> Option<Vec<Value>>`, added later for the same
  reason on rubevy's side, is the keys alone, for a host that then looks up only the ones it
  wants with `hash_get`.)
* `Vm::define_class_under(outer, name, superclass)` — `mrb_define_class_under`, so that
  `JSON::ParserError` is a constant of `JSON` and answers with its qualified name.

## Where it is checked

`serde/tests/roundtrip.rs` (the data model, both directions, with the Ruby side of each value
printed by the VM), `serde/tests/json.rs` (the class, the two error classes, `Serde<T>` in a
`define_fn` signature, and the CRuby case), and `serde/tests/declare.rs` (declarations in
order, a declaration without keywords, the file and line of a missing, mistyped and unknown
field, a duplicate name, the overwriting door, taking the table twice, declaring after it was
taken, what the collector is left with, and a table read back from Ruby).

`tools/check_no_std.sh` builds the VM alone; this crate's own `no_std` build is
`cargo build -p sabiruby-serde --lib --no-default-features --target thumbv7em-none-eabi`.
