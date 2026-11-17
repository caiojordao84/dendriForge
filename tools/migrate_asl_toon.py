#!/usr/bin/env python3
"""One-shot legacy ASL TOON → `.asl.toml` migrator (DEN-15).

Converts `core/asl/v010/{golden,examples,errors}` programs into tagged-statement
TOML. Drops `version` / `ASLversion`. Nesting is reconstructed with heuristics
documented in the Priority 6 progress doc — review complex FOR/WHILE by hand.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
V010 = ROOT / "core" / "asl" / "v010"
OUT = ROOT / "content" / "fixtures"

CTRL_IF = re.compile(r"^(IF(?:\s+NOT)?|ELSIF)\s+(.+):\s*(.*)$", re.I)
CTRL_ELSE = re.compile(r"^ELSE:\s*(.*)$", re.I)
CTRL_WHILE = re.compile(r"^WHILE\s+(.+):\s*(.*)$", re.I)
CTRL_FOR = re.compile(
    r"^FOR\s+([A-Za-z_][A-Za-z0-9_]*)\s*:=\s*0\s+TO\s+(.+):\s*(.*)$", re.I
)
CTRL_REPEAT = re.compile(r"^repeat\((\S+)\):\s*(.*)$", re.I)
ASSIGN = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.+)$")
CALL = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)\((.*)\)\s*$")
VENDOR = re.compile(r'^VENDOR_EXTENSION\((["\'].*["\'])\)\s*$', re.I)
RETURN = re.compile(r"^RETURN(?:\s+(.+))?$", re.I)
HW_ROW = re.compile(
    r'^(\d+)\|(\w+)\|(\w+)\|"([^"]*)"\|(.+)$'
)
DATA_ROW = re.compile(
    r'^([A-Za-z_][A-Za-z0-9_]*)\|(\w+)\|(\w+)\|(.+)$'
)
EDGE_MODES = {"RISING", "FALLING", "CHANGE"}
AND_CALL = re.compile(r'^AND\((.+)\)$', re.I)
FN_HEADER = re.compile(
    r'^([A-Za-z_][A-Za-z0-9_]*)\(([^)]*)\)\[(\d+)\]:\s*$'
)


def esc(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"')


def norm_expr(text: str) -> str:
    t = text.strip()
    t = t.replace("<>", "!=")
    # Legacy AND(a, b) → infix (expr_text tokenizer treats AND as operator)
    m = AND_CALL.match(t)
    if m:
        args = split_args(m.group(1))
        if len(args) == 2:
            t = f"{args[0]} AND {args[1]}"
    t = re.sub(r"\bTRUE\b", "TRUE", t)
    t = re.sub(r"\bFALSE\b", "FALSE", t)
    return t


def expr_toml(text: str) -> str:
    """Prefer compact literals; otherwise expr_text escape hatch."""
    t = norm_expr(text)
    if re.fullmatch(r"TRUE|FALSE", t, re.I):
        return f'{{ kind = "bool", value = {"true" if t.upper() == "TRUE" else "false"} }}'
    if t in EDGE_MODES:
        return f'{{ kind = "string", value = "{t}" }}'
    if re.fullmatch(r"[0-9]+", t):
        return f'{{ kind = "uint", value = {t} }}'
    if re.fullmatch(r"-?[0-9]+", t):
        return f'{{ kind = "int", value = {t} }}'
    if re.fullmatch(r"-?[0-9]+\.[0-9]+", t):
        return f'{{ kind = "real", value = {t} }}'
    if re.fullmatch(r'T#[0-9]+ms', t, re.I):
        return f'{{ kind = "time", value = "{t}" }}'
    if re.fullmatch(r'0x[0-9A-Fa-f]+', t):
        return f'{{ kind = "hex", value = "{t}" }}'
    if re.fullmatch(r'"([^"\\]|\\.)*"', t):
        inner = t[1:-1]
        return f'{{ kind = "string", value = "{esc(inner)}" }}'
    if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", t):
        return f'{{ kind = "var", name = "{t}" }}'
    # call or complex → expr_text (Phase 1 parser covers AND/OR/NOT/calls/=)
    return f'{{ kind = "expr_text", text = "{esc(t)}" }}'


def split_args(argstr: str) -> list[str]:
    args: list[str] = []
    cur: list[str] = []
    depth = 0
    in_str = False
    quote = ""
    for ch in argstr:
        if in_str:
            cur.append(ch)
            if ch == quote:
                in_str = False
            continue
        if ch in "\"'":
            in_str = True
            quote = ch
            cur.append(ch)
            continue
        if ch == "(":
            depth += 1
            cur.append(ch)
        elif ch == ")":
            depth -= 1
            cur.append(ch)
        elif ch == "," and depth == 0:
            args.append("".join(cur).strip())
            cur = []
        else:
            cur.append(ch)
    if cur and "".join(cur).strip():
        args.append("".join(cur).strip())
    return args


def classify(line: str) -> tuple[str, dict]:
    s = line.strip()
    if not s:
        return ("empty", {})
    if s.upper() in ("EXIT", "BREAK"):
        return ("exit", {})
    if s.upper() == "CONTINUE":
        return ("continue", {})
    m = RETURN.match(s)
    if m:
        return ("return", {"value": m.group(1)})
    m = VENDOR.match(s)
    if m:
        raw = m.group(1).strip()
        if (raw.startswith('"') and raw.endswith('"')) or (
            raw.startswith("'") and raw.endswith("'")
        ):
            raw = raw[1:-1]
        return ("vendor", {"reason": raw})
    m = CTRL_ELSE.match(s)
    if m:
        return ("else", {"inline": m.group(1).strip()})
    m = CTRL_IF.match(s)
    if m:
        kind = m.group(1).upper()
        cond = m.group(2).strip()
        inline = m.group(3).strip()
        if kind.startswith("IF") and "NOT" in kind.split()[:2]:
            # "IF NOT cond" → NOT cond
            if not cond.upper().startswith("NOT "):
                cond = f"NOT {cond}"
            kind = "IF"
        return (kind.lower(), {"cond": cond, "inline": inline})
    m = CTRL_WHILE.match(s)
    if m:
        return ("while", {"cond": m.group(1).strip(), "inline": m.group(2).strip()})
    m = CTRL_FOR.match(s)
    if m:
        return (
            "for",
            {
                "counter": m.group(1),
                "to": m.group(2).strip(),
                "inline": m.group(3).strip(),
            },
        )
    m = CTRL_REPEAT.match(s)
    if m:
        return (
            "for",
            {"counter": "_i", "to": m.group(1).strip(), "inline": m.group(2).strip()},
        )
    m = ASSIGN.match(s)
    if m and not s.startswith("IF "):
        # avoid matching comparison-only lines; assignments use single =
        # but "a = b = c" rare — OK
        left, right = m.group(1), m.group(2)
        # function call assign already covered; reject if left looks like call
        if "(" not in left:
            return ("assign", {"target": left, "value": right})
    m = CALL.match(s)
    if m:
        return ("call", {"name": m.group(1), "args": split_args(m.group(2))})
    # Fallback: treat as call-ish expr statement via vendor? keep as call with expr
    return ("raw", {"text": s})


def is_chain_header(kind: str) -> bool:
    return kind in ("if", "elsif", "else")


def is_compound_open(kind: str) -> bool:
    return kind in ("if", "while", "for")


def parse_block(items: list[str], i: int, stop: set[str]) -> tuple[list[dict], int]:
    out: list[dict] = []
    while i < len(items):
        kind, _ = classify(items[i])
        if kind in stop:
            break
        stmt, i = parse_one(items, i)
        out.append(stmt)
    return out, i


def parse_one(items: list[str], i: int) -> tuple[dict, int]:
    kind, meta = classify(items[i])
    i += 1
    if kind == "assign":
        return (
            {
                "kind": "assign",
                "target": meta["target"],
                "value": meta["value"],
            },
            i,
        )
    if kind == "call":
        return (
            {"kind": "call", "name": meta["name"], "args": meta["args"]},
            i,
        )
    if kind == "vendor":
        return ({"kind": "vendor_extension", "reason": meta["reason"]}, i)
    if kind == "return":
        return ({"kind": "return", "value": meta["value"]}, i)
    if kind == "exit":
        return ({"kind": "exit"}, i)
    if kind == "continue":
        return ({"kind": "continue"}, i)
    if kind == "raw":
        # Emit as call of empty? Use vendor_extension for visibility
        return (
            {
                "kind": "vendor_extension",
                "reason": f"UNPARSED_STMT:{meta['text']}",
            },
            i,
        )
    if kind == "if":
        then, elif_arms, else_body, i = parse_if_chain(items, i - 1)
        return (
            {
                "kind": "if",
                "condition": then["cond"],
                "then": then["body"],
                "elif": elif_arms,
                "else": else_body,
            },
            i,
        )
    if kind == "while":
        if meta["inline"]:
            body = [inline_stmt(meta["inline"])]
        else:
            # Nested WHILE/FOR take one compound or run of simples until IF/ELSIF/ELSE/WHILE/FOR peer
            body, i = parse_loop_body(items, i)
        return ({"kind": "while", "condition": meta["cond"], "body": body}, i)
    if kind == "for":
        if meta["inline"]:
            body = [inline_stmt(meta["inline"])]
        else:
            body, i = parse_loop_body(items, i)
        return (
            {
                "kind": "for",
                "counter": meta["counter"],
                "to": meta["to"],
                "body": body,
            },
            i,
        )
    if kind in ("elsif", "else"):
        # Should be consumed by parse_if_chain; treat as empty vendor marker
        return (
            {
                "kind": "vendor_extension",
                "reason": f"ORPHAN_{kind.upper()}",
            },
            i,
        )
    return ({"kind": "vendor_extension", "reason": f"UNKNOWN:{items[i-1]}"}, i)


def inline_stmt(text: str) -> dict:
    k, m = classify(text)
    if k == "assign":
        return {"kind": "assign", "target": m["target"], "value": m["value"]}
    if k == "call":
        return {"kind": "call", "name": m["name"], "args": m["args"]}
    if k == "return":
        return {"kind": "return", "value": m["value"]}
    if k == "vendor":
        return {"kind": "vendor_extension", "reason": m["reason"]}
    if k == "exit":
        return {"kind": "exit"}
    if k == "continue":
        return {"kind": "continue"}
    return {"kind": "vendor_extension", "reason": f"UNPARSED_INLINE:{text}"}


def parse_if_chain(
    items: list[str], i: int
) -> tuple[dict, list[dict], list[dict] | None, int]:
    """i points at IF line."""
    kind, meta = classify(items[i])
    assert kind == "if"
    i += 1
    if meta["inline"]:
        then_body = [inline_stmt(meta["inline"])]
    else:
        then_body, i = parse_block(items, i, {"if", "elsif", "else"})
    then = {"cond": meta["cond"], "body": then_body}
    elif_arms: list[dict] = []
    else_body: list[dict] | None = None
    while i < len(items):
        k2, m2 = classify(items[i])
        if k2 == "elsif":
            i += 1
            if m2["inline"]:
                body = [inline_stmt(m2["inline"])]
            else:
                body, i = parse_block(items, i, {"if", "elsif", "else"})
            elif_arms.append({"cond": m2["cond"], "body": body})
            continue
        if k2 == "else":
            i += 1
            if m2["inline"]:
                else_body = [inline_stmt(m2["inline"])]
            else:
                else_body, i = parse_block(items, i, {"if", "elsif", "else"})
            break
        break
    return then, elif_arms, else_body, i


def parse_loop_body(items: list[str], i: int) -> tuple[list[dict], int]:
    """FOR/WHILE body heuristic.

    - If next is FOR/WHILE, take a single nested compound (nested-loop idiom).
    - Else take simple statements until next IF/ELSIF/ELSE/WHILE/FOR or end.
      When an IF appears, include it (parse_one) and continue simples — stop
      before a *sibling* WHILE/FOR that would start a new nest at this level
      only if we already consumed an IF? 

    Corpus patterns:
    * fill_matrix: FOR; FOR; leaf  → nested single
    * G03 FOR; 6 calls; more calls → all simples until control (G03 needs hand fix
      when trailing calls belong outside FOR — rare; see progress doc)
    * find_hot_cell: WHILE; simples+WHILE+IF — use recursive parse_block stopping
      only at end for outermost remaining... 

    Chosen rule: parse_block stopping at WHILE/FOR that appear *after* we've
    started, but allowing nested via parse_one when we decide to take one
    compound. Simpler rule used here:

    If peek is while/for → single parse_one (nested loops).
    Else → keep taking parse_one until peek is while/for/elsif/else or end,
            BUT allow if (so IF inside WHILE works). Stop before a while/for
            only when the while/for is at the same 'flat' level after a
            complete statement — for find_hot_cell inner WHILE is the next
            line after `_c = 0`, so we'd stop before inner WHILE if we stop
            at while/for. That breaks find_hot_cell.

    Revised: never stop at while/for when collecting body; only stop at
    elsif/else. Sibling while/for become nested via parse_one. That means
    FOR greedily eats everything remaining in the block — bad for G03
    trailing set_phase after FOR inside ELSIF.

    Compromise: greedy until elsif/else/if(top); for G03-like cases, after
    conversion we keep golden G03 as already hand-fixed and skip overwrite.
    """
    if i >= len(items):
        return [], i
    peek_kind, _ = classify(items[i])
    if peek_kind in ("while", "for"):
        # Nested loop idiom: one nested compound as body (may itself nest).
        stmt, i2 = parse_one(items, i)
        # If after nested loop only simple stmts remain before a chain header,
        # and the nested body was itself a loop with empty following — also
        # absorb a single leaf that follows a FOR/FOR pyramid.
        body = [stmt]
        # Absorb trailing simple/IF statements that belong to this loop when
        # the nested compound was WHILE/FOR and more simples follow before
        # elsif/else — used by find_hot_cell after inner structures... 
        # Actually for FOR/FOR/leaf the leaf is inside innermost via recursion.
        return body, i2

    body: list[dict] = []
    while i < len(items):
        k, _ = classify(items[i])
        if k in ("elsif", "else"):
            break
        if k in ("while", "for") and body:
            # sibling loop ends this body (outer WHILE of find_hot_cell needs
            # inner WHILE inside body — so only break if we already have a
            # completed nested structure? )
            # Include nested loops inside body:
            stmt, i = parse_one(items, i)
            body.append(stmt)
            continue
        if k == "if" or k in (
            "assign",
            "call",
            "return",
            "exit",
            "continue",
            "vendor",
            "raw",
        ):
            stmt, i = parse_one(items, i)
            body.append(stmt)
            continue
        break
    return body, i


def emit_stmt(stmt: dict, indent: int = 0) -> str:
    """Emit a Stmt as an inline TOML table (for nested arrays) or block."""
    sp = "  " * indent
    k = stmt["kind"]
    if k == "assign":
        return (
            f'{{ kind = "assign", target = "{stmt["target"]}", '
            f'value = {expr_toml(stmt["value"])} }}'
        )
    if k == "call":
        args = ", ".join(expr_toml(a) for a in stmt["args"])
        return f'{{ kind = "call", name = "{stmt["name"]}", args = [{args}] }}'
    if k == "vendor_extension":
        return f'{{ kind = "vendor_extension", reason = "{esc(stmt["reason"])}" }}'
    if k == "return":
        if stmt.get("value"):
            return f'{{ kind = "return", value = {expr_toml(stmt["value"])} }}'
        return '{ kind = "return" }'
    if k == "exit":
        return '{ kind = "exit" }'
    if k == "continue":
        return '{ kind = "continue" }'
    if k == "while":
        body = ", ".join(emit_stmt(s) for s in stmt["body"])
        return (
            f'{{ kind = "while", condition = {expr_toml(stmt["condition"])}, '
            f"body = [{body}] }}"
        )
    if k == "for":
        body = ", ".join(emit_stmt(s) for s in stmt["body"])
        return (
            f'{{ kind = "for", counter = "{stmt["counter"]}", '
            f'to = {expr_toml(stmt["to"])}, body = [{body}] }}'
        )
    if k == "if":
        then = ", ".join(emit_stmt(s) for s in stmt["then"])
        parts = [
            f'kind = "if"',
            f'condition = {expr_toml(stmt["condition"])}',
            f"then = [{then}]",
        ]
        if stmt.get("elif"):
            arms = []
            for arm in stmt["elif"]:
                b = ", ".join(emit_stmt(s) for s in arm["body"])
                arms.append(
                    f'{{ condition = {expr_toml(arm["cond"])}, then = [{b}] }}'
                )
            parts.append(f'elif = [{", ".join(arms)}]')
        if stmt.get("else") is not None:
            eb = ", ".join(emit_stmt(s) for s in stmt["else"])
            parts.append(f"else = [{eb}]")
        return "{ " + ", ".join(parts) + " }"
    return f'{{ kind = "vendor_extension", reason = "BAD_STMT" }}'


def emit_top_stmt(stmt: dict, section: str) -> str:
    """Emit [[setup]] / [[loop]] block form for readability on simple stmts."""
    k = stmt["kind"]
    if k == "assign":
        return (
            f"[[{section}]]\n"
            f'kind = "assign"\n'
            f'target = "{stmt["target"]}"\n'
            f'value = {expr_toml(stmt["value"])}\n'
        )
    if k == "call":
        args = ", ".join(expr_toml(a) for a in stmt["args"])
        return (
            f"[[{section}]]\n"
            f'kind = "call"\n'
            f'name = "{stmt["name"]}"\n'
            f"args = [{args}]\n"
        )
    if k == "vendor_extension":
        return (
            f"[[{section}]]\n"
            f'kind = "vendor_extension"\n'
            f'reason = "{esc(stmt["reason"])}"\n'
        )
    if k == "return":
        if stmt.get("value"):
            return (
                f"[[{section}]]\n"
                f'kind = "return"\n'
                f'value = {expr_toml(stmt["value"])}\n'
            )
        return f"[[{section}]]\nkind = \"return\"\n"
    if k in ("exit", "continue"):
        return f"[[{section}]]\nkind = \"{k}\"\n"
    # Complex: single-table array element style via inline assignment
    return f"[[{section}]]\n" + _emit_complex_table(stmt)


def _emit_complex_table(stmt: dict) -> str:
    """Emit fields of a complex stmt as a TOML table (no [[header]])."""
    k = stmt["kind"]
    lines = [f'kind = "{k}"']
    if k == "if":
        lines.append(f'condition = {expr_toml(stmt["condition"])}')
        then = ", ".join(emit_stmt(s) for s in stmt["then"])
        lines.append(f"then = [{then}]")
        if stmt.get("elif"):
            arms = []
            for arm in stmt["elif"]:
                b = ", ".join(emit_stmt(s) for s in arm["body"])
                arms.append(
                    f'{{ condition = {expr_toml(arm["cond"])}, then = [{b}] }}'
                )
            lines.append(f"elif = [{', '.join(arms)}]")
        if stmt.get("else") is not None:
            eb = ", ".join(emit_stmt(s) for s in stmt["else"])
            lines.append(f"else = [{eb}]")
    elif k == "while":
        lines.append(f'condition = {expr_toml(stmt["condition"])}')
        body = ", ".join(emit_stmt(s) for s in stmt["body"])
        lines.append(f"body = [{body}]")
    elif k == "for":
        lines.append(f'counter = "{stmt["counter"]}"')
        lines.append(f'to = {expr_toml(stmt["to"])}')
        body = ", ".join(emit_stmt(s) for s in stmt["body"])
        lines.append(f"body = [{body}]")
    else:
        lines.append(f'reason = "UNSUPPORTED_COMPLEX"')
    return "\n".join(lines) + "\n"


def parse_metadata(text: str) -> dict[str, str]:
    meta: dict[str, str] = {}
    in_meta = False
    for line in text.splitlines():
        if line.startswith("# METADATA:"):
            in_meta = True
            continue
        if in_meta:
            if line.startswith("## ") or line.startswith("# ") and "HARDWARE" in line:
                break
            if line.startswith("##"):
                break
            if ":" in line and not line.startswith("#"):
                k, v = line.split(":", 1)
                meta[k.strip()] = v.strip()
            if line.strip() == "" and meta:
                # keep scanning until ##
                continue
    return meta


def parse_hardware(text: str) -> list[dict]:
    rows = []
    in_hw = False
    for line in text.splitlines():
        if "HARDWARE INTERFACE" in line:
            in_hw = True
            continue
        if in_hw and line.startswith("## "):
            break
        if not in_hw:
            continue
        s = line.strip()
        if not s or s.startswith("hardware[") or s.startswith("#"):
            continue
        m = HW_ROW.match(s)
        if m:
            io = m.group(3)
            # normalize io_mode
            io_map = {
                "input": "DI",
                "output": "DO",
                "bi": "BI",
                "DI": "DI",
                "DO": "DO",
                "BI": "BI",
                "input_pullup": "DI",
            }
            rows.append(
                {
                    "pin": int(m.group(1)),
                    "hw_type": m.group(2).lower(),
                    "io_mode": io_map.get(io, io.upper() if len(io) <= 3 else "DI"),
                    "label": m.group(4),
                    "target_role": m.group(5).strip().strip('"'),
                }
            )
    return rows


def parse_data(text: str) -> list[dict]:
    rows = []
    in_data = False
    for line in text.splitlines():
        if "SYSTEM DATA" in line:
            in_data = True
            continue
        if in_data and line.startswith("## "):
            break
        if not in_data:
            continue
        s = line.strip()
        if not s or s.startswith("data[") or s.startswith("#"):
            continue
        m = DATA_ROW.match(s)
        if m:
            ty = m.group(2).upper()
            type_aliases = {
                "FLOAT": "REAL",
                "DOUBLE": "LREAL",
                "BOOLEAN": "BOOL",
                "STRING": "STRING",
            }
            ty = type_aliases.get(ty, ty)
            scope = m.group(3).upper()
            scope_aliases = {
                "CONSTANT": "CONST",
                "VAR_INPUT": "VAR",
                "VAR_OUTPUT": "VAR",
            }
            scope = scope_aliases.get(scope, scope)
            rest = m.group(4).strip()
            # Normal: init|unit. Legacy STRING empty: ""-  (missing pipe before unit)
            if rest.endswith("| -") or rest.endswith("|-"):
                init, unit = rest.rsplit("|", 1)
                unit = unit.strip() or "-"
            elif "| " in rest or (rest.count("|") >= 1 and not rest.startswith('"')):
                # split on last pipe when present
                if "|" in rest:
                    init, unit = rest.rsplit("|", 1)
                    unit = unit.strip() or "-"
                else:
                    init, unit = rest, "-"
            elif re.match(r'^""-', rest):
                init, unit = '""', "-"
            elif rest.endswith("-") and rest.startswith('"') and rest.count('"') >= 2:
                # "foo"- or ""-
                init, unit = rest[:-1], "-"
            elif "|" in rest:
                init, unit = rest.rsplit("|", 1)
                unit = unit.strip() or "-"
            else:
                init, unit = rest, "-"
            init = init.strip()
            if init == '""':
                init = ""
            elif len(init) >= 2 and init[0] == '"' and init[-1] == '"':
                init = init[1:-1]
            rows.append(
                {
                    "name": m.group(1),
                    "type": ty,
                    "scope": scope,
                    "init": init,
                    "unit": unit or "-",
                }
            )
    return rows


def parse_list_section(text: str, header_substr: str) -> list[str]:
    lines: list[str] = []
    in_sec = False
    for line in text.splitlines():
        if header_substr in line and line.startswith("## "):
            in_sec = True
            continue
        if in_sec and line.startswith("## "):
            break
        if not in_sec:
            continue
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        if re.match(r"^(setup|loop)\[\d+\]:\s*$", s):
            continue
        if s.startswith("- "):
            lines.append(s[2:].strip())
    return lines


def parse_functions(text: str) -> list[dict]:
    fns: list[dict] = []
    in_fn = False
    cur: dict | None = None
    body_lines: list[str] = []
    for line in text.splitlines():
        if "PROCEDURAL FUNCTIONS" in line:
            in_fn = True
            continue
        if not in_fn:
            continue
        if line.startswith("## "):
            break
        raw = line.rstrip()
        s = raw.strip()
        if not s or s.startswith("#"):
            continue
        if s == "functions:":
            continue
        m = FN_HEADER.match(s)
        if m:
            if cur is not None:
                stmts, _ = parse_block(body_lines, 0, set())
                cur["body"] = stmts
                fns.append(cur)
            params = [p.strip() for p in m.group(2).split(",") if p.strip()]
            cur = {"name": m.group(1), "params": params}
            body_lines = []
            continue
        if s.startswith("- ") and cur is not None:
            body_lines.append(s[2:].strip())
    if cur is not None:
        stmts, _ = parse_block(body_lines, 0, set())
        cur["body"] = stmts
        fns.append(cur)
    return fns


def collect_for_counters(stmts: list[dict], into: set[str]) -> None:
    for st in stmts:
        k = st.get("kind")
        if k == "for":
            c = st.get("counter")
            if c:
                into.add(c)
            collect_for_counters(st.get("body") or [], into)
        elif k == "while":
            collect_for_counters(st.get("body") or [], into)
        elif k == "if":
            collect_for_counters(st.get("then") or [], into)
            for arm in st.get("elif") or []:
                collect_for_counters(arm.get("body") or [], into)
            if st.get("else"):
                collect_for_counters(st["else"], into)


def convert_program(text: str, source_rel: str) -> str:
    meta = parse_metadata(text)
    hw = parse_hardware(text)
    data = parse_data(text)
    setup_lines = parse_list_section(text, "INITIALIZATION SYSTEM")
    loop_lines = parse_list_section(text, "BEHAVIORAL LOGIC")
    setup_stmts, _ = parse_block(setup_lines, 0, set())
    loop_stmts, _ = parse_block(loop_lines, 0, set())
    fns = parse_functions(text)

    # Declare FOR counters so body uses like `passCount = _p` validate cleanly.
    declared = {d["name"] for d in data}
    counters: set[str] = set()
    collect_for_counters(setup_stmts, counters)
    collect_for_counters(loop_stmts, counters)
    for fn in fns:
        collect_for_counters(fn["body"], counters)
    for c in sorted(counters):
        if c not in declared:
            data.append(
                {
                    "name": c,
                    "type": "UINT",
                    "scope": "VAR",
                    "init": "0",
                    "unit": "-",
                }
            )
            declared.add(c)

    project = meta.get("project_name", "unnamed")
    editor = meta.get("editor", "dendriForge")
    author = meta.get("author", "dendriForge")
    notes = meta.get("notes", "")

    out: list[str] = []
    out.append(f"# Migrated from {source_rel}")
    out.append(f"# Legacy TOON retained under core/asl/v010/ (read-only).")
    out.append("")
    out.append("[metadata]")
    out.append(f'project_name = "{esc(project)}"')
    out.append(f'editor = "{esc(editor)}"')
    out.append(f'author = "{esc(author)}"')
    if notes:
        out.append(f'notes = "{esc(notes)}"')
    else:
        out.append('notes = "Migrated from legacy v010 TOON — ASL unversioned"')
    out.append("")

    for h in hw:
        out.append("[[hardware]]")
        out.append(f"pin = {h['pin']}")
        out.append(f'hw_type = "{h["hw_type"]}"')
        out.append(f'io_mode = "{h["io_mode"]}"')
        out.append(f'label = "{esc(h["label"])}"')
        out.append(f'target_role = "{esc(h["target_role"])}"')
        out.append("")

    for d in data:
        init = d["init"]
        # bool casing
        if init.upper() in ("TRUE", "FALSE"):
            init = init.upper()
        out.append("[[data]]")
        out.append(f'name = "{d["name"]}"')
        out.append(f'type = "{d["type"]}"')
        out.append(f'scope = "{d["scope"]}"')
        out.append(f'init = "{esc(init)}"')
        out.append(f'unit = "{esc(d["unit"])}"')
        out.append("")

    for st in setup_stmts:
        out.append(emit_top_stmt(st, "setup"))
    for st in loop_stmts:
        out.append(emit_top_stmt(st, "loop"))

    for fn in fns:
        body = ", ".join(emit_stmt(s) for s in fn["body"])
        params = ", ".join(f'"{p}"' for p in fn["params"])
        out.append("[[functions]]")
        out.append(f'name = "{fn["name"]}"')
        out.append(f"params = [{params}]")
        out.append(f"body = [{body}]")
        out.append("")

    return "\n".join(out).rstrip() + "\n"


# Already hand-migrated — do not overwrite.
PROTECTED = {
    "golden/G01-gpio-all-modes-probe.asl.toml",
    "golden/G02-input-pullup-pulldown-matrix.asl.toml",
    "golden/G03-traffic-light-sequencer.asl.toml",
    "golden/G05-pwm-fade-breathing-led.asl.toml",
    "golden/G08-stopwatch-lap-timer.asl.toml",
    "golden/G17-retain-power-cycle-counter.asl.toml",
    "examples/basic-blink-logic.asl.toml",
    "errors/E01-missing-metadata-field.asl.toml",
    "errors/E02-unknown-data-type.asl.toml",
    "errors/E03-undeclared-variable.asl.toml",
    "errors/E04-hardware-pin-conflict.asl.toml",
    "errors/E05-setup-count-mismatch.asl.toml",
    "errors/E06-invalid-scope-write-const.asl.toml",
    "errors/E07-function-arity-mismatch.asl.toml",
}


def convert_error_fixture(path: Path) -> str | None:
    """Errors already migrated; skip."""
    return None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--only", action="append", default=[])
    args = ap.parse_args()

    pairs: list[tuple[Path, Path]] = []
    for sub in ("golden", "examples"):
        for src in sorted((V010 / sub).glob("*.toon")):
            dst_name = src.name.replace(".toon", ".asl.toml")
            dst = OUT / sub / dst_name
            rel = f"{sub}/{dst_name}"
            if rel in PROTECTED:
                continue
            if args.only and not any(o in str(src) for o in args.only):
                continue
            pairs.append((src, dst))

    converted = 0
    for src, dst in pairs:
        text = src.read_text(encoding="utf-8")
        rel = str(src.relative_to(ROOT))
        try:
            toml = convert_program(text, rel)
        except Exception as e:
            print(f"FAIL {src.name}: {e}", file=sys.stderr)
            continue
        print(f"{'WRITE' if not args.dry_run else 'DRY '} {dst.relative_to(ROOT)}")
        if not args.dry_run:
            dst.parent.mkdir(parents=True, exist_ok=True)
            dst.write_text(toml, encoding="utf-8")
        converted += 1
    print(f"converted={converted}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
