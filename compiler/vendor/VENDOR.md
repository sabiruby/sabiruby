# Vendored sources

Copies from the reference mruby tree, made by `tools/vendor_compiler.sh` on 2026-09-11,
with **one patch**: `SABIRUBY_EVAL_SCOPES` (below). Nothing else here is edited by hand:
adapt in `compiler/csrc/shim.c` and `compiler/build.rs` instead.

| path | from | version | licence |
|---|---|---|---|
| `mruby-compiler/{include,src,LICENSE,README.md}` | `mrbgems/mruby-compiler` of mruby | 4.1.0-rc, commit `3cf73ee` | MIT, Copyright (c) HASUMI Hitoshi 2024 (`mruby-compiler/LICENSE`); gem author "mruby and PicoRuby developers" |
| `prism/{include,src,LICENSE.md}` | `mrbgems/mruby-compiler/lib/prism` (git submodule) | Prism 1.9.0, commit `c0e3781` | MIT, Copyright 2022-present Shopify Inc. (`prism/LICENSE.md`) |
| `prism/generated/{include,src}` | `build/prism/` of a reference build | generated from Prism 1.9.0's ERB templates by the reference `rake` | as Prism |
| `mrbconf.h` | `include/mrbconf.h` of mruby | 4.1.0-rc | MIT, mruby developers (`../../LICENSE-mruby`) |

4.1.0-rc2 (commit `c17ffcc24`)'s `mrbgems/mruby-compiler` (with the Prism submodule at the same `c0e3781`) and
`include/mrbconf.h` are identical to these (`git diff 4.1.0-rc 4.1.0-rc2 -- mrbgems/mruby-compiler include/mrbconf.h`
is empty; checked 2026-09-23), so nothing here was copied again for rc2.

`prism/generated/` holds the files Prism generates from `templates/*.erb`
(`include/prism/ast.h`, `include/prism/diagnostic.h`, `src/{diagnostic,node,prettyprint,serialize,token_type}.c`).
They are taken from the reference build instead of being regenerated (no Ruby needed to build
this crate). Their `#line` directives name `mrbgems/mruby-compiler/lib/prism/templates/...`,
the reference build's rewrite; they only affect diagnostics of the C compiler.

## The one patch: `SABIRUBY_EVAL_SCOPES`

`sabiruby-compiler` builds the compiler standalone, without `MRC_TARGET_MRUBY`, so it has no
mruby `RProc` chain to read the enclosing local variable names from — which is what compiling
an `eval` string needs (`docs/plans/eval-require-plan.md`). The patch adds the same two places the
reference fills from that chain, filled from a table of names instead, plus the field that
carries it. It is guarded by `SABIRUBY_EVAL_SCOPES` (defined by `build.rs`) and sits beside
the `MRC_TARGET_MRUBY` branches, so the reference's own paths are untouched and
`sabiruby_mrc_compile` (the `mrbc` entry, and the golden tests over it) is unchanged.

| file | what |
|---|---|
| `include/mrc_ccontext.h` | `struct sabiruby_eval_scope`/`sabiruby_eval_scopes` and the `eval_scopes` field of `mrc_ccontext` |
| `src/compile.c` | `sabiruby_pm_options_init()`: the Prism scopes from the table (what `mrc_pm_options_init()` builds from the `RProc` chain), called from `mrc_pm_parser_init()` |
| `src/codegen.c` | `search_upvar()`: after the code generator's own scopes, the table decides `(index, depth)`, with the reference's `lv - 1`; `sabiruby_numbered_parameter_upvar()` for `_1`..`_9` |

121 added lines, no line changed or removed. The diff is kept as
`sabiruby-eval-scopes.patch` beside this file; after a `tools/vendor_compiler.sh` refresh,
apply it again (`git apply compiler/vendor/sabiruby-eval-scopes.patch`) and check
`cargo test -p sabiruby-compiler` (the golden tests) and the `gem_eval` file of
`tools/mrbtest.sh`.

## Updating

1. In the reference tree (`../../ref/mruby`, or `MRUBY_SRC`), check out the new tag and run
   `rake` once so that `build/prism/` holds the generated sources for its Prism.
2. `tools/vendor_compiler.sh`
3. Update the table above and `sabiruby_mrc_version()` in `compiler/csrc/shim.c`.
4. Re-apply `sabiruby-eval-scopes.patch` (above).
5. `cargo test -p sabiruby-compiler` (golden tests against the reference `mrbc` output) and
   `tools/mrbtest.sh` (`gem_eval`, `gem_binding`).
