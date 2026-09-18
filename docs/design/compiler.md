# Compiler

`sabiruby run foo.rb`, `sabiruby -e CODE` and `sabiruby compile` compile Ruby source with the
**reference compiler itself**: mruby 4.1.0-rc's `mrbgems/mruby-compiler` (Prism 1.9.0 as the
parser, mruby's code generator), built as C and linked into the command. The compiler is not
the subject of this project (nor of the book), so it is not ported to Rust; what matters is
that the bytecode is exactly the reference's. The plan this follows is
[`../plans/compiler-plan.md`](../plans/compiler-plan.md).

## Layout

* **Crate `sabiruby-compiler`** (`compiler/`, a workspace member). std, links libc, does
  not depend on `sabiruby`. The VM crate `sabiruby` stays pure Rust and `no_std` and does not
  depend on it. The `sabiruby` command is a third crate, `sabiruby-cli` (`cli/`), which
  depends on both. (A first version made the compiler a default feature of `sabiruby` for its
  binary; features apply to the whole crate, so every library user, rubevy included, would
  have built C and lost wasm. Split on review, like mruby/edge's `mrubyedge-cli`.)
* `compiler/vendor/`: unmodified copies of `mruby-compiler`, Prism, Prism's generated sources
  and `mrbconf.h` (origin, versions, licences and the update procedure in
  [`compiler/vendor/VENDOR.md`](../../compiler/vendor/VENDOR.md); `tools/vendor_compiler.sh`).
* `compiler/build.rs`: compiles them with `cc` as `gnu99`, the reference build's `-std` (strict
  `c99` hides POSIX declarations such as `memccpy`, which wasi-libc then refuses).
* `compiler/csrc/shim.c`: the only C written here (below).
* `compiler/src/ffi.rs`: the `extern "C"` functions, the crate's only `unsafe`.
* `compiler/src/lib.rs`: `compile(src, &Options) -> Result<Vec<u8>, CompileError>`,
  `Diagnostic`, `highlight(src) -> Vec<u8>`, `version()`.

## The standalone path, as the reference mrbc

The reference build compiles `mrbc` in a sub-build without the mruby VM
(`lib/mruby/build.rb`, `generate_mrbc_build`: `disable_libmruby`). Its compile flags for the
compiler (`build/host/mrbc/mrbgems/mruby-compiler/src/*.o.flags` in a reference build) have
`-DMRB_NO_GEMS`, so `mrbgem.rake` does not add `MRC_TARGET_MRUBY`; the `.d` files show that
`mruby.h` is never included. With neither `MRC_TARGET_MRUBY` nor `MRC_TARGET_MRUBYC`,
`include/mrc_common.h` takes the standalone path: `mrb_state` is `void`, only `mrbconf.h` is
read, and `prism_xallocator.h` allocates with libc. `build.rs` uses the same configuration:
`PRISM_XALLOCATOR`, `PRISM_DEPTH_MAXIMUM=256`, `PRISM_BUILD_MINIMAL` (a debug reference build
has `MRC_DEBUG` instead, which adds assertions, poisons freed ireps and keeps Prism's AST
printer; the golden tests compare with the `mrbc` of the Docker image), no `MRC_TARGET_*`, no `MRC_NO_STDIO`, no `MRC_INT32` (so `MRB_INT64`, like SabiRuby).

The reference `mrbc.c` ends with dummy definitions of `mrb_intern` and `mrb_sym_name`. The
shim does not copy them: every call to them in `mruby-compiler` is inside
`#if defined(MRC_TARGET_MRUBY)`, so the standalone build does not reference them (`nm` shows no
undefined symbol), and defining them would clash with a real mruby linked into the same program.

## The shim

`sabiruby_mrc_compile(src, len, filename, flags, &out, &out_len, &diag)` does what `mrbc`'s
`main` does, from memory instead of files: `mrc_ccontext_new(NULL)`, `mrc_ccontext_filename`
(it copies the string), `no_exec`/`no_ext_ops`/`no_optimize`, `mrc_load_string_cxt` on a
NUL-terminated copy of the source, the diagnostics, `mrc_irep_remove_lv` if asked,
`mrc_dump_irep` into memory (`MRC_DUMP_DEBUG_INFO` for `-g`), and every free. Rust never
touches `mrc_ccontext` (it has bit fields), so no bindgen. Diagnostics come back as one string
(records separated by `0x1e`, fields by `0x1f`) and are split in Rust.

`mrc_presym.c` writes a static variable on every parse, so `compile` holds a global lock.

`sabiruby_mrc_highlight(src, len, out)` fills one **category byte per source byte** for an
editor's colours: 0 default, 1 keyword, 2 string, 3 comment, 4 number, 5 symbol, 6 constant,
7 variable, 8 method name. It is family-mruby's `picoruby-syntax-highlight` (same nine
categories, same Prism 1.9.0) without its mruby binding and otherwise whole — two passes, in
its order:

1. **the lexer.** A `pm_lex_callback_t` on the parser paints each token's `start..end` as it
   is lexed, from its type. A `#{}` inside a string, a regular expression, `%w[]` and a
   heredoc are therefore told apart the way the parser tells them apart, and **a source with
   syntax errors still comes back with a map** — the lexer runs ahead of the parser, and what
   it reached is already written.
2. **the tree.** `pm_visit_node` then paints what a token type cannot say, overwriting the
   first pass: `PM_CALL_NODE`'s `message_loc` and `PM_DEF_NODE`'s `name_loc` as the method
   name, `PM_SYMBOL_NODE` whole as a symbol. Without it the bare calls a script is mostly made
   of (`tell :all, "season", s`, `sleep 0.5`, `every 60 do`) would carry no colour at all, and
   `:Plant` would read as a colon and a constant. A call whose identifier only looks like a
   variable is left alone (`PM_CALL_NODE_FLAGS_VARIABLE_CALL`), and an operator call is a call,
   so `=~` and `+` are method names.

There is no maximum source size, and no lock: nothing under `vendor/prism` touches
`mrc_presym.c`'s global, so an editor may colour while another thread compiles.
`compiler/tests/highlight.rs` records what Prism answers for each case;
[`../worklog/2026-09-18-highlight.md`](../worklog/2026-09-18-highlight.md) is the work.

With the feature `ast`, the shim also has `sabiruby_mrc_ast`: it parses with Prism as mrc does
(no options, line 1, the file name as the file path) and returns `pm_prettyprint`'s text, the
syntax tree mruby's code generator walks, in the format of a debug `mrbc --verbose` (the book's
chapter 5 listing `cg_ast_ast` is reproduced line for line). `PRISM_BUILD_MINIMAL` is then
replaced by its exclusions minus `PRISM_EXCLUDE_PRETTYPRINT`; the golden tests pass with the
feature on as well. The browser playground uses it for its AST pane.

## Verification

`compiler/tests/golden.rs` compiles every `.rb` of the repository that has a `.mrb` made by the
reference `mrbc` (Docker image `kishima/mruby:4.1.0-rc`) and requires identical bytes, with the
file name the generating script passed to `mrbc` (it ends up in the DBG section under `-g`):

| set | files | options | result |
|---|---:|---|---|
| `tests/fixtures/*.rb` | 17 | none | identical |
| `tests/mrbtest/src/*.rb` (mruby's test suite and gem tests) | 61 | `-g` (DBG and LVAR) | identical |
| `bench/src/*.rb` | 22 | none | identical |

A negative control (the test files without `-g`) makes all 61 differ. Checked once by hand as
well: the ten embedded mrblib binaries (`src/mrblib*.mrb`) recompiled from their sources are
identical, and the messages and exit codes of `sabiruby compile` on syntax errors are
byte-identical to `mrbc`'s.

The verification baseline does not move: `tools/mrbtest.sh`, `tools/fixtures.sh` and
`tools/bench.sh` keep compiling with the reference `mrbc` in Docker; the golden tests show that
the embedded compiler agrees with it.

## CLI (crate `sabiruby-cli`, `cli/`)

* `sabiruby FILE [args]` (also `sabiruby run FILE`): a file starting with `RITE` runs as
  bytecode; anything else is compiled with `filename` = FILE and `debug_info` (DBG is not read by
  the VM, but the LVAR section is needed by `local_variables` and `Proc#parameters`, and the
  reference only writes it with `-g`). The switches are the reference `mruby` command's
  (`-b`, `-c`, `-d`, `-e`, `-r`, `-v`, `--verbose`, `--version`, `--copyright`), parsed with clap;
  `-r` (require) is not implemented yet. With no program file the source comes from stdin.
* `sabiruby -e 'CODE' [args]`: file name `-e`, as `mruby -e`.
* `sabiruby compile FILE [-o OUT] [-g] [--remove-lv] [--no-ext-ops] [--no-optimize]`: `mrbc`'s
  options. The default output replaces the extension with `.mrb`; unlike `mrbc`, a name
  without an extension gets `.mrb` appended (`mrbc` would reuse the input name and overwrite
  the source). `-o -` writes to stdout.
* `sabiruby dump FILE`: `.rb` or `.mrb`.
* Errors print as `FILE:LINE:COL: message` on stderr, exit code 1 (as `mrbc`; warnings are not
  printed, as `mrbc` without `--verbose`).
* `sabiruby --version` also names the embedded compiler.
* `sabiruby mrbtest` is unchanged (`.mrb` only).

## Build cost and platforms

A clean build of the C part: about 2 s (debug, `-O0`) and 7 s (release), on one core
(`cc` compiles the 35 files, 34 vendored plus the shim, one after another). The
`sabiruby-compiler` package is 2.9 MiB (394 KiB compressed). CI builds and tests on Linux and macOS; Windows (MSVC) should work but is
not tested. The VM library itself builds for `wasm32-unknown-unknown`.

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

## Publishing

In dependency order: `sabiruby-macros` 0.1.0, `sabiruby` 0.5.0, `sabiruby-compiler` 0.2.2,
`sabiruby-serde` 0.1.0, `sabiruby-cli` 0.5.0. The macros come *first* from 0.5.0 on: the VM's
feature `macros` makes `sabiruby-macros` an optional dependency of the VM, so the VM can no
longer be the first crate published. `cargo publish --workspace` works the order out itself
(the compiler's dependency on the VM is optional, behind the `host` feature, so the VM still
goes before the compiler). What each release adds is [`CHANGELOG.md`](../../CHANGELOG.md);
what 0.3.0 adds over 0.2.0 is
the gems: the numeric tower, Struct, Set, Time, pack, eval and `binding`, UTF-8 strings,
`require`/`load`, Regexp and the task scheduler (`docs/design/gems.md`); the compiler's 0.2.0 adds
the `host` feature, which is what `eval` asks for a compile. 0.4.0 adds what a host needs to
drive the scheduler itself: `task_next_wakeup_ticks`, `task_pending`, and `task_queue_new` /
`task_queue_push` (how a host answers a script that is parked on a question), plus `VERSION` and
`REVISION` so an embedder can say which VM it runs. (`task_queue_len`, `task_queue_try_pop` and
`Vm::hash_keys` were listed here as 0.4.0's and are not: `git log -S` puts all three in
`6af8276`, after it. They are 0.5.0's — [`CHANGELOG.md`](../../CHANGELOG.md).)

`sabiruby-compiler` 0.2.2 is a republish rather than a change: nothing of the vendored compiler
or the shim moved between 0.2.1 and it (`git log 0.2.1.. -- compiler/` is two commits, the
repository URL of the organization move and one documentation link), but its optional
dependency on the VM has to name 0.5.0 — a `sabiruby-cli` that pulled the VM at 0.5 and the
compiler at 0.2.1 would have two different `sabiruby` crates in one program.

A release need not publish all five. 0.5.2 (2026-09-18) published two: `sabiruby-compiler` 0.2.3
(`highlight()`) and `sabiruby` 0.5.2 (the release's name; the VM's source is unchanged).
`sabiruby-cli` 0.5.0, `sabiruby-macros` 0.1.0 and `sabiruby-serde` 0.1.0 had no change since
`v0.5.1` and stay as published; the CLI's requirements (`sabiruby` `^0.5.0`,
`sabiruby-compiler` `^0.2.2`) pick the new two up on a fresh install. Between those two the
order is free — the compiler's optional dependency on the VM is `^0.5.0`, which the published
0.5.1 already satisfied, and the VM's dev-dependency on the compiler is stripped from the
package ([`docs/worklog/2026-09-18-release-0.5.2.md`](../worklog/2026-09-18-release-0.5.2.md)).

A dev-dependency that names a version would be resolved from crates.io when the packaged crate
is verified, so the one on `sabiruby-compiler` here carries a path and no version: a version
would pin it to the last published compiler, which need not have the features this tree uses.
`cargo publish --dry-run --workspace` is in CI and catches exactly that.
