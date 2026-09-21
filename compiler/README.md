# sabiruby-compiler

The reference mruby 4.1.0-rc compiler as a Rust library: Ruby source in, RITE bytecode
(`.mrb`) out, **byte for byte what the reference `mrbc` writes**.

It is not a port. The crate builds mruby's own `mruby-compiler` (the Prism parser plus
mruby's code generator) as C with the [`cc`](https://crates.io/crates/cc) crate, in the
standalone configuration the reference `mrbc` itself is built in (no mruby VM linked), and
calls it through a small C shim. The bytecode runs on the
[SabiRuby](https://crates.io/crates/sabiruby) VM, or on mruby.

```rust
use sabiruby_compiler::{compile, Options};

let bin = compile(b"puts 'hello'", &Options::default())?;          // like `mrbc -`
let dbg = compile(src, &Options { filename: "app.rb".into(), debug_info: true, ..Default::default() })?; // `mrbc -g app.rb`
```

A failure is a `CompileError` whose `diagnostics` carry kind, message, file, line and
column; `Display` prints the errors as `FILE:LINE:COL: message`, as `mrbc` does.

## What is vendored

`vendor/` holds unmodified copies from mruby 4.1.0-rc (commit `3cf73ee`), listed with their
licences in [`vendor/VENDOR.md`](https://github.com/sabiruby/sabiruby/blob/main/compiler/vendor/VENDOR.md):

* `mrbgems/mruby-compiler` (`include/`, `src/`): MIT, Copyright (c) HASUMI Hitoshi
* Prism 1.9.0 (`lib/prism`: `include/`, `src/`): MIT, Copyright Shopify Inc.
* the Prism sources generated from its ERB templates, taken from a reference build
  (so building this crate needs no Ruby)
* mruby's `include/mrbconf.h`: MIT, mruby developers

`tools/vendor_compiler.sh` in the repository redoes the copy for a new mruby version.

## API

* `compile(src: &[u8], opts: &Options) -> Result<Vec<u8>, CompileError>`
* `Options { filename, debug_info /* -g */, remove_lv, no_ext_ops, no_optimize }`;
  the default is `filename: "-e"`, everything else off (plain `mrbc`)
* `Diagnostic { kind: Kind, message, filename, line, column }`,
  `Kind::{ParserWarning, ParserError, GeneratorWarning, GeneratorError}`
* `highlight(src: &[u8]) -> Vec<u8>`: one category byte per source byte, for an editor's
  colours — 0 default, 1 keyword, 2 string, 3 comment, 4 number, 5 symbol, 6 constant,
  7 variable, 8 method name. Prism decides, first with its lexer (so interpolation, regular
  expressions, `%w[]` and heredocs are told apart the way the parser tells them apart) and
  then with its syntax tree (method names, whole symbols). A source with syntax errors still
  gets a map, and a run of one category never cuts a UTF-8 character in half.
* `version()`: `"mruby 4.1.0-rc (3cf73ee), Prism 1.9.0"`
* feature `ast`: `ast(src, filename) -> Option<String>`, Prism's syntax tree pretty-printed by
  `pm_prettyprint`, the format a debug build of `mrbc --verbose` prints (and the book's listings).
  Off by default: the default build keeps the reference's `PRISM_BUILD_MINIMAL`; the feature
  keeps every exclusion of it except the pretty-printer, which does not change the bytecode (the
  golden tests pass with it too).

Compilations are serialised by a lock: `mrc_presym.c` writes a global on every parse.

## Platforms

Needs a C compiler (`gnu99`, as the reference build) at build time. Built and tested on Linux (gcc) and macOS (clang)
in CI; Windows (MSVC) is expected to work, as Prism and mruby support it, but is not tested.
Compiling the vendored C is most of what a clean build of this crate costs, and it is paid once.

**wasm32-wasip1** works with [wasi-sdk](https://github.com/WebAssembly/wasi-sdk) (checked with
wasi-sdk 34, clang 23): set `CC_wasm32_wasip1=<wasi-sdk>/bin/clang` and
`AR_wasm32_wasip1=<wasi-sdk>/bin/llvm-ar` and build for `--target wasm32-wasip1`. The build
script then adds what the target needs: the compiler's `MRC_TRY`/`MRC_THROW` (code generator
errors) are `setjmp`/`longjmp`, which wasi-libc implements with WebAssembly exception handling,
so the C is compiled with `-mllvm -wasm-enable-sjlj -mllvm -wasm-use-legacy-eh=true` and
wasi-sdk's `libsetjmp.a` is linked (copied into `OUT_DIR` under its own name, so that `-lc`
still resolves to rustc's wasi libc). The resulting module needs an engine with Wasm exception
handling, legacy encoding: Chrome 95+, Firefox 100+, Safari 15.2+, Node 22. It is what the
browser playground runs. `wasm32-unknown-unknown` (no libc) is not supported.

## Verification

The repository's golden tests (`compiler/tests/golden.rs`) compile every `.rb` that has a
`.mrb` produced by the reference `mrbc` (Docker image `kishima/mruby:4.1.0-rc`): the VM
fixtures, mruby's own test suite (with `-g`, so DBG and LVAR are compared too) and the
benchmarks — 157 files as this is written. All are byte-identical. The error messages and exit
codes of `sabiruby compile` match `mrbc`'s.

## License

MIT for the Rust code and the shim; the vendored sources keep their own MIT licences
(`vendor/VENDOR.md`). What changed in each release is
[`CHANGELOG.md`](https://github.com/sabiruby/sabiruby/blob/main/CHANGELOG.md) of the repository.
