#!/bin/bash
# Compile tests/fixtures/*.rb with the reference mruby (Docker image kishima/mruby:4.1.0-rc2)
# into .mrb and record the reference output (.out) and the verbose dump (.dump).
# The dump's pointers (`irep 0x...`) change on every run (ASLR) and are written as 0xADDR,
# inside the container because the files it writes are root's (as the book's tools/build_kit.rb).
#   tools/fixtures.sh            # all fixtures
#   tools/fixtures.sh hello      # one fixture
# Also builds the Ruby part of mruby's core library into src/mrblib/core.mrb,
# src/mrblib/require.rb (SabiRuby's own require/load) into src/mrblib/require.mrb, and
# src/mrblib/block-frames.rb (the loops of block-taking natives) into src/mrblib/block-frames.mrb.
set -eu
cd "$(dirname "$0")/.."
IMG=kishima/mruby:4.1.0-rc2
# the same mruby built with MRB_UTF8_STRING, for the fixtures whose answer depends on how a
# string is read (`docs/design/utf8.md`)
IMG_UTF8=kishima/mruby:4.1.0-rc2-utf8
MRUBY=${MRUBY_SRC:-../../ref/mruby}
if [ -d "$MRUBY/mrblib" ]; then
  mkdir -p target/mrblib
  cat "$MRUBY"/mrblib/*.rb > target/mrblib/mrblib_all.rb
  docker run --rm -v "$PWD/target/mrblib:/w" $IMG mrbc -o /w/core.mrb /w/mrblib_all.rb
  cp target/mrblib/core.mrb src/mrblib/core.mrb
fi
# SabiRuby's own Ruby part: require/load (`docs/plans/eval-require-plan.md` 5). The reference has none
# of it, so this one is compiled from the source that lives beside it.
docker run --rm -v "$PWD/src/mrblib:/w" $IMG mrbc -o /w/require.mrb /w/require.rb
# the same for the loops of the block-taking natives (`docs/design/wait-anywhere.md`)
docker run --rm -v "$PWD/src/mrblib:/w" $IMG mrbc -o /w/block-frames.mrb /w/block-frames.rb
for rb in tests/fixtures/${1:-*}.rb; do
  base=${rb%.rb}
  docker run --rm -v "$PWD/tests/fixtures:/w" $IMG /bin/sh -c "
    mrbc -o /w/$(basename $base).mrb /w/$(basename $rb) &&
    mrbc --verbose /w/$(basename $rb) > /w/$(basename $base).dump 2>&1 &&
    sed -i -E 's/0x[0-9a-f]{6,}/0xADDR/g' /w/$(basename $base).dump &&
    mruby /w/$(basename $rb) > /w/$(basename $base).out 2>&1 || true"
  # a fixture read both ways records the byte-string answer beside the UTF-8 one
  if [ -e "$base-bytes.out" ] || [ "$(basename $base)" = utf8 ]; then
    mv "$base.out" "$base-bytes.out"
    docker run --rm -v "$PWD/tests/fixtures:/w" $IMG_UTF8 /bin/sh -c \
      "mruby /w/$(basename $rb) > /w/$(basename $base).out 2>&1 || true"
  fi
  echo "$base: $(wc -c < $base.mrb) bytes, expected $(wc -l < $base.out) lines"
done
