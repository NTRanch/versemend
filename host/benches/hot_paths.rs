//! Hot-path benchmarks for the versemend host (hand-rolled, std::time only, no new dependencies): the fixed text-metrics lookup every frame depends on, one full selftest run (the crate's end-to-end deterministic workload), and the determinism twin-run cost.
use std::time::Instant;

/// Metric lookups per timing sample.
const METRIC_LOOKUPS: usize = 200_000;
const KINDS: [&str; 7] = [
    "title",
    "display",
    "code",
    "code_small",
    "small",
    "body",
    "body_bold",
];

/// FNV-1a 64-bit hash: deterministic across runs and platforms.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn selftest_hash() -> Result<(u64, i64), Box<dyn std::error::Error>> {
    let (transcript, failures) = versemend::run_selftest()?;
    Ok((fnv1a(transcript.as_bytes()), failures))
}

fn main() {
    // Workload 1: metrics lookup.
    let start = Instant::now();
    let mut acc: f32 = 0.0;
    for i in 0..METRIC_LOOKUPS {
        let (w, h) = versemend::metrics_of(KINDS[i % KINDS.len()]);
        acc += w + h;
    }
    let elapsed = start.elapsed();
    println!(
        "metrics_of: {} lookups in {:?} ({:.1} ns/op, acc {:.1})",
        METRIC_LOOKUPS,
        elapsed,
        elapsed.as_nanos() as f64 / METRIC_LOOKUPS as f64,
        acc
    );

    // Workload 2: one full selftest run.
    let start = Instant::now();
    let single = selftest_hash();
    let elapsed = start.elapsed();
    match single {
        Ok((h, f)) => println!(
            "selftest_run: 1 run in {:?} (failures {}, hash {:016x})",
            elapsed, f, h
        ),
        Err(e) => {
            eprintln!("bench failed: {e}");
            std::process::exit(1);
        }
    }

    // Workload 3: determinism twin-run.
    let start = Instant::now();
    let twin = (selftest_hash(), selftest_hash());
    let elapsed = start.elapsed();
    match twin {
        (Ok((a, _)), Ok((b, _))) => println!(
            "twin_run_determinism: 2 selftest runs in {:?}, hashes_equal={} hash={:016x}",
            elapsed,
            a == b,
            a
        ),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("bench failed: {e}");
            std::process::exit(1);
        }
    }
}
