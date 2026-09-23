#!/bin/bash
# Turns one result TSV from tools/bench.sh into the Markdown a reader wants:
# a table per category, each with a subtotal, and a summary of the categories.
#   tools/bench_report.sh bench/results/before.tsv [label] > report.md
set -eu
TSV=$1
LABEL=${2:-$(basename "$TSV" .tsv)}

awk -F'\t' -v label="$LABEL" -v tsv="$TSV" '
NR == 1 { next }
{
  cat = $1; b = $2; best = $3; med = $4; insn = $5; rbest = $6; rmed = $7
  if (!(cat in seen)) { seen[cat] = 1; order[++ncat] = cat }
  n = ++cnt[cat]
  B[cat, n] = b; MB[cat, n] = best; MM[cat, n] = med; IN[cat, n] = insn
  RB[cat, n] = rbest; RM[cat, n] = rmed
  if (best + 0 > 0) { sum[cat] += best; total += best }
  if (rbest + 0 > 0) { rsum[cat] += rbest; rtotal += rbest; haveref = 1 }
  if (best + 0 > 0 && rbest + 0 > 0) {
    psum[cat] += best; prsum[cat] += rbest; ptotal += best; prtotal += rbest
    R[cat, ++rn[cat]] = best / rbest; A[++an] = best / rbest
  }
}
# median of the per-benchmark ratios of one category (insertion sort, the lists are short)
function medratio(cat,   i, j, k, m, v) {
  m = rn[cat]; if (m + 0 == 0) return ""
  for (i = 1; i <= m; i++) v[i] = R[cat, i]
  for (i = 2; i <= m; i++) { k = v[i]; for (j = i - 1; j >= 1 && v[j] > k; j--) v[j+1] = v[j]; v[j+1] = k }
  return sprintf("%.2fx", v[int((m + 1) / 2)])
}
function medall(   i, j, k, m, v) {
  m = an; if (m + 0 == 0) return ""
  for (i = 1; i <= m; i++) v[i] = A[i]
  for (i = 2; i <= m; i++) { k = v[i]; for (j = i - 1; j >= 1 && v[j] > k; j--) v[j+1] = v[j]; v[j+1] = k }
  return sprintf("%.2fx", v[int((m + 1) / 2)])
}
function ratio(a, b) { return (b + 0 > 0 && a + 0 > 0) ? sprintf("%.2fx", a / b) : "" }
END {
  printf "# Benchmarks by category: %s\n\n", label
  printf "SabiRuby against mruby 4.1.0-rc2, best and median of the runs `tools/bench.sh` made.\n"
  printf "The reference runs inside the Docker image on the same machine, so read the ratio and not\n"
  printf "the milliseconds. Empty reference columns mean Docker was not there when this was measured.\n"
  printf "Source: `%s`.\n\n", tsv

  printf "## Summary\n\n"
  printf "`ratio (sum)` weighs every benchmark by how long it runs, so one long benchmark speaks for the\n"
  printf "whole category; `ratio (median)` is the middle of the per-benchmark ratios, which one does not.\n\n"
  printf "| category | mruby ms | SabiRuby ms | ratio (sum) | ratio (median) |\n|---|---:|---:|---:|---:|\n"
  for (i = 1; i <= ncat; i++) {
    c = order[i]
    printf "| %s | %s | %.0f | %s | %s |\n", c, (prsum[c] + 0 > 0 ? sprintf("%.0f", prsum[c]) : ""), sum[c], ratio(psum[c], prsum[c]), medratio(c)
  }
  printf "| **all** | %s | %.0f | %s | %s |\n\n", (prtotal + 0 > 0 ? sprintf("%.0f", prtotal) : ""), total, ratio(ptotal, prtotal), medall()

  for (i = 1; i <= ncat; i++) {
    c = order[i]
    printf "## %s\n\n", c
    printf "| benchmark | mruby ms (best) | mruby ms (median) | SabiRuby ms (best) | SabiRuby ms (median) | ratio | instructions | ns/instruction |\n"
    printf "|---|---:|---:|---:|---:|---:|---:|---:|\n"
    for (n = 1; n <= cnt[c]; n++) {
      best = MB[c, n]
      if (best + 0 <= 0) { printf "| %s | | | %s | | | | |\n", B[c, n], best; continue }
      nsi = (IN[c, n] + 0 > 0) ? sprintf("%.1f", best * 1000000 / IN[c, n]) : ""
      printf "| %s | %s | %s | %s | %s | %s | %s | %s |\n",
        B[c, n], RB[c, n], RM[c, n], best, MM[c, n], ratio(best, RB[c, n]), IN[c, n], nsi
    }
    printf "| **subtotal** | %s | | %.0f | | %s | | |\n\n", (prsum[c] + 0 > 0 ? sprintf("%.0f", prsum[c]) : ""), sum[c], ratio(psum[c], prsum[c])
  }
}
' "$TSV"
