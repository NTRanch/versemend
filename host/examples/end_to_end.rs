//! End-to-end: this crate has no headless sim core yet — the Bevy/Lua host's separable deterministic surface is its embedded 82-check selftest harness. This example runs `run_selftest` once on the fixed embedded game, printing the failure count, the transcript length, and an FNV-1a hash of the transcript. Expected output: one line `failures=0 transcript_bytes=<n> hash=<16 hex digits>`; running it twice prints identical hashes.
use std::process::ExitCode;

/// FNV-1a 64-bit hash: deterministic across runs and platforms.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn main() -> ExitCode {
    match versemend::run_selftest() {
        Ok((transcript, failures)) => {
            println!(
                "failures={} transcript_bytes={} hash={:016x}",
                failures,
                transcript.len(),
                fnv1a(transcript.as_bytes())
            );
            if failures == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("end_to_end failed: {e}");
            ExitCode::FAILURE
        }
    }
}
