Task-heavy micro-benchmarks for the 0.7.0 scheduler change (the queues as `BTreeMap`s with
deadline and waiter indexes; `docs/worklog/2026-09-27-release-0.7-bench.md`). Compiled with the
reference `mrbc` into `.mrb` beside them and run with `tools/bench_ab.sh` pointed at this directory
(`DIR`). `bench/micro-wait/` has the ones for the 0.7.0 block paths.
