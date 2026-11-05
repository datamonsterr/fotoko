#!/usr/bin/env python3
"""Crate dependency rules of the Fotoko workspace (see "Architecture rules" in AGENTS.md).

Run through `bash scripts/ci/check-arch.sh` (CI job `arch`). Facts come from `cargo metadata`
and `cargo tree`; nothing is built. Exit 0 when every rule holds, 1 with one line per violation.

Rules:
  1. Workspace edges: a crate may normal-depend only on the workspace crates in ALLOWED.
     Direction is protocol <- core <- api, protocol <- pi-agent, pi-agent -> printer-sim; pi-agent
     never depends on core or api, api never on pi-agent. A new edge needs an edit here.
  2. Nothing depends on fotoko-e2e in any dependency kind (it is a test harness).
  3. fotoko-protocol (wire types, no IO) uses only PROTOCOL_EXTERNAL, and fotoko-core (domain,
     no IO) uses none of CORE_FORBIDDEN as a normal dependency.
  4. The Lambda graph (`fotoko-api --no-default-features`) has no local/sim code: no rumqttc,
     rumqttd or wiremock, `fotoko-core` without feature `testing`; `local` is not a default
     feature; only `fotoko-local` (and its tests) require it.
  5. Deploy paths never turn on `local`, `testing` or `--all-features`.
"""

import json
import re
import subprocess
import sys
from pathlib import Path

# Normal (non-dev, non-build) workspace dependencies each crate may have.
ALLOWED = {
    "fotoko-protocol": set(),
    "fotoko-core": {"fotoko-protocol"},
    "fotoko-api": {"fotoko-core", "fotoko-protocol"},
    "fotoko-printer-sim": set(),
    "fotoko-pi-agent": {"fotoko-printer-sim", "fotoko-protocol"},
    # Test harness: may use everything, nothing may use it.
    "fotoko-e2e": {
        "fotoko-api",
        "fotoko-core",
        "fotoko-pi-agent",
        "fotoko-printer-sim",
        "fotoko-protocol",
    },
}
HARNESS = "fotoko-e2e"
PROTOCOL_EXTERNAL = {"chrono", "serde", "serde_json", "thiserror"}
# Runtimes, clients and cloud SDKs: the domain talks to them only through ports.
CORE_FORBIDDEN = re.compile(
    r"^(aws-|lambda_|axum|tower|hyper|tokio|reqwest|rumqtt|rusqlite|tracing-subscriber|wiremock)"
)
LAMBDA_FORBIDDEN = {"rumqttc", "rumqttd", "wiremock"}
# Files that build or ship production artifacts.
DEPLOY_PATHS = [
    "scripts/deploy",
    "scripts/ci/lambda-build.sh",
    "deploy/pi/build.sh",
    "deploy/pi/deploy.sh",
    "deploy/pi/install.sh",
    ".github/workflows/deploy.yml",
    "infra/cdk/lib",
    "infra/cdk/bin",
]
FEATURE_FLAG = re.compile(
    r"--all-features|(--features|-F)[ =]\S*\b(local|testing)\b|--features[ =]\S*fotoko-core/testing"
)


def run(*cmd):
    done = subprocess.run(cmd, capture_output=True, text=True)
    if done.returncode != 0:
        sys.exit(f"arch: ERROR: {' '.join(cmd)} failed:\n{done.stderr}")
    return done.stdout


def metadata():
    meta = json.loads(run("cargo", "metadata", "--format-version", "1", "--no-deps"))
    members = set(meta["workspace_members"])
    return {p["name"]: p for p in meta["packages"] if p["id"] in members}


def check_edges(pkgs, errors):
    for name, pkg in sorted(pkgs.items()):
        if name not in ALLOWED:
            errors.append(f"{name}: crate is not in scripts/ci/check_arch.py ALLOWED; add its rules")
            continue
        for dep in pkg["dependencies"]:
            d = dep["name"]
            if d not in pkgs:
                continue
            kind = dep["kind"] or "normal"
            if d == HARNESS and name != HARNESS:
                errors.append(f"{name} has a {kind} dependency on {HARNESS}; nothing may depend on the test harness")
            elif kind == "normal" and d not in ALLOWED[name]:
                errors.append(
                    f"{name} -> {d}: normal dependency not allowed "
                    f"(allowed: {sorted(ALLOWED[name]) or 'none'})"
                )


def check_external(pkgs, errors):
    for dep in pkgs["fotoko-protocol"]["dependencies"]:
        if (dep["kind"] is None) and dep["name"] not in PROTOCOL_EXTERNAL and dep["name"] not in pkgs:
            errors.append(f"fotoko-protocol -> {dep['name']}: wire types may only use {sorted(PROTOCOL_EXTERNAL)}")
    for dep in pkgs["fotoko-core"]["dependencies"]:
        if dep["kind"] is None and CORE_FORBIDDEN.match(dep["name"]):
            errors.append(f"fotoko-core -> {dep['name']}: the domain has no IO; put it behind a port in fotoko-api")


def check_lambda_graph(pkgs, errors):
    tree = run(
        "cargo", "tree", "-p", "fotoko-api", "--no-default-features",
        "--edges", "normal", "--prefix", "none", "--format", "{p} [{f}]",
    )
    seen = {}
    for line in tree.splitlines():
        m = re.match(r"(\S+) v\S+.*\[(.*)\]", line)
        if m:
            seen[m.group(1)] = set(m.group(2).split(","))
    for bad in sorted(LAMBDA_FORBIDDEN & set(seen)):
        errors.append(f"fotoko-api --no-default-features pulls in {bad}: local/sim code leaked into the Lambda graph")
    if "testing" in seen.get("fotoko-core", set()):
        errors.append("fotoko-api --no-default-features enables fotoko-core/testing (in-memory adapters)")
    default = set(pkgs["fotoko-api"]["features"].get("default", []))
    if default & {"local", "fotoko-core/testing"}:
        errors.append("fotoko-api enables `local` by default; the deployed build must not need a flag to drop it")
    for target in pkgs["fotoko-api"]["targets"]:
        if "bin" not in target["kind"]:
            continue
        req = target.get("required-features", [])
        if target["name"] == "fotoko-local" and req != ["local"]:
            errors.append(f"fotoko-local must require feature `local`, has {req}")
        if target["name"] != "fotoko-local" and req:
            errors.append(f"Lambda bin {target['name']} requires features {req}; deployed bins take none")


def check_deploy_flags(errors):
    for rel in DEPLOY_PATHS:
        path = Path(rel)
        files = [path] if path.is_file() else sorted(p for p in path.rglob("*") if p.is_file())
        for f in files:
            if f.suffix in {".png", ".pem"} or "node_modules" in f.parts:
                continue
            for n, line in enumerate(f.read_text(errors="replace").splitlines(), 1):
                stripped = line.lstrip()
                if stripped.startswith("#") or stripped.startswith("//"):
                    continue
                if FEATURE_FLAG.search(line):
                    errors.append(f"{f}:{n}: production path turns on local/testing/all features: {line.strip()}")


def main():
    pkgs = metadata()
    errors = []
    check_edges(pkgs, errors)
    check_external(pkgs, errors)
    check_lambda_graph(pkgs, errors)
    check_deploy_flags(errors)
    if errors:
        print("arch: FAILED", file=sys.stderr)
        for e in errors:
            print(f"  - {e}", file=sys.stderr)
        print("Rules: AGENTS.md 'Architecture rules', checks: scripts/ci/check_arch.py", file=sys.stderr)
        return 1
    print(f"arch: ok ({len(pkgs)} crates)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
