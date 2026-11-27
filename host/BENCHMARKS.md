# Benchmarks

Machine: primo, 13th Gen Intel Core i7-13620H, 16 threads, 62 GiB RAM; cargo 1.100.0 (Arch rust 1:1.100.0-1).
Harness: hand-rolled on `std::time` (`benches/hot_paths.rs`, `harness = false`), zero new dependencies. Benches run in the bench (release) profile; examples run in the dev profile.
Load caveat: primo was under heavy multi-lane load when these numbers were measured (2026-11-27 night). Treat every number as a baseline, not a peak figure.


Workload definitions (crate: `versemend` host; the crate has no headless sim core yet — its separable deterministic surface is the embedded Lua selftest harness, exercised end-to-end by `examples/end_to_end.rs`):

- `metrics_of`: 200,000 fixed text-metric lookups cycling the seven metric kinds.
- `selftest_run`: one full run of the game's embedded 82-check selftest (`run_selftest`), producing a 5,200-byte transcript with 0 failures.
- `twin_run_determinism`: two selftest runs plus a transcript-hash compare.

Measured (2026-11-27):

- metrics_of: 200,000 lookups in 399.4 µs — **2.0 ns/op**
- selftest_run: 1 run in **74.2 ms** (failures 0, hash 8e590e6c68b03d8e)
- twin_run_determinism: 2 runs in 199.8 ms, hashes equal

Example (`examples/end_to_end.rs`): `failures=0 transcript_bytes=5200 hash=8e590e6c68b03d8e`, identical on re-run. The `versemend-jitrunner` crate is a binary-only spike with no library surface, so it carries no example/bench of its own; the host crate covers the repo.
