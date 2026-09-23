#!/bin/bash
# Runs the benchmarks of bench/ on SabiRuby, and on the reference `mruby` (Docker) when
# one is reachable, and writes a machine-readable TSV plus a Markdown report per category.
#
#   tools/bench.sh                              # everything, 5 runs each
#   tools/bench.sh fib                          # only the names containing "fib"
#   tools/bench.sh --label before ds            # name the result file bench/results/before.tsv
#   SABIRUBY_BIN=/path/to/sabiruby tools/bench.sh --label 354b6bb
#
# Options (all have an environment variable of their own):
#   --label NAME   BENCH_LABEL   name of the result files    (default: the git short SHA)
#   --runs N       RUNS          runs per benchmark          (default: 5)
#   --core N       BENCH_CORE    pin every run to this cpu   (default: none)
#   --no-ref       BENCH_NO_REF  skip the reference `mruby` even if Docker answers
#   --out DIR      BENCH_OUT     where the results go        (default: bench/results)
#                  SABIRUBY_BIN  the binary to measure       (default: build it here)
#                  BENCH_DOC     extra path for the Markdown (docs/verification/bench.md is not written
#                                by this script; the maintainer points BENCH_DOC at it)
#                  MRUBY_SRC     the reference tree, for refreshing bench/src/bm_*.rb
#
# The reference runs inside the container (date +%s%N around `mruby`) and SabiRuby on the
# host, both on the same .mrb, so the useful number is the ratio and not the milliseconds.
# Both sides report the best and the median of the runs. Without Docker the reference
# columns stay empty and the ratio is left out: SabiRuby's own numbers still come out, which
# is what comparing two commits of SabiRuby needs.
set -eu
cd "$(dirname "$0")/.."

IMG=kishima/mruby:4.1.0-rc2
MRUBY=${MRUBY_SRC:-../../ref/mruby}
DIR=bench
RUNS=${RUNS:-5}
OUT=${BENCH_OUT:-bench/results}
CORE=${BENCH_CORE:-}
NOREF=${BENCH_NO_REF:-}
LABEL=${BENCH_LABEL:-}
FILTER=""

while [ $# -gt 0 ]; do
  case "$1" in
    --label) LABEL=$2; shift 2;;
    --runs)  RUNS=$2; shift 2;;
    --core)  CORE=$2; shift 2;;
    --out)   OUT=$2; shift 2;;
    --no-ref) NOREF=1; shift;;
    -h|--help) sed -n '2,25p' "$0"; exit 0;;
    *) FILTER=$1; shift;;
  esac
done
[ -n "$LABEL" ] || LABEL=$(git rev-parse --short HEAD 2>/dev/null || echo local)

# The reference is optional: no Docker, no reference columns. Never start the daemon here.
REF=1
[ -n "$NOREF" ] && REF=""
if [ -n "$REF" ] && ! docker info > /dev/null 2>&1; then
  echo "note: docker does not answer, measuring SabiRuby only" >&2
  REF=""
fi

# With the reference at hand, refresh the copied benchmarks and recompile every bench/src
# with the reference mrbc (compiler/tests/golden.rs holds the .mrb to exactly this output).
if [ -n "$REF" ]; then
  mkdir -p $DIR/src
  cp "$MRUBY"/benchmark/bm_*.rb "$MRUBY"/benchmark/vm_optimization_bench.rb $DIR/src/ 2>/dev/null || true
  docker run --rm -v "$PWD/$DIR:/w" $IMG /bin/sh -c \
    'cd /w && for rb in src/*.rb; do b=$(basename $rb .rb); mrbc -o $b.mrb $rb || echo "compile failed: $b" >&2; done'
fi

# The binary under test.
BIN=${SABIRUBY_BIN:-}
if [ -z "$BIN" ]; then
  cargo build --release -q -p sabiruby-cli
  BIN=$PWD/target/release/sabiruby
fi
[ -x "$BIN" ] || { echo "no such binary: $BIN" >&2; exit 1; }
PIN=""
[ -n "$CORE" ] && PIN="taskset -c $CORE"

mkdir -p "$OUT"
TSV=$OUT/$LABEL.tsv
MD=${BENCH_DOC:-$OUT/$LABEL.md}

# category of a benchmark, and the categories in the order they are listed
category() { awk -F'\t' -v b="$1" '$1==b {print $2; exit}' $DIR/categories.tsv; }

# best and median of the numbers on stdin
stat2() { sort -n | awk '{a[NR]=$1} END {if (NR) printf "%s\t%s", a[1], a[int((NR+1)/2)]}'; }

echo "label=$LABEL runs=$RUNS bin=$BIN ref=${REF:-none}" >&2
printf 'category\tbenchmark\tms_best\tms_median\tinstructions\tref_ms_best\tref_ms_median\n' > "$TSV"

for mrb in $DIR/*.mrb; do
  b=$(basename "$mrb" .mrb)
  [ -n "$FILTER" ] && [[ "$b" != *"$FILTER"* ]] && continue
  cat=$(category "$b"); [ -n "$cat" ] || cat=other

  ours=""; insn=""; fail=""
  for _ in $(seq "$RUNS"); do
    st=$( { timeout 600 $PIN "$BIN" run --stats "$mrb" > /dev/null; } 2>&1 | tail -1 )
    if [[ "$st" == instructions:* ]]; then
      ours="$ours$(echo "$st" | awk '{print $4}')"$'\n'
      insn=$(echo "$st" | awk '{print $2}')
    else
      fail="fail: $(echo "$st" | cut -c1-60)"; break
    fi
  done

  ref=""
  if [ -n "$REF" ] && [ -z "$fail" ]; then
    for _ in $(seq "$RUNS"); do
      ref="$ref$(docker run --rm -v "$PWD/$DIR:/w:ro" $IMG /bin/sh -c \
        "s=\$(date +%s%N); timeout 600 mruby /w/$b.mrb > /dev/null 2>&1; e=\$(date +%s%N); echo \$(( (e - s) / 1000000 ))")"$'\n'
    done
  fi

  if [ -n "$fail" ]; then
    printf '%s\t%s\t%s\t\t\t\t\n' "$cat" "$b" "$fail" >> "$TSV"
    printf '  %-24s %s\n' "$b" "$fail" >&2
  else
    printf '%s\t%s\t%s\t%s\t%s\n' "$cat" "$b" "$(echo "$ours" | grep -v '^$' | stat2)" "$insn" \
      "$(echo "$ref" | grep -v '^$' | stat2)" >> "$TSV"
    printf '  %-24s %s ms\n' "$b" "$(echo "$ours" | grep -v '^$' | sort -n | head -1)" >&2
  fi
done

tools/bench_report.sh "$TSV" "$LABEL" > "$MD"
echo "wrote $TSV and $MD" >&2
cat "$MD"
