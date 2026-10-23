#!/usr/bin/env python3
"""Compile one reusable workload against the actual demand source and this standalone crate.

Only library dependencies are built. The baseline archive and consumer are temporary.
Results are CSV with allocator instrumentation enabled equally for both builds.
"""
import argparse
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import sys


def run(command, **kwargs):
    try:
        return subprocess.run(command, check=True, text=True, **kwargs)
    except subprocess.CalledProcessError as error:
        if error.stderr:
            print(error.stderr, file=sys.stderr)
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", default="demand/main")
    parser.add_argument("--iterations", default=10000, type=int)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.iterations <= 0:
        parser.error("--iterations must be positive")
    root = Path(__file__).resolve().parents[1]
    revision = run(["git", "rev-parse", args.baseline], cwd=root, capture_output=True).stdout.strip()
    environment = dict(os.environ, ROLES_BENCH_ITERATIONS=str(args.iterations))
    with tempfile.TemporaryDirectory(prefix="roles-demand-baseline-") as directory:
        baseline = Path(directory)
        archive = subprocess.check_output(["git", "archive", revision, "common", "protocols", "utils"], cwd=root)
        with tarfile.open(fileobj=io.BytesIO(archive)) as source:
            source.extractall(baseline, filter="data")
        consumer = baseline / "consumer"
        (consumer / "src").mkdir(parents=True)
        (consumer / "src" / "main.rs").write_bytes((root / "benches" / "migration.rs").read_bytes())
        manifest = '''[package]
name = "roles-demand-comparison"
version = "0.0.0"
edition = "2021"
[workspace]
[dependencies]
roles_logic_sv2 = { path = "../protocols/v2/roles-logic-sv2" }
binary_sv2 = { path = "../protocols/v2/binary-sv2" }
mining_sv2 = { path = "../protocols/v2/subprotocols/mining" }
template_distribution_sv2 = { path = "../protocols/v2/subprotocols/template-distribution" }
bitcoin = "0.32.6"
[lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(demand_baseline)'] }
'''
        (consumer / "Cargo.toml").write_text(manifest)
        demand_environment = dict(environment, CARGO_TARGET_DIR=str(root / "target" / "performance-demand"),
                                  RUSTFLAGS=environment.get("RUSTFLAGS", "") + " --cfg demand_baseline")
        result = run(["cargo", "run", "--release", "--manifest-path", str(consumer / "Cargo.toml")],
                     env=demand_environment, capture_output=True)
        old = result.stdout
    result = run(["cargo", "bench", "--bench", "migration"], cwd=root, env=environment, capture_output=True)
    native = result.stdout
    if old.splitlines()[0] != native.splitlines()[0]:
        raise RuntimeError("Baseline and native fixture/fanout bytes differ")
    report = f"demand/main source: {revision}\niterations: {args.iterations}\n\nBASELINE\n{old}\nNATIVE\n{native}"
    print(report)
    if args.output:
        args.output.write_text(report)


if __name__ == "__main__":
    main()
