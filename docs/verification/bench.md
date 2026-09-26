# Benchmarks: SabiRuby vs mruby 4.1.0-rc

Measured with `tools/bench.sh` (`bench/src/*.rb`, compiled by the reference `mrbc`; the reference `mruby`
runs inside the Docker image on the same machine, so read the ratio and not the milliseconds). Every
run is pinned to a P core (`--core 2`; this machine mixes P and E cores and an E core is 20% slower).
The raw results of every measurement are in `bench/results/<label>.tsv` with a Markdown view beside
them; `tools/bench_compare.sh a.tsv b.tsv` puts two side by side.

**From `96221fb` (2026-09-17) every number in this file is a `codegen-units = 1` build**, which is the
release default from that commit on. Everything above that line was taken with cargo's default of 16
units, so a comparison that crosses it carries the profile change with it — see
[The release profile](#the-release-profile-codegen-units--1-96221fb-2026-09-17) at the end of this
section for what that is worth (−2.0% over the 27).

## The baseline and the stages of `docs/plans/host-bridge-plan.md` (2026-09-15)

Best of 5 (the baseline, `e9da768`) and best of 7 (`5c3eb6e`), by category: milliseconds for
SabiRuby, and the ratio to the reference. `ratio (sum)` weighs every benchmark by how long it runs;
`ratio (median)` is the middle of the per-benchmark ratios.

| category | benchmarks | e9da768 (baseline) | ratio (sum) | ratio (median) | 5c3eb6e (stages 0, 1, 3) | ratio (sum) | change |
|---|---|---:|---:|---:|---:|---:|---:|
| whole program | app_json_hash, app_robot, app_tak, bm_fib | 10511 | 3.40x | 2.87x | 11312 | 3.55x | +7.6% |
| data structures | bm_so_lists, ds_array, ds_hash, ds_string | 7259 | 10.85x | 11.43x | 7395 | 10.91x | +1.9% |
| instruction loop | bm_so_mandelbrot, loop_if_branch, loop_times, loop_while_add, vm_optimization_bench | 28402 | 5.71x | 4.09x | 29464 | 5.80x | +3.7% |
| calls | call_args, call_block_yield, call_fiber, call_kwargs | 4392 | 3.40x | 3.31x | 4541 | 3.48x | +3.4% |
| memory | gc_churn, mem_retained, mem_short_lived | 2320 | 1.06x | 2.44x | 2435 | 1.07x | +5.0% |
| **all** | | 52883 | 4.32x | 3.68x | 55147 | 4.41x | +4.3% |

What the baseline says: the slowness is concentrated in **Hash and String** (`ds_hash` 12.9x, `ds_string`
11.4x, `bm_so_lists` 16.4x) while Array is 4.6x, so it is not the cost of moving values in and out of
slots but something in those two kinds. Calls are 3.0–3.7x, instruction loops 3.6–4.2x
(`vm_optimization_bench` alone 7.1x). `mem_retained` — allocating while 20,000 long-lived objects stay
alive — is *faster* than the reference (0.5x): the reference's generational collector re-marks the live
set; at 100,000 objects it took 31 s to SabiRuby's 1.2 s.

### After stages 2, 2b, 4 and 5 (`9fa5b0b`, 2026-09-15)

Baseline `440d4ba` (main after stages 0, 1, 3) against the merged tree, best of 5, reference included
(`bench/results/440d4ba.tsv`, `9fa5b0b.tsv`):

| category | 440d4ba ms | 9fa5b0b ms | change | ratio to mruby (before → after) |
|---|---:|---:|---:|---|
| whole program | 11302 | 8992 | **−20.4%** | 3.53x → **2.88x** |
| data structures | 7413 | 5848 | **−21.1%** | 10.88x → **8.79x** |
| instruction loop | 29218 | 21618 | **−26.0%** | 5.77x → **4.37x** |
| calls | 4557 | 3768 | **−17.3%** | 3.46x → **2.94x** |
| memory | 2417 | 2053 | **−15.1%** | 1.08x → **0.95x** |
| **all** | 54908 | 42279 | **−23.0%** | 4.40x → **3.47x** (median 3.15x) |

Every one of the 22 benchmarks got faster. Stages 4 and 5 add no cost the benchmarks see (they do not
touch the instruction loop; the merged tree measures the same as the perf branch alone, −22.0%).
How each candidate was measured — alternating A/B rather than best of 5, and why — is in
`docs/worklog/2026-09-15-stage2-perf.md`.

### After stage 2c (`9da4724`, 2026-09-15): `Array#shift` in O(1), Hash writes through one door, an index past 16 entries

Against the previous head and against the original baseline, best of 5, reference included
(`bench/results/9da4724.tsv`):

| category | 9fa5b0b → 9da4724 | e9da768 (baseline) → 9da4724 | ratio to mruby now |
|---|---:|---:|---|
| whole program | −3.0% | −17.0% | 2.84x |
| data structures | **−48.9%** | **−58.9%** | 10.85x → **4.57x** |
| instruction loop | −12.8% | −33.7% | 3.83x |
| calls | +0.8% | −13.5% | 2.98x |
| memory | −2.4% | −13.6% | 0.92x |
| **all** | **−14.0%** | **−31.3%** | 4.32x → **3.01x** (median 3.05x) |

`bm_so_lists` 3.94 s → 1.02 s (16.4x → 4.4x), `ds_hash` 12.9x → 7.7x, `ds_string` 11.4x → 4.5x,
`vm_optimization_bench` 7.1x → 4.6x (it holds a 50,000-entry Hash workload). How the two changes
were measured, and the O(n) that came out of hiding when a borrow changed, are in
`docs/worklog/2026-09-15-stage2c-array-hash.md`.

### After stage 2d (`2aa4f13`, 2026-09-15): Hash without the copy per lookup, `vm_optimization_bench` split

`vm_optimization_bench` now sits under "whole program" and its five parts (`vmo_dispatch`, `vmo_arith`,
`vmo_calls`, `vmo_index`, `vmo_objects`) under "instruction loop", so the category sums are not those
of the tables above; `tools/bench_compare.sh` compares the benchmarks the two files share
(`bench/results/2aa4f13.tsv`, best of 5, reference included):

| | 9da4724 → 2aa4f13 (shared benchmarks) | e9da768 (baseline) → 2aa4f13 | ratio to mruby now (sum / median) |
|---|---:|---:|---|
| all | **−13.5%** | **−40.6%** | **2.82x / 3.05x** |

Per benchmark now: `ds_hash` **2.85x** (was 7.65x after 2c, 12.9x at the baseline), `vmo_objects` (a
50,000-entry Hash) 1.92x, `app_robot` 2.08x, `app_json_hash` 2.82x, `bm_fib` 3.05x, `bm_so_lists` 4.35x,
`ds_string` 4.63x. Memory stays 0.94x. What the small-Hash cost is made of, the O(n) that hid in
`hash_sync`, and why `include?`/`count` were fixed although no benchmark calls them, are in
`docs/worklog/2026-09-15-stage2d-perf.md`.

### After `printf`/`putc`: the two reference benchmarks join the set (`519eb17`, 2026-09-16)

`bm_ao_render` and `bm_mandel_term` have been in `bench/` since the beginning and have been
printing `fail: undefined method 'printf'` / `'putc'` in every result file since the baseline.
`Kernel#printf` and `Kernel#putc` (`leftovers-plan.md` item 5) make them run, and their output
is byte for byte the reference's (3900 bytes of terminal art, 12301 bytes of PPM). The set is
now **27 files, all of which produce a number**; it was 22 files of which 20 ran before stage 2d
cut `vm_optimization_bench` into five, and 27 of which 25 ran after. `tools/bench.sh` discards
what a benchmark prints (`> /dev/null`), so nothing here times a terminal.

Best of 5, reference included, `--core 2` (`bench/results/519eb17.tsv`):

| benchmark | mruby ms | SabiRuby ms | ratio | instructions | ns/instruction |
|---|---:|---:|---:|---:|---:|
| bm_ao_render | 2478 | 5578.982 | **2.25x** | 472066695 | 11.8 |
| bm_mandel_term | 10 | 20.231 | **2.02x** | 4745006 | 4.3 |

`bm_mandel_term` runs for twenty milliseconds, which is short enough that it says more about
start-up than about the loop; it is kept because it is the reference's own file and because it
is the only benchmark in the set whose inner loop is a native that writes.

Totals, next to the previous baseline. The two sets are not the same size, so both are given:
**25 shared** is the benchmarks `2aa4f13` also has a number for, **27 all** is the set as it now
stands.

| | SabiRuby ms | mruby ms | ratio (sum) | ratio (median) |
|---|---:|---:|---:|---:|
| `2aa4f13`, 25 shared | 41022 | 14529 | 2.82x | 3.05x |
| `519eb17`, 25 shared | 42251 | 15343 | **2.75x** | **3.08x** |
| `519eb17`, 27 all | 47851 | 17831 | **2.68x** | **2.87x** |

The milliseconds are 3% higher than `2aa4f13`'s and the reference's are 5.6% higher: this run
sits in a slower window of the machine, which is why only the ratio is read (`../design/optimizations.md` §4).
By category, all 27 (`bench/results/519eb17.md`):

| category | mruby ms | SabiRuby ms | ratio (sum) | ratio (median) |
|---|---:|---:|---:|---:|
| whole program | 8947 | 25563 | 2.86x | 2.85x |
| data structures | 1170 | 3642 | 3.11x | 3.14x |
| instruction loop | 3345 | 9878 | 2.95x | 3.22x |
| calls | 2140 | 6647 | 3.11x | 2.87x |
| memory | 2229 | 2122 | 0.95x | 2.20x |
| **all** | 17831 | 47851 | **2.68x** | **2.87x** |

Two methods more in `Kernel` is two more entries in a method table and a different code layout,
so it was checked rather than assumed: interleaved A/B of the binary before and after, 7 rounds,
`bm_fib` −0.6%, `call_args` +0.9%, `ds_hash` −0.6%, `loop_while_add` +0.2%
(`bench/results/ab-287826e-printf.tsv`). Nothing moved.

**The first attempt at this baseline was thrown away.** It completed, but its best-to-median
spread averaged 3.3% and reached 7.7% (`loop_while_add`), where every earlier baseline sits at
0.8–0.9%. §4's rule — over 1%, take it again — applies to the whole-set run as much as to an A/B.
The kept run is 0.8% mean, 2.7% worst.

### The third round of speed: `perf3-plan.md` stages 3a and 3b (`a0ef97e`, 2026-09-16)

Four changes went in ([`../design/optimizations.md`](../design/optimizations.md) #18-#21; how each was measured, and
the four candidates that were dropped, in [`../worklog/2026-09-16-perf3.md`](../worklog/2026-09-16-perf3.md)):

| stage | commit | what changed | measured alone (interleaved A/B, 7 rounds, `codegen-units=1`) |
|---|---|---|---|
| 3a | `a423a92` | `String#<<` grows the string in place | `ds_string` −5.4% and −6.1% (two runs, default build), −4.8% (cgu=1); one million appends 268 → 152 ms |
| 3a | `f83cc81` | eleven String natives borrow instead of copying | `ds_string` **−11.3%**; `String#+` −19.7%, `size` −11.3%, `==` −3.0%, empty loop −0.1% |
| 3b | `e26ccff` | one guard over the four things the loop head asks | all eight benchmarks −1.1 to −4.6% (`bm_fib` −4.6%, `loop_while_add` −4.2%) |
| 3b | `a0ef97e` | `OP_ADD` and the comparisons answer two numbers where they stand | all eight −0.9 to −6.7% (`bm_so_mandelbrot` −6.7%, `bm_fib` −4.2%, `vmo_arith` −4.0%) |

**Read the two totals below together.** The per-candidate numbers are `codegen-units=1` builds, which is what
isolates the change; the whole-set run is the default build, which is what the earlier baselines are, and the default
build re-rolls its code-generation-unit partitioning at every edit (see §4 of `optimizations.md`: the same
`String#<<` change measured `bm_so_mandelbrot` **+16%** in the default build and −2.5% at `codegen-units=1`,
both reproduced).

Best of 5, reference included, `--core 2` (`bench/results/a0ef97e.tsv`; the run is quiet on SabiRuby's side,
0.68% mean best-to-median spread and 1.62% worst, but the reference column is not — it averages 4.06% and
reaches 33% on `call_kwargs`, which is Docker start-up jitter and is why its ratio column moves):

| | SabiRuby ms | mruby ms | ratio (sum) | ratio (median) |
|---|---:|---:|---:|---:|
| `519eb17`, 27 all | 47851 | 17831 | 2.68x | 2.87x |
| `a0ef97e`, 27 all | **47254** | 17656 | **2.68x** | 3.05x |

Per benchmark against `519eb17` (`tools/bench_compare.sh`), SabiRuby only:

| benchmark | 519eb17 | a0ef97e | change | | benchmark | 519eb17 | a0ef97e | change |
|---|---:|---:|---:|---|---|---:|---:|---:|
| ds_string | 446.452 | 384.116 | **−14.0%** | | bm_so_mandelbrot | 1412.855 | 1482.693 | +4.9% |
| ds_hash | 228.295 | 220.120 | −3.6% | | app_json_hash | 1247.028 | 1300.003 | +4.2% |
| mem_short_lived | 1010.371 | 974.941 | −3.5% | | bm_mandel_term | 20.231 | 20.957 | +3.6% |
| bm_ao_render | 5578.982 | 5407.529 | −3.1% | | call_block_yield | 1016.881 | 1051.558 | +3.4% |
| bm_fib | 5333.604 | 5179.682 | −2.9% | | gc_churn | 381.100 | 393.169 | +3.2% |
| ds_array | 860.198 | 840.008 | −2.3% | | call_kwargs | 851.606 | 866.800 | +1.8% |
| vm_optimization_bench | 11071.396 | 10842.976 | −2.1% | | bm_so_lists | 973.053 | 988.293 | +1.6% |
| vmo_dispatch | 2172.188 | 2126.015 | −2.1% | | mem_retained | 730.217 | 740.931 | +1.5% |
| call_fiber | 1012.895 | 994.228 | −1.8% | | loop_times | 859.266 | 869.921 | +1.2% |
| app_robot | 1178.589 | 1163.310 | −1.3% | | loop_while_add | 841.535 | 848.913 | +0.9% |
| app_tak | 1132.832 | 1118.714 | −1.2% | | vmo_index | 514.448 | 517.944 | +0.7% |
| vmo_arith | 3700.926 | 3657.410 | −1.2% | | vmo_objects | 619.491 | 621.730 | +0.4% |
| loop_if_branch | 890.735 | 884.099 | −0.7% | | call_args | 875.563 | 875.640 | +0.0% |
| vmo_calls | 2889.959 | 2882.313 | −0.3% | | | | | |

| category (sum) | 519eb17 | a0ef97e | change |
|---|---:|---:|---:|
| whole program | 25563 | 25033 | −2.1% |
| data structures | 3642 | 3572 | −1.9% |
| instruction loop | 9878 | 9869 | −0.1% |
| calls | 6647 | 6671 | +0.4% |
| memory | 2122 | 2109 | −0.6% |
| **all** | 47851 | **47254** | **−1.2%** |

The whole stage measured the way a candidate is measured -- `ad8e7cb` against `a0ef97e`, both built at
`codegen-units=1`, interleaved, 7 rounds, all 27 benchmarks (`bench/results/ab-perf3-cgu1.tsv`):

| benchmark | A (`ad8e7cb`) | B (`a0ef97e`) | change | | benchmark | A | B | change |
|---|---:|---:|---:|---|---|---:|---:|---:|
| ds_string | 426.084 | 361.191 | **−15.2%** | | vmo_dispatch | 2245.858 | 2121.305 | −5.5% |
| bm_mandel_term | 21.805 | 18.512 | **−15.1%** | | app_tak | 1170.444 | 1112.957 | −4.9% |
| bm_so_mandelbrot | 1482.835 | 1283.357 | **−13.5%** | | loop_times | 888.265 | 846.144 | −4.7% |
| loop_while_add | 879.454 | 807.791 | −8.1% | | gc_churn | 372.394 | 355.838 | −4.4% |
| bm_fib | 5508.710 | 5100.221 | −7.4% | | loop_if_branch | 868.205 | 833.022 | −4.1% |
| vmo_objects | 610.204 | 569.295 | −6.7% | | vm_optimization_bench | 11293.267 | 10892.173 | −3.6% |
| call_kwargs | 874.903 | 824.336 | −5.8% | | vmo_calls | 2912.025 | 2818.037 | −3.2% |
| vmo_arith | 3842.531 | 3617.880 | −5.8% | | ds_array | 849.095 | 822.723 | −3.1% |
| call_block_yield | 1050.790 | 992.785 | −5.5% | | call_fiber | 897.887 | 870.789 | −3.0% |
| call_args | 908.201 | 860.662 | −5.2% | | ds_hash | 220.896 | 214.499 | −2.9% |
| app_json_hash | 1231.917 | 1207.885 | −2.0% | | bm_ao_render | 5504.783 | 5460.360 | −0.8% |
| vmo_index | 505.549 | 501.356 | −0.8% | | bm_so_lists | 922.702 | 917.901 | −0.5% |
| app_robot | 1146.517 | 1160.472 | +1.2% | | mem_short_lived | 991.572 | 1008.322 | +1.7% |
| mem_retained | 716.917 | 738.088 | +3.0% | | | | | |

| category | A ms | B ms | change |
|---|---:|---:|---:|
| whole program | 25877 | 24953 | −3.6% |
| data structures | 3535 | 3387 | −4.2% |
| instruction loop | 10207 | 9509 | **−6.8%** |
| calls | 6644 | 6367 | −4.2% |
| memory | 2081 | 2102 | +1.0% |
| **all** | 48344 | 46318 | **−4.2%** |

Twenty-four of the twenty-seven are faster, and the three that are not are +1.2 to +3.0%. That is what the
four changes are worth to the code; **−1.2%** is what the shipped binary does today, because the default
profile's code-generation-unit partitioning took the rest back.

The one benchmark that moved on its own terms in the default build is `ds_string` (**4.33x → 3.77x**). The rest is a −1.2% total
where the four changes are worth more than that in isolation and the code-generation-unit roll takes the
difference back: `bm_so_mandelbrot` +4.9% here is the same benchmark the arithmetic fast path measured
**−6.7%** on at `codegen-units=1`, and it calls no string method and takes no keyword argument. Which of the
two numbers is "the truth" depends on what is being asked — the isolated A/B answers "is the change right",
the whole-set run answers "what does the binary we ship do today".

### `items()` without the copy, where the answer is small (`e27d9a4`, `87ad19b`, `e877d17`, 2026-09-16)

`leftovers-plan.md` item 6. No benchmark in the set calls the methods that changed, and the eight
that were A/B'd moved −1.3% to +2.9% (code layout; `bench/results/ab-519eb17-items-g1.tsv`,
`ab-items-g1-g2.tsv`). The numbers that say anything are the micro loops of `bench/micro`, run
with `tools/ab_micro.sh` (interleaved, 7–9 rounds, `--core 2`):

| micro | change | floor in the same round (empty loop) |
|---|---:|---:|
| `m_ary_read` — `fetch`/`at` on a 1000-element array | **−52.4%** | +2.2% |
| `m_ary_ends` — `first(3)`/`last(3)`/`take`/`drop` of 1000 | **−59.1%** | +2.2% |
| `m_ary_dup` — `dup`/`clone`/`replace` of 200 | **−16.6%** | +0.3% |
| `m_ary_walk` — `index`/`count {}`/`assoc` of 200 | −2.9% | −2.0% |

The last row is the one that did *not* pay: subtract the round's floor and it is about −1%. It was
taken anyway, because it makes `assoc`/`rassoc`/`__ary_index` re-read the array each step the way
`ary_assoc` in mruby-array-ext does — a shape, not a speed-up. Full numbers, including the
candidates that were left out, in
[`../worklog/2026-09-16-leftovers-perf.md`](../worklog/2026-09-16-leftovers-perf.md).

Per stage (each measured against the commit before it):

| stage | commit | what changed | all | notes |
|---|---|---|---:|---|
| 0 | `354b6bb` | opcode decode by table, no `unsafe` | +2.5% | dispatch-bound loops +3–7% (`loop_times` +6.6%), heavy-instruction benchmarks 0% (`bm_so_lists`). The `transmute` had no load; the table has one per instruction. To be won back in stage 2 |
| 1 | `9837294` | the benchmarks themselves | — | no VM change |
| 2 c0 | `5119773` | `Op::from_u8` as a 119-arm `match`, not a table | −1.7% | wins back stage 0; `objdump` shows the match folded to a range check |
| 2 c3 | `68261b0` | `find_method` answers a Copy `MethodRef` on the dispatch path | −2.1% | wins back stage 3: `bm_fib` −5.2%, `call_args` −4.7% |
| 2 c1 | `fcf0772` | the instruction loop reads three fields of `CallInfo`, not a clone of all thirteen | −2.8% | the widest win: nearly everything faster |
| 2 c2 | `815ba7f` | `op_counts` off by default, and a fixed array when on (`Vm::set_op_counting`) | **−5.6%** | 20–25% on dense loops: a `Vec` pointer reload and a store→load dependency per instruction |
| 2 c4 | `4a6826e` | a method cache invalidated by `Heap::method_serial` | −1.3% | every mutable access to a `ClassData` goes through `class_mut`, which bumps the serial |
| 2b | `05f6f28` | `String#[]` without copying the whole string; one borrow per Hash scan | −8.7% | `ds_string` 7.9× faster, `ds_hash` −25 to −32% |
| 4, 5 | `ccc60de`, `c667a9d` | `define_fn`, `ObjKind::Data` | — | not on the instruction loop |
| 2c A | `35d58a5` | `ArrayData { buf, start }`: `shift`/`unshift`/`insert(0)` in O(1) | −6.1% | `bm_so_lists` −73%; `ds_array` first +155% (a hidden O(n) once `&Vec` became `&[Slot]`), fixed by reading the length without copying |
| 2c B1 | `8ba2336` | `HashData` fields private, writes through seven methods | −0.3% | no behaviour change; the door the index needs |
| 2c B2 | `e2b026b` | an index (`hash → entry`, one chain per bucket) past 16 entries | −7.1% | `ds_hash` −19.8%, `vm_optimization_bench` −14.4% |
| 2d | `07659aa` | `hash_sync` asks whether the cached hashes are stale before copying every key | **−11.7%** | `ds_hash` −62.8%, `vmo_objects` −85%, `call_kwargs` −15%: one copy of every key per lookup, gone |
| 2d | `2521b79` | one `key_hash` per store; a String key copied only when inserted | −0.7% | a store to an existing key 138 → 45 ns |
| 2d | `f222934` | `Array#include?`/`member?`/`count` read one element, not a copy per element | +1.3% (layout) | O(n²) → O(n); no benchmark calls them |
| 2d | `17ab5dc` | `vm_optimization_bench` cut into five | — | the 50,000-entry Hash part is now `vmo_objects` |
| 6c | `6314b84` | a Ruby `method_missing` runs in the caller's frame (as the reference's) | ±1% (noise) | `call_args` +1.1%, `bm_fib` +0.7% over 15 rounds; the same change moved `bm_fib` −0.1% in another run |
| ECS | `ad54ed4` | `OP_GETIDX`/`GETIDX0`/`SETIDX` as the reference: fast paths for Array/Hash/String, else a send in the caller's frame | +0.1% | data structures −3.7% (`ds_hash` −8 to −9%, `vmo_index` −7%); `call_kwargs` +8% is code layout (candidates that fixed it cost +12–20% elsewhere) |
| leftovers 1–4, 8, 9 | `0ea6411` → `1ae258f` | `**{}` to natives, `method_missing` packing, definition hooks (no A/B at merge time; taken afterwards) | −1.3% (27 benches, 7 rounds, `ab-leftovers-check.tsv`) | `call_fiber` +4.9%, `vmo_index` +6.9%, `loop_if_branch` +4.2% against `bm_so_mandelbrot` −10.5%, `call_kwargs` −6.9%: the cgu=16 layout lottery, not the change |
| 5 | `519eb17` | `Kernel#printf`/`#putc`, so `bm_ao_render` and `bm_mandel_term` run | ±1% (noise) | not a speed change: two entries more in `Kernel`. A/B over 7 rounds: −0.6 to +0.9% |
| 6 | `e27d9a4`, `87ad19b`, `e877d17` | natives that read one element, or a few, borrow the array instead of copying it; `dup`/`replace` copy the Slots once instead of twice | ±0 (benchmarks) | micro: `fetch`/`at` −52%, `first`/`last`/`take`/`drop` −59%, `dup`/`clone`/`replace` −17% |
| release profile | `96221fb` | `[profile.release] codegen-units = 1`, the release default from here on | **−2.0%** | not a code change: the same tree built twice. 22 of 27 faster, the rest +0.0 to +2.5%; binary −16.8%; clean release build 9.2 s → 18.1 s (`bench/results/ab-cgu1.tsv`) |
| 3 | `5c3eb6e` (merge of `93824b1`, `1472346`) | `Method::Closure`, host state | +1.8% | `bm_fib` +4.2%, `call_args` +4.0%, `app_tak` +3.8%: `Method`'s `Clone` is no longer a plain copy and `find_method` clones one per call (stage 2, candidate 3, removes that). `loop_while_add` +6.6% (reproduced twice, A/B on a quiet machine: 1100 → 1190 ms) is not explained by that — the loop makes no calls; `loop_times` moved −5.8% at the same time, so code layout is the likely cause. Data structures unchanged |

### The release profile: `codegen-units = 1` (`96221fb`, 2026-09-17)

Author's decision, taken on the evidence of `2026-09-16-perf3.md` 0: the crate had no
`[profile.release]`, so release builds used cargo's default of **16 code generation units**, and which
function landed in which unit changed whenever a line did. That is the "code layout" noise §4 of
[`../design/optimizations.md`](../design/optimizations.md) keeps warning about, and it was large enough
to swamp a real change: `String#<<` in place measured `bm_so_mandelbrot` **+16%** in the default build,
twice, and −2.5% at one unit. `96221fb` sets `codegen-units = 1`.

**What this means for this file.** The split that the perf3 work had to live with — candidates measured
at `codegen-units=1`, the whole-set baseline at the default — is gone: from `96221fb` the shipped binary,
the A/B binaries and the baseline are the same build, and there is one number per question again.
Numbers above this section are 16-unit builds; the A/B below is what the change from 16 to 1 is worth,
so a comparison that crosses `96221fb` should subtract it.

The two binaries are **the same tree** (`abf5f80`, this branch's base), built twice, once with each
profile, and measured interleaved, 7 rounds, all 27 benchmarks, `--core 2`
(`bench/results/ab-cgu1.tsv`). A is 16 units, B is 1:

| benchmark | A (16) | B (1) | change | | benchmark | A (16) | B (1) | change |
|---|---:|---:|---:|---|---|---:|---:|---:|
| bm_mandel_term | 20.643 | 17.816 | **−13.7%** | | ds_array | 842.912 | 824.811 | −2.1% |
| bm_so_mandelbrot | 1453.743 | 1281.047 | **−11.9%** | | vm_optimization_bench | 10856.764 | 10631.512 | −2.1% |
| call_fiber | 983.362 | 879.662 | **−10.5%** | | loop_times | 856.891 | 847.564 | −1.1% |
| gc_churn | 390.719 | 358.692 | −8.2% | | vmo_calls | 2831.081 | 2798.909 | −1.1% |
| call_block_yield | 1047.834 | 978.497 | −6.6% | | vmo_arith | 3635.565 | 3601.511 | −0.9% |
| bm_so_lists | 978.007 | 920.658 | −5.9% | | call_args | 865.365 | 858.503 | −0.8% |
| app_json_hash | 1267.914 | 1199.397 | −5.4% | | vmo_objects | 594.712 | 590.409 | −0.7% |
| ds_string | 386.031 | 365.096 | −5.4% | | vmo_index | 499.163 | 499.156 | −0.0% |
| loop_if_branch | 876.283 | 832.416 | −5.0% | | app_tak | 1112.954 | 1112.969 | +0.0% |
| call_kwargs | 858.869 | 819.262 | −4.6% | | bm_ao_render | 5358.476 | 5374.772 | +0.3% |
| loop_while_add | 842.586 | 803.915 | −4.6% | | vmo_dispatch | 2108.332 | 2120.334 | +0.6% |
| ds_hash | 223.244 | 215.437 | −3.5% | | bm_fib | 5097.235 | 5132.304 | +0.7% |
| mem_retained | 740.702 | 715.194 | −3.4% | | mem_short_lived | 967.869 | 991.690 | +2.5% |
| app_robot | 1173.006 | 1138.719 | −2.9% | | | | | |

| category | A (16 units) ms | B (1 unit) ms | change |
|---|---:|---:|---:|
| whole program | 24887 | 24607 | −1.1% |
| data structures | 3524 | 3416 | −3.1% |
| instruction loop | 9773 | 9487 | −2.9% |
| calls | 6587 | 6335 | −3.8% |
| memory | 2099 | 2066 | −1.6% |
| **all** | 46870 | 45910 | **−2.0%** |

Twenty-two of the twenty-seven are faster and the five that are not are +0.0 to +2.5%. The spread is
what §4 predicts of one-unit builds: the change is real but it is not evenly spread, and the benchmarks
that move most (`bm_mandel_term`, `bm_so_mandelbrot`, `call_fiber`) are the ones whose inner loop is
small enough to care where it sits. **One unit does not end the placement lottery; it stops re-rolling
it on every edit.**

It also makes the binary smaller — the `sabiruby` command goes 5,621,344 → 4,678,600 bytes (−16.8%) and
the thumbv7em archives 8.9–11.8% ([`size.md`](size.md)) — and the release build slower to produce: a
clean `cargo build --release -p sabiruby-cli` goes **9.2 s → 18.1 s** (best of 3) although its CPU time
*falls*, 58 s → 39 s, because one unit cannot be split across the machine's 24 threads. `dev` builds,
which is what `cargo test` uses, are untouched.

#### The new baseline (`bench/results/96221fb.tsv`)

Best of 5, reference included, `--core 2`, taken with the same binary as the B column above. The
SabiRuby side is quiet (0.85% mean best-to-median spread, 4.45% worst on `vmo_objects`); the reference
side is not (1.45% mean, 10% on `bm_mandel_term`, which runs for 10 ms), so read the ratio and not the
milliseconds. **25 shared** is the benchmarks `2aa4f13` also has a number for, **27 all** is the set as
it stands.

| | SabiRuby ms | mruby ms | ratio (sum) | ratio (median) |
|---|---:|---:|---:|---:|
| `a0ef97e`, 25 shared (16 units) | 41826 | 15151 | 2.76x | 3.18x |
| `96221fb`, 25 shared (1 unit) | **40594** | 15405 | **2.64x** | **2.86x** |
| `a0ef97e`, 27 all (16 units) | 47254 | 17656 | 2.68x | 3.05x |
| `96221fb`, 27 all (1 unit) | **46061** | 17911 | **2.57x** | **2.85x** |

By category, all 27 (`bench/results/96221fb.md`):

| category | mruby ms | SabiRuby ms | ratio (sum) | ratio (median) |
|---|---:|---:|---:|---:|
| whole program | 9038 | 24713 | 2.73x | 2.65x |
| data structures | 1160 | 3379 | 2.91x | 3.09x |
| instruction loop | 3346 | 9545 | 2.85x | 3.08x |
| calls | 2131 | 6362 | 2.99x | 2.85x |
| memory | 2236 | 2062 | 0.92x | 2.11x |
| **all** | 17911 | 46061 | **2.57x** | **2.85x** |

−2.5% of SabiRuby's milliseconds against `a0ef97e` on the 27, −2.9% on the 25, which is about what the
A/B says the profile is worth; the tree also gained the `leftovers` merge (`abf5f80`) in between, and
that merge's own A/B was −1.3%. The reference ran 1.4% slower in this window than in `a0ef97e`'s, so the
ratio column moves a little more than SabiRuby's own change does — the usual reason for reading both.

**The first attempt at this baseline was thrown away**, as `519eb17`'s was. It completed and its totals
agree with the kept one to 0.4% (45862 ms against 46061), but three polling loops of this session's own
tooling were alive during it and its best-to-median spread averaged 1.00% against the kept run's 0.85%.
§4's own rule — over 1%, take it again — does not get an exception because the first number looked fine.


### Waiting inside blocks: `wait-anywhere` (`3170b63` → `f3c7d9c`, 2026-09-26)

What a task being able to wait inside `instance_exec`, `Method#call`, `Class#new`, `index { }`, `sort { }`, a
Hash's default proc and the rest costs where nothing waits ([`../design/wait-anywhere.md`](../design/wait-anywhere.md);
the whole story, with the builds that were measured and dropped, is
[`../worklog/2026-09-26-wait-anywhere.md`](../worklog/2026-09-26-wait-anywhere.md), "計測").

**How.** Interleaved A/B, 5 runs a round, `--core 2`, the order of A and B swapped every other round
(`bench_ab.sh` always runs A first, and the A/A rounds leaned about +0.8% towards B). A number is the best of all
runs of a side over all rounds. The 27 benchmarks of `bench/` and 21 micro-benchmarks (`bench/micro-wait/`: the
paths the change touched and a few it did not). Another job ran on the PC from 13:24 on; `vmstat -t 10` was kept
for the whole session and every round that saw the PC under 90% idle was dropped. The TSVs are in
`bench/results/wait-anywhere/`.

**The line.** Measured first, on the old binary against itself (A/A): 3 rounds of each set. The largest difference
between the two sides was **2.7%** for the micro-benchmarks and **6.6%** for `bench/` (`gc_churn`; within one round,
values up to 56.6% occur — `vmo_index` hit a slow window). A benchmark counts as slower when its difference is above
that and every round points the same way. Nothing narrower has a measurement behind it on this PC.

Micro-benchmarks, 2 rounds (ms):

| benchmark | `3170b63` | `f3c7d9c` | change | rounds |
|---|---:|---:|---:|---|
| `m_array_new_block` | 143.397 | 140.954 | -1.7% | -1.7 -1.1 |
| `m_catch` | 145.956 | 145.189 | -0.5% | -0.6 -0.5 |
| `m_class_exec` | 82.428 | 76.216 | -7.5% | -7.1 -7.5 |
| `m_class_new_block` | 54.815 | 53.630 | -2.2% | -5.6 -2.2 |
| `m_hash_default_proc` | 73.695 | 54.797 | -25.6% | -25.6 -25.7 |
| `m_hash_hit` | 141.146 | 135.209 | -4.2% | -4.3 -4.2 |
| `m_hash_miss` | 174.862 | 138.392 | -20.9% | -21.1 -20.9 |
| `m_index_arg` | 297.531 | 297.234 | -0.1% | +2.1 -0.1 |
| `m_index_block` | 261.830 | 227.304 | -13.2% | -11.4 -13.2 |
| `m_instance_eval` | 88.460 | 81.664 | -7.7% | -6.4 -7.7 |
| `m_instance_exec` | 92.701 | 84.685 | -8.6% | -9.4 -8.6 |
| `m_method_call` | 83.976 | 80.906 | -3.7% | -3.7 -3.7 |
| `m_method_loop` | 81.839 | 84.226 | +2.9% | +3.5 +0.4 |
| `m_new_plain` | 149.426 | 136.204 | -8.8% | -10.3 -7.2 |
| `m_new_ruby_init` | 208.861 | 198.521 | -5.0% | -4.8 -7.2 |
| `m_plain_loop` | 92.957 | 93.608 | +0.7% | +1.7 -1.7 |
| `m_public_send` | 70.033 | 65.606 | -6.3% | -5.8 -6.3 |
| `m_send` | 122.514 | 122.789 | +0.2% | +0.2 +1.3 |
| `m_sort_block` | 272.629 | 294.657 | +8.1% | +8.7 +8.1 |
| `m_sort_plain` | 242.366 | 250.387 | +3.3% | +3.4 +3.3 |
| `m_yield_loop` | 161.558 | 163.849 | +1.4% | +2.4 +1.4 |

`bench/`, 2 rounds (ms):

| benchmark | `3170b63` | `f3c7d9c` | change | rounds |
|---|---:|---:|---:|---|
| app_json_hash | 1212.711 | 1213.254 | +0.0% | -0.0 +0.0 |
| app_robot | 1133.318 | 1088.464 | -4.0% | -4.8 -3.5 |
| app_tak | 1112.499 | 1131.858 | +1.7% | +0.7 +1.7 |
| bm_ao_render | 5403.696 | 5287.048 | -2.2% | -2.2 -2.2 |
| bm_fib | 5085.598 | 5161.792 | +1.5% | +1.5 +1.4 |
| bm_mandel_term | 18.085 | 18.139 | +0.3% | -0.2 +0.3 |
| bm_so_lists | 933.161 | 987.701 | +5.8% | +5.9 +5.8 |
| bm_so_mandelbrot | 1274.874 | 1281.721 | +0.5% | +0.5 +0.6 |
| call_args | 852.549 | 855.283 | +0.3% | -0.6 +0.9 |
| call_block_yield | 982.258 | 989.729 | +0.8% | +1.0 +0.6 |
| call_fiber | 879.955 | 892.631 | +1.4% | +1.4 +1.9 |
| call_kwargs | 806.828 | 811.449 | +0.6% | +0.6 +0.1 |
| ds_array | 821.587 | 838.725 | +2.1% | +2.2 +1.6 |
| ds_hash | 218.884 | 213.141 | -2.6% | -2.6 -1.4 |
| ds_string | 359.940 | 375.795 | +4.4% | +4.4 +3.4 |
| gc_churn | 364.982 | 361.670 | -0.9% | -0.9 -0.8 |
| loop_if_branch | 835.354 | 840.954 | +0.7% | +0.5 +2.3 |
| loop_times | 835.046 | 843.119 | +1.0% | +1.0 +0.3 |
| loop_while_add | 811.821 | 823.618 | +1.5% | +1.6 -1.4 |
| mem_retained | 722.646 | 682.305 | -5.6% | -5.6 -5.7 |
| mem_short_lived | 980.499 | 871.909 | -11.1% | -11.1 -11.4 |
| vm_optimization_bench | 10698.981 | 11037.000 | +3.2% | +3.2 +2.3 |
| vmo_arith | 3608.763 | 3648.287 | +1.1% | +1.7 +1.1 |
| vmo_calls | 2817.807 | 2870.355 | +1.9% | +1.7 +2.2 |
| vmo_dispatch | 2096.661 | 2158.294 | +2.9% | +2.9 +4.1 |
| vmo_index | 499.364 | 495.782 | -0.7% | -1.1 -0.7 |
| vmo_objects | 590.560 | 604.646 | +2.4% | +4.2 -1.5 |

All 27 together: +0.9% and −0.7% in the two rounds (A/A: +0.3%, −0.1%, −0.0%).

- **Over the line: `sort { |x, y| x <=> y }`, +8.1%.** The block now runs in a frame of its own and the sort is a
  native loop frame that takes one more trip through the instruction loop per comparison (the instruction count goes
  up by exactly the 2,400,000 comparisons). The same loop written in Ruby was +206%. Left as it is and reported:
  the author decides between keeping it, putting `sort! { }` back behind a native boundary, or the plan's stage 3.
- `m_method_loop` +2.9% and `m_sort_plain` +3.3% are over the micro line but run no code the change touched: their
  instruction counts are the same before and after. `bm_so_lists` +5.8%, `ds_string` +4.4%, `vmo_dispatch` +2.9% and
  `bm_fib` +1.5% are inside the line and have the same instruction counts too; only the time per instruction moved.
  Measured one build at a time, `bm_fib` went −0.1%, +3.8%, +4.0%, +0.4%, +4.3% over five builds none of which
  touched its path: this is where `exec_frames` lands ("The release profile", above), and `f3c7d9c` is one of the
  better draws.
- Faster: a Hash's default proc −25.6%, a Hash miss −20.9% (`Class#new` and `Hash#[]` look methods up through the
  method cache now), `index { }` −13.2%, `Object.new` −8.8%, `instance_exec` −8.6%, `mem_short_lived` −11.1%.

**After the author's decisions (2026-09-26, `fcbda97`).** `sort { }` / `sort! { }` went back to the native nested
loop (the author chose to keep `sort`'s comparator a named boundary rather than pay +8.1% on it; waiting inside a
comparison is not something a script needs). Re-measured against `3170b63` with the fixed `bench_ab.sh` (A and B
alternate inside a round): `m_sort_block` **+5.4%** (rounds +3.0 +5.4 +5.4) with an **identical instruction count**
(12,162,140 before and after). Five builds of the same source path scattered from +1.0% to +6.6%, and paths the
change does not touch (`m_sort_plain`, `bm_so_lists`) moved by as much between builds, so the remainder is read as
code placement in `exec_frames` plus at most one extra compare in `unwind_return`. The 2.7% line was set from A/A
runs of one binary and does not cover build-to-build placement; **the author accepted the +5.4% on that reading**
(2026-09-26). Integer-only `sort` without a block now uses `sort_unstable` (equal Integers are one value, so the
order cannot differ): `m_sort_plain` −36.9%. Rounds: `bench/results/wait-anywhere/{sortfin,fin5}-*.tsv`.

## Earlier measurements (the five reference benchmarks, best of 3)


mruby's `benchmark/*.rb`, compiled by the reference `mrbc`. Best of 3 runs. Reference `mruby` runs inside the
Docker image on the same machine (so the numbers are a ratio, not an absolute). Generated by `tools/bench.sh` on 2026-09-12.

| benchmark | mruby ms | SabiRuby ms | ratio | instructions | ns/instruction |
|---|---:|---:|---:|---:|---:|
| bm_ao_render | 2519 | fail: undefined method 'printf' for Object (NoMethodError) | | | |
| bm_fib | 1758 | 6223.078 | 3.53x | 742675842 | 8.4 |
| bm_mandel_term | 11 | fail: undefined method 'putc' for Object (NoMethodError) | | | |
| bm_so_lists | 242 | 3813.499 | 15.75x | 57014750 | 66.9 |
| bm_so_mandelbrot | 885 | 1707.520 | 1.92x | 341836153 | 5.0 |
| gc_churn | 92 | 360.368 | 3.91x | 15000650 | 24.0 |
| vm_optimization_bench | 3351 | 22853.327 | 6.81x | 1796631395 | 12.7 |

## Re-measured after the inspection hooks (2026-09-12)

`src/inspect.rs` added event recording to `frame_env`, `pop_frame`, `handle_raise`,
`unwind_return`, `switch_context` and `gc_collect`. The instruction loop (`exec_frames`) is
unchanged; with recording off (the default) the cost is one `Option` test at those places.
Same machine, same procedure, recording off:

| benchmark | before ms | after ms | change |
|---|---:|---:|---:|
| bm_fib | 6093.638 | 6165.897 | +1.2% |
| bm_so_lists | 3767.137 | 3781.102 | +0.4% |
| bm_so_mandelbrot | 1697.848 | 1611.633 | −5.1% |

All within the ±3% the plan asked for, apart from mandelbrot getting *faster*, which is the
run-to-run noise of this machine. See [`../design/inspect.md`](../design/inspect.md).

## Re-measured after the numeric tower, pack and eval (2026-09-12)

mruby-bigint, -rational, -complex, -cmath, -pack, -eval and -binding went in. The VM's fast
paths were not touched: an Integer operation still takes the `checked_*` route and only its
overflow exit reaches the wide-integer code, and `eval` is a native that calls the host.
`Class#new` changed from allocating directly to sending `allocate` (the reference's
`new_iseq` does), which is one method lookup per `new` — `bm_so_lists` (a list of 10000
objects rebuilt 16 times) shows no change.

| benchmark | before ms | after ms | change |
|---|---:|---:|---:|
| bm_fib | 6282.426 | 6223.078 | −0.9% |
| bm_so_lists | 3817.435 | 3813.499 | −0.1% |
| bm_so_mandelbrot | 1700.850 | 1707.520 | +0.4% |

`vm_optimization_bench` and `gc_churn` run for the first time here: the first needs `Time`
(mruby-time, ported 2026-09-12) and the second is new in the reference tree.

## Re-measured after UTF-8 strings (2026-09-12)

Strings became sequences of characters (the feature `utf8`, on by default; `docs/design/utf8.md`).
The character helpers take how the string is read as an argument, so every string method now
starts with one test of that flag, and the instruction loop is untouched. These three
benchmarks are numeric and list work with no string method in their inner loops, which is what
the numbers say; a string-heavy benchmark is not in mruby's set.

| benchmark | before ms | after ms | change |
|---|---:|---:|---:|
| bm_fib | 6223.078 | 6304.543 | +1.3% |
| bm_so_lists | 3813.499 | 3803.837 | −0.3% |
| bm_so_mandelbrot | 1707.520 | 1674.126 | −2.0% |

Within this machine's run-to-run spread (the same three moved ±1.2% between two runs of the
unchanged VM above).
