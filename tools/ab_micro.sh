#!/bin/bash
# Interleaved A/B of two SabiRuby binaries over the micro benchmarks in bench/micro.
#
#   tools/ab_micro.sh --a <binary> --b <binary> --label NAME [--runs N] [--core N] [filter]
#   tools/ab_micro.sh --compile          # rebuild bench/micro/*.mrb from bench/micro/src/*.rb
#
# Why a second script beside `tools/bench_ab.sh`: that one walks `bench/*.mrb`, which is the
# published benchmark set and must not grow every time one native method is measured. A change
# that no benchmark calls (a native that copies an array to read one element of it) still needs
# a number, and that number wants a loop around the one operation and an empty loop beside it
# to subtract. Those loops live in `bench/micro/src/*.rb`, compiled by the reference `mrbc` the
# same way `bench/src` is, and are not part of any published total.
#
# Everything else follows `bench_ab.sh`: one round runs A then B so both meet the same machine
# in the same minute, and each side reports the best and the median of its N rounds.
set -eu
cd "$(dirname "$0")/.."

IMG=kishima/mruby:4.1.0-rc2
DIR=bench/micro
RUNS=${RUNS:-7}
OUT=${BENCH_OUT:-bench/results}
CORE=${BENCH_CORE:-}
LABEL=""
A=""; B=""
FILTER=""
COMPILE=""

while [ $# -gt 0 ]; do
  case "$1" in
    --a) A=$2; shift 2;;
    --b) B=$2; shift 2;;
    --label) LABEL=$2; shift 2;;
    --runs) RUNS=$2; shift 2;;
    --core) CORE=$2; shift 2;;
    --out) OUT=$2; shift 2;;
    --compile) COMPILE=1; shift;;
    -h|--help) sed -n '2,18p' "$0"; exit 0;;
    *) FILTER=$1; shift;;
  esac
done

if [ -n "$COMPILE" ]; then
  docker info > /dev/null 2>&1 || { echo "docker does not answer; cannot compile" >&2; exit 1; }
  docker run --rm -v "$PWD/$DIR:/w" $IMG /bin/sh -c \
    'cd /w && for rb in src/*.rb; do b=$(basename $rb .rb); mrbc -o $b.mrb $rb || echo "compile failed: $b" >&2; done'
  echo "compiled $DIR/*.mrb" >&2
  [ -n "$A" ] || exit 0
fi

[ -n "$A" ] && [ -n "$B" ] && [ -n "$LABEL" ] || { echo "need --a, --b and --label" >&2; exit 1; }
[ -x "$A" ] || { echo "no such binary: $A" >&2; exit 1; }
[ -x "$B" ] || { echo "no such binary: $B" >&2; exit 1; }
PIN=""
[ -n "$CORE" ] && PIN="taskset -c $CORE"

stat2() { sort -n | awk '{a[NR]=$1} END {if (NR) printf "%s\t%s", a[1], a[int((NR+1)/2)]}'; }
one() { { timeout 600 $PIN "$1" run --stats "$2" > /dev/null; } 2>&1 | tail -1 | awk '/^instructions:/ {print $4}'; }

mkdir -p "$OUT"
TSV=$OUT/$LABEL.tsv
echo "label=$LABEL runs=$RUNS a=$A b=$B" >&2
printf 'micro\ta_ms_best\ta_ms_median\tb_ms_best\tb_ms_median\tchange_pct\n' > "$TSV"

for mrb in $DIR/*.mrb; do
  [ -e "$mrb" ] || { echo "no $DIR/*.mrb: run tools/ab_micro.sh --compile" >&2; exit 1; }
  b=$(basename "$mrb" .mrb)
  [ -n "$FILTER" ] && [[ "$b" != *"$FILTER"* ]] && continue
  ra=""; rb=""; fail=""
  for _ in $(seq "$RUNS"); do
    ta=$(one "$A" "$mrb"); tb=$(one "$B" "$mrb")
    if [ -z "$ta" ] || [ -z "$tb" ]; then fail="fail"; break; fi
    ra="$ra$ta"$'\n'; rb="$rb$tb"$'\n'
  done
  if [ -n "$fail" ]; then
    printf '%s\tfail\t\t\t\t\n' "$b" >> "$TSV"
    printf '  %-24s fail\n' "$b" >&2
    continue
  fi
  sa=$(echo "$ra" | grep -v '^$' | stat2); sb=$(echo "$rb" | grep -v '^$' | stat2)
  ch=$(awk -v a="${sa%%	*}" -v c="${sb%%	*}" 'BEGIN{printf "%+.1f", (c-a)*100/a}')
  printf '%s\t%s\t%s\t%s\n' "$b" "$sa" "$sb" "$ch" >> "$TSV"
  printf '  %-24s A %10s  B %10s  %6s%%\n' "$b" "${sa%%	*}" "${sb%%	*}" "$ch" >&2
done

awk -F'\t' -v lab="$LABEL" '
FNR == 1 { next }
$2 == "fail" { next }
{ printf "| %s | %s | %s | %s | %s | %s%% |\n", $1, $2, $3, $4, $5, $6 }
BEGIN { printf "\n# Interleaved A/B (micro)\n\n| micro | A best | A median | B best | B median | change |\n|---|---:|---:|---:|---:|---:|\n" }
' "$TSV"
echo "wrote $TSV" >&2
