#!/bin/bash
# Runs mruby's own test suite (test/assert.rb + test/t/*.rb of the reference tree)
# on SabiRuby and writes docs/verification/mrbtest.md (per-file table + opcode coverage).
#   tools/mrbtest.sh            # compile (Docker, reference mrbc) + run + write docs/verification/mrbtest.md
#   tools/mrbtest.sh --update   # also refresh tests/mrbtest/baseline.txt (the regression floor)
#   tools/mrbtest.sh -v hash    # print the full output of one file
#   tools/mrbtest.sh --bytes …  # the byte-string build (no feature `utf8`): docs/verification/mrbtest-bytes.md
#                               # and tests/mrbtest/baseline-bytes.txt
#   tools/mrbtest.sh --no-regexp … # the build without mruby-regexp (no feature `regexp`):
#                               # docs/verification/mrbtest-noregexp.md and tests/mrbtest/baseline-noregexp.txt.
#                               # The gem's own test files are left out of the run: without the
#                               # gem there is no `Regexp` constant for them to name.
# The test bytecode is the same for both builds (what a string is, is the VM's to say), so
# either mode may compile it; which assertions run is decided at run time by `__ENCODING__`.
set -eu
cd "$(dirname "$0")/.."
IMG=kishima/mruby:4.1.0-rc2
MRUBY=${MRUBY_SRC:-../../ref/mruby}
DIR=tests/mrbtest
FEATURES=""
OUT=docs/verification/mrbtest.md
BASE=$DIR/baseline.txt
MODE="characters (feature \`utf8\`, the default)"
# mruby-regexp's `spec.build_settings`: the unicode_* and ascii_* test files assert opposite
# things about the same patterns, so each pair belongs to exactly one of the two builds. Both
# are compiled either way (the bytecode is the same); only the run leaves one pair out.
OTHER_BUILD='gem_ascii_case|gem_ascii_ctype'
# mruby-regexp's own test files (mrbgems/mruby-regexp/test/*.rb, copied as gem_*): without the
# gem there is no `Regexp` for them to name, so a build without it leaves exactly these out.
# `regexperror.rb` of mruby's core tests stays in - `RegexpError` is a core class (15.2.27) and
# the file's one assertion is commented out on the reference anyway.
REGEXP_FILES='gem_regexp|gem_match_data|gem_string_regexp|gem_symbol_regexp|gem_string_index|gem_backref_scope|gem_backtracking_stack|gem_unicode_case|gem_unicode_ctype|gem_ascii_case|gem_ascii_ctype'
if [ "${1:-}" = "--bytes" ]; then
  shift
  FEATURES="--no-default-features --features regexp"
  OUT=docs/verification/mrbtest-bytes.md
  BASE=$DIR/baseline-bytes.txt
  MODE="bytes (no feature \`utf8\`)"
  OTHER_BUILD='gem_unicode_case|gem_unicode_ctype'
elif [ "${1:-}" = "--no-regexp" ]; then
  shift
  FEATURES="--no-default-features --features utf8"
  OUT=docs/verification/mrbtest-noregexp.md
  BASE=$DIR/baseline-noregexp.txt
  MODE="characters (feature \`utf8\`), without mruby-regexp (no feature \`regexp\`)"
  OTHER_BUILD=$REGEXP_FILES
fi
mkdir -p $DIR/src
if [ "${1:-}" = "-v" ]; then
  cargo run --release -q -p sabiruby-cli $FEATURES -- mrbtest -v $DIR/assert.mrb $DIR/prelude.mrb $DIR/${2}.mrb
  exit 0
fi
# copy sources (they are MIT, from mruby test/) and compile.
# Gem tests (mrbgems/<gem>/test/*.rb) are copied as gem_<file>.rb; the gem's
# mrblib is compiled into src/mrblib/<gem>.mrb and loaded by Vm::with_mrblib.
GEMS="mruby-sprintf mruby-metaprog mruby-proc-ext mruby-method mruby-fiber mruby-enumerator mruby-array-ext mruby-enum-ext mruby-hash-ext mruby-range-ext mruby-string-ext mruby-compar-ext mruby-toplevel-ext mruby-enum-chain mruby-enum-lazy mruby-object-ext mruby-symbol-ext mruby-kernel-ext mruby-class-ext mruby-numeric-ext mruby-catch mruby-objectspace mruby-math mruby-random mruby-struct mruby-data mruby-set mruby-time mruby-bigint mruby-rational mruby-complex mruby-cmath mruby-pack mruby-eval mruby-binding mruby-proc-binding mruby-regexp mruby-task mruby-sleep mruby-strftime"
# the gem test files are copied afresh (the collision rule below looks at what this run copied)
rm -f $DIR/src/gem_*.rb $DIR/gem_*.mrb
cp "$MRUBY/test/assert.rb" $DIR/src/
cp "$MRUBY"/test/t/*.rb $DIR/src/
mkdir -p target/mrblib
for g in $GEMS; do
  short=${g#mruby-}
  # a test file named like an earlier gem's (numeric.rb of string-ext and numeric-ext)
  # is qualified with its gem: gem_<gem>_<file>.rb
  for rb in "$MRUBY"/mrbgems/$g/test/*.rb; do
    dst=$DIR/src/gem_$(basename "$rb")
    [ -e "$dst" ] && dst=$DIR/src/gem_${short//-/_}_$(basename "$rb")
    cp "$rb" "$dst"
  done
  if ls "$MRUBY"/mrbgems/$g/mrblib/*.rb >/dev/null 2>&1; then
    cat "$MRUBY"/mrbgems/$g/mrblib/*.rb > target/mrblib/mrblib_$short.rb
    docker run --rm -v "$PWD/target/mrblib:/w" $IMG mrbc -o /w/$short.mrb /w/mrblib_$short.rb
    cp target/mrblib/$short.mrb src/mrblib/$short.mrb
  fi
done
# The helpers the gem test files share. The reference's driver links every test file into one
# program, so a helper one file defines is there for the next; this runs a file at a time, so
# they are extracted here and loaded after assert.rb for every file.
sed -n '/^RE_TESTS_NEED_STACK/,/^end$/p' "$MRUBY"/mrbgems/mruby-regexp/test/backtracking_stack.rb > $DIR/src/prelude.rb
# -g keeps the LVAR section (local variable names): the reference test driver
# compiles the tests from source, so `local_variables` and `Proc#parameters`
# see the names there too.
docker run --rm -v "$PWD/$DIR:/w" $IMG /bin/sh -c '
  mrbc -g -o /w/assert.mrb /w/src/assert.rb
  for rb in /w/src/*.rb; do b=$(basename "$rb" .rb); [ "$b" = assert ] && continue
    mrbc -g -o /w/$b.mrb "$rb" || echo "compile failed: $b"; done'
cargo build --release -q -p sabiruby-cli $FEATURES
{
  echo "# mruby test suite on SabiRuby"
  echo
  echo "Reference: mruby 4.1.0-rc2 \`test/assert.rb\` + \`test/t/*.rb\`, compiled by the reference \`mrbc\`."
  echo "Strings are $MODE."
  echo "Generated by \`tools/mrbtest.sh\` on $(date -u +%Y-%m-%d). Do not edit."
  echo
  ./target/release/sabiruby mrbtest $DIR/assert.mrb $(ls $DIR/*.mrb | grep -v assert.mrb | grep -Ev "$OTHER_BUILD") \
    | awk -F'\t' 'NR==FNR { note[$1]=$2; next }
                  /^\| [a-z_0-9]+ \|/ { n=$0; sub(/^\| /, "", n); sub(/ \|.*/, "", n); if (n in note) sub(/\|  \|$/, "| " note[n] " |") }
                  { print }' $DIR/notes.tsv -
  echo
  echo "Why an assertion does not pass, per file: [\`mrbtest-notes.md\`](mrbtest-notes.md) (hand-written; the \`note\` column comes from \`tests/mrbtest/notes.tsv\`)."
} > $OUT
cat $OUT
if [ "${1:-}" = "--update" ]; then
  grep '^| [a-z_0-9]* |' $OUT | grep -v '^| file |' | awk -F'|' '{gsub(/ /,"",$2); gsub(/ /,"",$4); print $2, $4}' > $BASE
  echo "baseline updated: $BASE"
fi
