# sabiruby-cli

The `sabiruby` command: runs Ruby source and mruby 4.1 bytecode on the
[SabiRuby](https://crates.io/crates/sabiruby) VM, and compiles like `mrbc` with the reference
compiler ([`sabiruby-compiler`](https://crates.io/crates/sabiruby-compiler), built as C, so a C
compiler is needed to install it).

The switches follow the reference `mruby` command (`sabiruby -h` lists them), and `compile` is
`mrbc`:

```
cargo install sabiruby-cli

sabiruby foo.rb [arguments...]        # Ruby source or a .mrb file; the arguments go to ARGV
sabiruby -e 'p [1, 2].sum'            # one line of script (-e may be repeated)
echo 'puts 1' | sabiruby              # no program file: read it from standard input
sabiruby -c foo.rb                    # check syntax only ("Syntax OK")
sabiruby -v foo.rb                    # version, then the instruction listing, then run
sabiruby -b foo.mrb                   # bytecode only (refuse source)
sabiruby -d foo.rb                    # $DEBUG = true
sabiruby -r lib foo.rb                # require the library first (-r may be repeated)
sabiruby --stats foo.rb               # instructions, time and GC statistics to stderr
sabiruby compile foo.rb -o foo.mrb    # like mrbc: -g, -c, --remove-lv, --no-ext-ops, --no-optimize
sabiruby dump foo.rb                  # instruction listing (.rb or .mrb)
sabiruby mrbtest assert.mrb hash.mrb  # run mruby's test suite and print a Markdown report
sabiruby --version / --copyright
```

`sabiruby run foo.rb` still works. A subcommand name wins over a file of the same name: for a
program called `compile`, run `./compile` or `sabiruby run compile`. `require`/`load` search
the program's own directory and the working directory (`$LOAD_PATH`).

The bytecode is byte-identical to what the reference `mrbc` (mruby 4.1.0-rc) writes; compile
errors are printed as `FILE:LINE:COL: message`, as `mrbc` does. `SABIRUBY_GC_STRESS=1` makes
the VM collect garbage after every allocation (for testing).

The library crates are separate so that the VM stays pure Rust and `no_std`: use
[`sabiruby`](https://crates.io/crates/sabiruby) to embed the VM, and add `sabiruby-compiler`
only if the program itself compiles Ruby source.

MIT. See the [repository](https://github.com/sabiruby/sabiruby) for the design notes, and
[`CHANGELOG.md`](https://github.com/sabiruby/sabiruby/blob/main/CHANGELOG.md) for what changed
in each release.
