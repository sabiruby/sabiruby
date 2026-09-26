The micro-benchmarks of `docs/worklog/2026-09-26-wait-anywhere.md`: one path each that the
change touched, and a few it did not. Each file lacks its first line, `N = <iterations>`, which
was chosen so that a run takes 50–300 ms (the list is in the worklog); they were compiled with the
reference `mrbc` and run with a copy of `tools/bench_ab.sh` pointed at their directory.
