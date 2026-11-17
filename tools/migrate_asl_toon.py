#!/usr/bin/env python3
"""DEPRECATED (DEN-20): use the Rust migrator instead.

    cargo run -p dendri-cli -- migrate-asl [--dry-run] [--only STEM]

Implementation: `dendri_ast::toon` / `dendri migrate-asl`.
Hand-protected fixtures are never overwritten. ASL stays unversioned (no 0.0.1).
"""

from __future__ import annotations

import sys


def main() -> int:
    print(
        "DEPRECATED: tools/migrate_asl_toon.py was replaced by the Rust migrator (DEN-20).\n"
        "  cargo run -p dendri-cli -- migrate-asl --dry-run\n"
        "  cargo run -p dendri-cli -- migrate-asl\n"
        "See crates/dendri-ast/src/toon.rs",
        file=sys.stderr,
    )
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
