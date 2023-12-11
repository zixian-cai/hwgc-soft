#!/usr/bin/env python3
import logging
import tempfile
import subprocess
import os
from typing import List, Optional
import shutil
from pathlib import Path
import sys
import argparse
import itertools

ROOT = Path(__file__).parent.parent.resolve()
BUILDS = ROOT / "builds"


def cargo_wrapper(rustflags: Optional[str], dst: Optional[str]) -> Optional[str]:
    cmd = ["cargo", "build", "--release"]
    logging.info(
        "{}{}".format(
            'RUSTFLAGS="' + rustflags + '" ' if rustflags else "", " ".join(cmd)
        )
    )
    env = os.environ.copy()
    if rustflags:
        env["RUSTFLAGS"] = rustflags
    p = subprocess.run(cmd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if p.returncode != 0:
        print(p.stdout)
        print(p.stderr)
        sys.exit(p.returncode)
    logging.info("stdout: " + "\n".join(p.stdout.decode("utf-8").splitlines()[-5:]))
    logging.info("stderr: " + "\n".join(p.stderr.decode("utf-8").splitlines()[-5:]))
    if dst:
        shutil.copy("./target/release/hwgc_soft", dst)
        return dst


def build_baseline(bin_name: str) -> str:
    return cargo_wrapper(None, str(BUILDS / bin_name))


def build_profile_generate(pgo_dir: str, bin_name: str) -> str:
    return cargo_wrapper(f"-Cprofile-generate={pgo_dir}", str(BUILDS / bin_name))


def build_pgo(
    pgo_dir: str, profile_gen_bin: str, profile_name: str, inputs: List[List[str]]
):
    pgo = Path(pgo_dir)
    for profraw in pgo.glob("*.profraw"):
        logging.info(f"Removing {profraw}")
        os.remove(profraw)
    logging.info(f"Building profile: {profile_name}")
    for i in inputs:
        cmd = [profile_gen_bin]
        cmd += i
        logging.info(f"\t{cmd}")
        stdout = subprocess.check_output(cmd)
        logging.info(stdout)
    profraws = list(pgo.glob("*.profraw"))
    assert len(profraws) == 1
    profdata = f"{pgo_dir}/{profile_name}.profdata"
    cmd = [
        "/opt/rust/toolchains/nightly-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-profdata",
        "merge",
        "-o",
        profdata,
        profraws[0],
    ]
    logging.info(f"\t{cmd}")
    subprocess.check_output(cmd)
    return cargo_wrapper(f"-Cprofile-use={profdata}", str(BUILDS / profile_name))


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("heapdumps", metavar="heapdumps", type=str, nargs="+")
    return parser.parse_args()


def main():
    logging.basicConfig(
        format="[%(levelname)s] %(asctime)s %(filename)s:%(lineno)d %(message)s",
        level=logging.INFO,
    )
    args = parse_args()
    BUILDS.mkdir(exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="pgo_") as pgo_dir:
        logging.info("Temporary directory: {}".format(pgo_dir))
        baseline_path = build_baseline("baseline")
        logging.info(f"Baseline binary: {baseline_path}")
        profile_gen_path = build_profile_generate(pgo_dir, "profile_generate")
        logging.info(f"Profile gen binary: {profile_gen_path}")

        # Get possible args
        heapdumps: List[str]
        heapdumps = args.heapdumps
        object_models = [
            "openjdk",
            "openjdk-ae",
            "bidirectional",
            "bidirectional-fallback",
        ]
        tracing_loops = ["edge-slot", "edge-objref"]

        ITERATIONS = "25"

        # Individual PGO
        for object_model in object_models:
            for tracing_loop in tracing_loops:
                inputs = [
                    ["-i", ITERATIONS, "-o", object_model, "-t", tracing_loop]
                    + heapdumps
                ]
                profile_name = f"{object_model}_{tracing_loop}".replace("-", "_")
                build_pgo(pgo_dir, profile_gen_path, profile_name, inputs)

        # Two object models
        for tracing_loop in tracing_loops:
            inputs = [
                ["-i", ITERATIONS, "-o", "openjdk", "-t", tracing_loop] + heapdumps,
                ["-i", ITERATIONS, "-o", "openjdk-ae", "-t", tracing_loop] + heapdumps,
            ]
            profile_name = f"openjdk_both_{tracing_loop}".replace("-", "_")
            build_pgo(pgo_dir, profile_gen_path, profile_name, inputs)
        for tracing_loop in tracing_loops:
            inputs = [
                ["-i", ITERATIONS, "-o", "bidirectional", "-t", tracing_loop]
                + heapdumps,
                ["-i", ITERATIONS, "-o", "bidirectional-fallback", "-t", tracing_loop]
                + heapdumps,
            ]
            profile_name = f"bidirectional_both_{tracing_loop}".replace("-", "_")
            build_pgo(pgo_dir, profile_gen_path, profile_name, inputs)

        # Two tracing loops
        for object_model in object_models:
            inputs = [
                ["-i", ITERATIONS, "-o", object_model, "-t", "edge-objref"] + heapdumps,
                ["-i", ITERATIONS, "-o", object_model, "-t", "edge-slot"] + heapdumps,
            ]
            profile_name = f"{object_model}_edge_both".replace("-", "_")
            build_pgo(pgo_dir, profile_gen_path, profile_name, inputs)

        # All in one
        inputs = []
        for object_model in object_models:
            for tracing_loop in tracing_loops:
                inputs.append(
                    ["-i", ITERATIONS, "-o", object_model, "-t", tracing_loop]
                    + heapdumps
                )
        profile_name = f"all_in_one"
        build_pgo(pgo_dir, profile_gen_path, profile_name, inputs)


if __name__ == "__main__":
    main()
