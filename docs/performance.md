# Reusable demand/main comparison

Run `python3 scripts/compare-demand.py --iterations 10000 --output /tmp/comparison.txt`. The default baseline is the actual `demand/main` Git ref; `--baseline <ref>` selects another source revision. Compilation uses temporary archived library sources and published external dependencies. Source files and manifests in this repository are left intact. Cargo build artifacts go under `target/`.

`benches/migration.rs` runs the same workloads with the APIs of each revision. Its conditional imports are confined to comparison code. Both builds encode the same 573-byte native-compatible job: a valid coinbase, payout and witness-commitment outputs, sixteen extranonce bytes, and twelve Merkle siblings. Borrowed decoding releases the message immediately; retained decoding owns all fields; encoding clones the stored job before consuming it. Fanout retains 64 standard jobs before dropping them. Fixture and fanout SHA256 checksums must match between builds.

The allocator counts allocations and reallocations, plus requested bytes. Setup and warmup are excluded. Instrumentation is enabled equally in both builds. Timings are a single-machine snapshot, include allocator instrumentation, and vary with compiler, machine load, and dependency resolution; allocation counts are the more stable regression signal. The fanout sample runs 1,000 iterations; other rows run 10,000.

Snapshot on the migration workspace, Rust 1.94.0, optimized builds; baseline `f037e6db3568454cdf7973aa6f457dd41d0856e1`:

| Operation | demand ns/op | Native ns/op | demand allocations/op | Native allocations/op | demand bytes/op | Native bytes/op |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Get size | 5.9 | 2.0 | 0 | 0 | 0 | 0 |
| Decode borrowed | 1460.9 | 887.4 | 36 | 23 | 3548 | 1568 |
| Decode retained | 1854.9 | 1187.5 | 51 | 38 | 4486 | 2410 |
| Encode owned clone | 820.1 | 762.1 | 22 | 22 | 2567 | 2887 |
| Decode and forward ID | 368.0 | 343.1 | 10 | 10 | 512 | 448 |
| Retained fanout, 64 jobs | 200825.1 | 195402.0 | 1345 | 1281 | 104384 | 91584 |

The native checked ID patch alone measured 8.2 ns and **zero allocations / zero allocated bytes**. It updates the receive payload directly and contains no encoding call. The complete decode-and-forward operation keeps the baseline's ten decoder allocations. The native encode row requests more bytes despite equal allocation counts; this is the cost of the target codec's native encoding representation, not an added roles-logic adapter.

The fixture SHA256 is `d307589db001906d5e3aa0daa08635e8fc5abf67151b47c39b17f25e09cd5c12`; the retained fanout bytes hash to `c57e6b181135b1e3d5db1dd1f059e06b61667b05fef36778dd462e347de3fcae` in both builds.

Protocol-required differences are tested separately and excluded from these equivalent-byte benchmarks: common Reconnect, JD PushSolution and wtxids, extended group IDs, complete count-prefixed custom outputs, output sigop budgets, and BIP323 validation. Comparing these with the removed fields of the old protocols would measure different operations. No compat implementation is used as the baseline.
