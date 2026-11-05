#!/usr/bin/env python3
"""Workspace facts from `cargo metadata`, shared by CI workflows and scripts.

  cargo_meta.py test-matrix       JSON {"include":[{"crate":..,"lib":..}]} for the rust-test matrix
  cargo_meta.py has-lib <crate>   exit 0 when the crate has a library target (doctests apply)
  cargo_meta.py lambda-bins       one binary name per line: fotoko-api bins that build without
                                  extra features (so not fotoko-local)

Crates that have their own CI job are left out of the test matrix.
"""

import json
import subprocess
import sys

# Own CI job (rust-e2e); the matrix would run it a second time.
OWN_JOB = {"fotoko-e2e"}
LAMBDA_PACKAGE = "fotoko-api"


def workspace_packages():
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    meta = json.loads(out)
    members = set(meta["workspace_members"])
    return [p for p in meta["packages"] if p["id"] in members]


def has_lib(pkg):
    return any(
        kind in ("lib", "rlib", "proc-macro") for t in pkg["targets"] for kind in t["kind"]
    )


def main(argv):
    if not argv:
        print(__doc__, file=sys.stderr)
        return 2
    cmd = argv[0]
    pkgs = workspace_packages()
    if cmd == "test-matrix":
        include = [
            {"crate": p["name"], "lib": has_lib(p)}
            for p in sorted(pkgs, key=lambda p: p["name"])
            if p["name"] not in OWN_JOB
        ]
        print(json.dumps({"include": include}, separators=(",", ":")))
        return 0
    if cmd == "has-lib":
        match = [p for p in pkgs if p["name"] == argv[1]]
        if not match:
            print(f"cargo_meta: no workspace crate named {argv[1]}", file=sys.stderr)
            return 2
        return 0 if has_lib(match[0]) else 1
    if cmd == "lambda-bins":
        for p in pkgs:
            if p["name"] != LAMBDA_PACKAGE:
                continue
            for t in p["targets"]:
                if "bin" in t["kind"] and not t.get("required-features"):
                    print(t["name"])
        return 0
    print(f"cargo_meta: unknown command {cmd}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
