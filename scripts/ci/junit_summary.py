#!/usr/bin/env python3
"""Print failing tests from a nextest JUnit report as Markdown, with a local repro per test.

Usage: junit_summary.py <junit.xml>
Prints nothing when the report is missing or has no failures.
"""

import sys
import xml.etree.ElementTree as ET


def main(argv):
    if len(argv) != 1:
        print(__doc__, file=sys.stderr)
        return 2
    try:
        root = ET.parse(argv[0]).getroot()
    except (OSError, ET.ParseError):
        return 0
    failed = []
    for suite in root.iter("testsuite"):
        crate = suite.get("name", "").split("::")[0]
        for case in suite.iter("testcase"):
            if case.find("failure") is not None or case.find("error") is not None:
                failed.append((crate, case.get("name", "?")))
    if not failed:
        return 0
    print(f"**{len(failed)} failing test(s)**")
    print()
    for crate, name in failed:
        print(f"- `{crate}` `{name}`")
        print(f"  - `cargo nextest run -p {crate} --profile ci -E 'test(={name})'`")
    print()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
