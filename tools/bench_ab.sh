#!/bin/bash
# Interleaved A/B of two SabiRuby binaries over bench/*.mrb.
#
#   tools/bench_ab.sh --a <binary> --b <binary> --label NAME [--runs N] [--core N] [filter]
#
# Why this exists beside `tools/bench.sh`: that script measures one binary, all benchmarks,
# one after another. That is only sound when the machine keeps the same speed for the whole
# run. On the machine these were taken on it does not: work outside the Linux VM (the host's
# own scheduler, another build) moves every absolute time by 10-20% over a few minutes, and
# best-of-5 does not see it because every one of the five runs sits inside the same slow
# window. Two such runs, taken twenty minutes apart, then differ by more than any change to
# the VM does.
#
# Alternating A and B *within* a round cancels that drift (and which one goes first alternates
# too, run by run): both binaries meet the same
# machine in the same minute, so the ratio stays put even when the milliseconds do not. Each
# benchmark reports the best and the median of its N rounds, and the change between them.
#
# The result is written as `<out>/<label>.tsv` with both sides in it, so two commits can be
# compared later without re-running either.
set -eu
cd "$(dirname "$0")/.."

DIR=bench
RUNS=${RUNS:-5}
OUT=${BENCH_OUT:-bench/results}
CORE=${BENCH_CORE:-}
LABEL=""
A=""; B=""
FILTER=""

while [ $# -gt 0 ]; do
  case "$1" in
    --a) A=$2; shift 2;;
    --b) B=$2; shift 2;;
    --label) LABEL=$2; shift 2;;
    --runs) RUNS=$2; shift 2;;
    --core) CORE=$2; shift 2;;
    --out) OUT=$2; shift 2;;
    -h|--help) sed -n '2,20p' "$0"; exit 0;;
    *) FILTER=$1; shift;;
  esac
done
[ -n "$A" ] && [ -n "$B" ] && [ -n "$LABEL" ] || { echo "need --a, --b and --label" >&2; exit 1; }
[ -x "$A" ] || { echo "no such binary: $A" >&2; exit 1; }
[ -x "$B" ] || { echo "no such binary: $B" >&2; exit 1; }
PIN=""
[ -n "$CORE" ] && PIN="taskset -c $CORE"

category() { awk -F'\t' -v b="$1" '$1==b {print $2; exit}' $DIR/categories.tsv; }
# best and median of the numbers on stdin
stat2() { sort -n | awk '{a[NR]=$1} END {if (NR) printf "%s\t%s", a[1], a[int((NR+1)/2)]}'; }
one() { { timeout 600 $PIN "$1" run --stats "$2" > /dev/null; } 2>&1 | tail -1 | awk '/^instructions:/ {print $4}'; }

mkdir -p "$OUT"
TSV=$OUT/$LABEL.tsv
echo "label=$LABEL runs=$RUNS a=$A b=$B" >&2
printf 'category\tbenchmark\ta_ms_best\ta_ms_median\tb_ms_best\tb_ms_median\tchange_pct\n' > "$TSV"

for mrb in $DIR/*.mrb; do
  b=$(basename "$mrb" .mrb)
  [ -n "$FILTER" ] && [[ "$b" != *"$FILTER"* ]] && continue
  cat=$(category "$b"); [ -n "$cat" ] || cat=other
  ra=""; rb=""; fail=""
  for i in $(seq "$RUNS"); do
    # which of the two goes first alternates: running A first every time leaned the A/A of
    # 2026-09-26 about +0.8% towards B (docs/worklog/2026-09-26-wait-anywhere.md)
    if [ $((i % 2)) -eq 1 ]; then ta=$(one "$A" "$mrb"); tb=$(one "$B" "$mrb")
    else tb=$(one "$B" "$mrb"); ta=$(one "$A" "$mrb"); fi
    if [ -z "$ta" ] || [ -z "$tb" ]; then fail="fail"; break; fi
    ra="$ra$ta"$'\n'; rb="$rb$tb"$'\n'
  done
  if [ -n "$fail" ]; then
    printf '%s\t%s\tfail\t\t\t\t\n' "$cat" "$b" >> "$TSV"
    printf '  %-24s fail\n' "$b" >&2
    continue
  fi
  sa=$(echo "$ra" | grep -v '^$' | stat2); sb=$(echo "$rb" | grep -v '^$' | stat2)
  ch=$(awk -v a="${sa%%	*}" -v c="${sb%%	*}" 'BEGIN{printf "%+.1f", (c-a)*100/a}')
  printf '%s\t%s\t%s\t%s\t%s\n' "$cat" "$b" "$sa" "$sb" "$ch" >> "$TSV"
  printf '  %-24s A %10s  B %10s  %6s%%\n' "$b" "${sa%%	*}" "${sb%%	*}" "$ch" >&2
done

# the table, and each category weighted by how long it runs
awk -F'\t' -v lab="$LABEL" '
FNR == 1 { next }
$3 == "fail" { next }
{ n++; order[n]=$2; cat[$2]=$1; ab[$2]=$3; am[$2]=$4; bb[$2]=$5; bm[$2]=$6; ch[$2]=$7
  if (!($1 in seen)) { seen[$1]=1; corder[++nc]=$1 }
  asum[$1]+=$3; bsum[$1]+=$5; atot+=$3; btot+=$5 }
END {
  printf "\n# Interleaved A/B: %s\n\n", lab
  printf "| benchmark | A best | A median | B best | B median | change |\n|---|---:|---:|---:|---:|---:|\n"
  for (i=1;i<=n;i++) { b=order[i]
    printf "| %s | %s | %s | %s | %s | %s%% |\n", b, ab[b], am[b], bb[b], bm[b], ch[b] }
  printf "\n| category | A ms | B ms | change |\n|---|---:|---:|---:|\n"
  for (i=1;i<=nc;i++) { c=corder[i]
    printf "| %s | %.0f | %.0f | %+.1f%% |\n", c, asum[c], bsum[c], (bsum[c]-asum[c])*100/asum[c] }
  printf "| **all** | %.0f | %.0f | %+.1f%% |\n", atot, btot, (btot-atot)*100/atot
}' "$TSV"
echo "wrote $TSV" >&2
