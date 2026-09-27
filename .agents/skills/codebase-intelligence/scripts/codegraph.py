#!/usr/bin/env python3
"""
codegraph.py — Agent-agnostic codebase graph builder.

Extracts ASTs via tree-sitter, builds a file-dependency graph, computes
PageRank, and caches everything in SQLite with Merkle-tree delta-indexing.

Usage:
    python3 codegraph.py index [--force]
    python3 codegraph.py report [--top N]
    python3 codegraph.py query <symbol> [--refs]
    python3 codegraph.py path <from> <to>
    python3 codegraph.py stats
    python3 codegraph.py export [--format json|graphml]

Requires: Python 3.10+, tree-sitter, tree-sitter-language-pack, networkx
Fallback: regex-based extraction when tree-sitter is unavailable.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import posixpath
import re
import sqlite3
import struct
import sys
import time
from collections import defaultdict
from pathlib import Path
from typing import Any

# ---------------------------------------------------------------------------
# Optional imports with graceful fallback
# ---------------------------------------------------------------------------

try:
    import tree_sitter_language_pack as tslp

    HAS_TREE_SITTER = True
except ImportError:
    HAS_TREE_SITTER = False

try:
    import networkx as nx

    HAS_NX = True
except ImportError:
    HAS_NX = False

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

SCHEMA_VERSION = 2
DB_NAME = "index.sqlite3"
STATS_NAME = "stats.json"
EXPORT_NAME = "export.json"

# Languages that tree-sitter can extract definitions from
EXTRACTABLE_LANGS = {
    ".rs": "rust",
    ".ts": "typescript",
    ".tsx": "typescript",
    ".js": "javascript",
    ".mjs": "javascript",
    ".cjs": "javascript",
    ".py": "python",
    ".go": "go",
    ".java": "java",
    ".c": "c",
    ".cpp": "cpp",
    ".cc": "cpp",
    ".cxx": "cpp",
    ".h": "c",
    ".hpp": "cpp",
    ".rb": "ruby",
    ".php": "php",
    ".swift": "swift",
    ".kt": "kotlin",
    ".kts": "kotlin",
    ".cs": "c_sharp",
    ".scala": "scala",
    ".ex": "elixir",
    ".exs": "elixir",
    ".hs": "haskell",
    ".lua": "lua",
    ".r": "r",
    ".R": "r",
}

# File extensions to include (source + config)
SOURCE_EXTS = set(EXTRACTABLE_LANGS.keys()) | {
    ".json", ".toml", ".yaml", ".yml", ".md", ".css", ".html",
    ".sh", ".bash", ".zsh", ".fish", ".service", ".plist",
    ".lock", ".txt", ".log",
}

# Module-specifier extensions probed during import resolution.
TS_EXTS = (".ts", ".tsx", ".d.ts", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs")
# Index file stems consulted when a module path names a directory.
TS_INDEX_STEMS = ("index",)
JAVA_EXTS = (".java",)

# ---------------------------------------------------------------------------
# Tree-sitter queries per language: (definition_node_types, reference_node_types)
# We define node types that represent definitions and references for each lang.
# ---------------------------------------------------------------------------

# Definition node types: what constitutes a "definition" in each language
DEF_TYPES = {
    "rust": [
        "function_item", "struct_item", "enum_item", "trait_item",
        "impl_item", "type_item", "const_item", "static_item",
        "trait_declaration",
    ],
    "typescript": [
        "function_declaration", "class_declaration", "interface_declaration",
        "type_alias_declaration", "enum_declaration", "lexical_declaration",
        "property_definition", "method_definition", "formal_parameters",
        "arrow_function", "function", "class",
    ],
    "javascript": [
        "function_declaration", "class_declaration",
        "lexical_declaration", "property_definition", "method_definition",
    ],
    "python": [
        "function_definition", "class_definition", "assignment",
    ],
    "go": [
        "function_declaration", "type_declaration", "var_declaration",
        "const_declaration", "method_declaration", "short_var_declaration",
    ],
    "java": [
        "class_declaration", "interface_declaration", "method_declaration",
        "field_declaration", "constructor_declaration", "enum_declaration",
    ],
}

# Node types that contain identifiers (used for reference extraction)
IDENTIFIER_TYPES = {
    "rust": ["identifier", "field_identifier", "type_identifier"],
    "typescript": ["identifier", "type_identifier", "property_identifier"],
    "javascript": ["identifier", "property_identifier"],
    "python": ["identifier", "attribute"],
    "go": ["identifier", "field_identifier", "type_identifier"],
    "java": ["identifier", "field_identifier", "type_identifier"],
}

# Import node types per language. These produce file-level dependency edges,
# not symbol definitions, so they are deliberately absent from DEF_TYPES.
IMPORT_TYPES = {
    "rust": ["use_declaration", "extern_crate_declaration"],
    "typescript": ["import_statement", "export_statement", "import_require_statement"],
    "javascript": ["import_statement", "export_statement", "import_require_statement"],
    "python": ["import_statement", "import_from_statement"],
    "go": ["import_declaration"],
    "java": ["import_declaration"],
}

# Module-declaration node types. A bodiless `mod foo;` contributes a file edge to
# the sibling module file; an inline `mod foo { .. }` (which has a body) does not.
MODULE_TYPES = {
    "rust": ["mod_item"],
}

# ---------------------------------------------------------------------------
# Merkle tree helpers
# ---------------------------------------------------------------------------

def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def merkle_leaf(path: str, content_hash: str) -> str:
    """Hash a leaf node: H(path || content_hash)."""
    return sha256(f"{path}\0{content_hash}".encode())


def merkle_internal(child_hashes: list[str]) -> str:
    """Hash an internal node: H(sorted child hashes joined)."""
    joined = "\0".join(sorted(child_hashes))
    return sha256(joined.encode())


# ---------------------------------------------------------------------------
# SQLite helpers
# ---------------------------------------------------------------------------

def open_db(root: Path) -> sqlite3.Connection:
    db_dir = root / ".code-intelligence"
    db_dir.mkdir(exist_ok=True)
    conn = sqlite3.connect(str(db_dir / DB_NAME))
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA synchronous=NORMAL")
    conn.execute("PRAGMA cache_size=-64000")  # 64MB
    _init_schema(conn)
    return conn


def _init_schema(conn: sqlite3.Connection) -> None:
    conn.executescript("""
        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS files (
            path       TEXT PRIMARY KEY,
            sha256     TEXT NOT NULL,
            lang       TEXT,
            size       INTEGER,
            mtime_ns   INTEGER,
            extractor  TEXT DEFAULT 'tree-sitter',
            indexed_at REAL
        );
        CREATE TABLE IF NOT EXISTS defs (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            file      TEXT NOT NULL,
            name      TEXT NOT NULL,
            kind      TEXT NOT NULL,
            line      INTEGER NOT NULL,
            end_line  INTEGER,
            parent    TEXT,
            signature TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_defs_name ON defs(name);
        CREATE INDEX IF NOT EXISTS idx_defs_file ON defs(file);

        CREATE TABLE IF NOT EXISTS refs (
            id    INTEGER PRIMARY KEY AUTOINCREMENT,
            file  TEXT NOT NULL,
            name  TEXT NOT NULL,
            line  INTEGER NOT NULL,
            kind  TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_refs_name ON refs(name);
        CREATE INDEX IF NOT EXISTS idx_refs_file ON refs(file);

        CREATE TABLE IF NOT EXISTS imports (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            file            TEXT NOT NULL,
            target_raw      TEXT NOT NULL,
            target_resolved TEXT,
            resolution_method TEXT,
            confidence      TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_imports_file ON imports(file);

        CREATE TABLE IF NOT EXISTS edges (
            src_file  TEXT NOT NULL,
            dst_file  TEXT NOT NULL,
            kind      TEXT,
            weight    REAL DEFAULT 1.0,
            resolution_method TEXT,
            confidence TEXT,
            PRIMARY KEY (src_file, dst_file)
        );

        CREATE TABLE IF NOT EXISTS merkle (
            path TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            hash TEXT NOT NULL
        );
    """)
    conn.commit()
    _migrate_schema(conn)


# Columns added after the initial schema. Keep additive so an existing index can
# be upgraded in place instead of forcing a full rebuild.
_ADDITIVE_COLUMNS = {
    "imports": [("resolution_method", "TEXT"), ("confidence", "TEXT")],
    "edges": [("resolution_method", "TEXT"), ("confidence", "TEXT")],
}


def _migrate_schema(conn: sqlite3.Connection) -> None:
    """Add columns introduced by newer schema versions to an existing index."""
    for table, columns in _ADDITIVE_COLUMNS.items():
        existing = {row[1] for row in conn.execute(f"PRAGMA table_info({table})")}
        for name, decl in columns:
            if name not in existing:
                conn.execute(f"ALTER TABLE {table} ADD COLUMN {name} {decl}")
    conn.commit()


# ---------------------------------------------------------------------------
# File enumeration
# ---------------------------------------------------------------------------

def git_tracked_files(root: Path) -> list[str]:
    """Get all git-tracked files, repo-relative."""
    import subprocess
    raw = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=root, capture_output=True
    ).stdout
    return sorted(raw.decode(errors="replace").split("\0"))


def detect_lang(path: str) -> str | None:
    ext = Path(path).suffix
    return EXTRACTABLE_LANGS.get(ext)


def should_include(path: str) -> bool:
    ext = Path(path).suffix
    if ext in SOURCE_EXTS:
        return True
    basename = Path(path).name
    if basename in ("Dockerfile", "Makefile", "Cargo.toml", "package.json"):
        return True
    return False


# ---------------------------------------------------------------------------
# Tree-sitter extraction
# ---------------------------------------------------------------------------

_parser_cache: dict[str, Any] = {}


def _get_parser(lang: str) -> Any:
    if lang in _parser_cache:
        return _parser_cache[lang]
    try:
        parser = tslp.get_parser(lang)
        _parser_cache[lang] = parser
        return parser
    except Exception:
        return None


def _walk_nodes(node, depth: int = 0):
    """Yield (node, depth) for all nodes in the tree."""
    yield node, depth
    for child in node.children:
        yield from _walk_nodes(child, depth + 1)


def _node_text(node, source: bytes) -> str:
    return source[node.start_byte:node.end_byte].decode(errors="replace")


def _extract_name(node, source: bytes) -> str | None:
    """Extract the name from a definition node."""
    for child in node.children:
        if child.type in ("identifier", "type_identifier", "field_identifier",
                          "property_identifier", "name"):
            return _node_text(child, source)
        if child.type == "declarator":
            # Rust: fn name(...) or let name = ...
            return _extract_name(child, source)
        if child.type == "pattern":
            # Python: def name(...)
            return _extract_name(child, source)
        if child.type == "left_hand_side":
            # JS/TS: const name = ...
            return _extract_name(child, source)
    # Fallback: first identifier child
    for child in node.children:
        if child.type.endswith("_identifier"):
            return _node_text(child, source)
    return None


def _extract_signature(node, source: bytes) -> str:
    """Extract a minimal signature from a definition node."""
    text = _node_text(node, source)
    # For functions/methods: extract up to the opening brace or body
    # This gives us "fn foo(bar: Baz) -> Qux" without the body
    body_markers = ["{", "=>", ":"]
    for marker in body_markers:
        idx = text.find(marker)
        if idx > 0:
            sig = text[:idx].strip()
            if len(sig) > 200:
                sig = sig[:197] + "..."
            return sig
    if len(text) > 200:
        return text[:197] + "..."
    return text.strip()


def _strip_quotes(text: str) -> str:
    """Remove one layer of matching string quotes."""
    text = text.strip()
    if len(text) >= 2 and text[0] == text[-1] and text[0] in "\"'`":
        return text[1:-1]
    return text


def _split_top_level_commas(text: str) -> list[str]:
    """Split on commas that are not nested inside braces."""
    parts: list[str] = []
    depth = 0
    current = ""
    for ch in text:
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
        if ch == "," and depth == 0:
            parts.append(current)
            current = ""
        else:
            current += ch
    parts.append(current)
    return [p.strip() for p in parts if p.strip()]


def _expand_rust_use(target: str) -> list[str]:
    """Expand a Rust `use` argument into individual module paths.

    `super::{a, b::c}` yields `super::a` and `super::b::c`. A trailing `::*` or
    `::self` is stripped so each result names a module rather than an item.
    """
    target = target.strip().rstrip(";").strip()
    if not target:
        return []

    brace = target.find("{")
    if brace != -1:
        end = target.rfind("}")
        if end > brace:
            prefix = target[:brace]
            inner = target[brace + 1:end]
            out: list[str] = []
            for part in _split_top_level_commas(inner):
                out.extend(_expand_rust_use(prefix + part))
            return out

    for suffix in ("::*", "::self"):
        if target.endswith(suffix):
            target = target[: -len(suffix)]
            break
    return [target] if target else []


def _import_specs(node, source: bytes, lang: str) -> list[tuple[str, str]]:
    """Extract (module_specifier, kind) pairs from an import or module node.

    Returns the module specifier only — never the whole statement — so the
    resolver receives something it can actually map to a file.
    """
    if lang == "rust":
        if node.type == "mod_item":
            if node.child_by_field_name("body") is not None:
                return []  # inline module: no separate file to depend on
            name = node.child_by_field_name("name")
            return [(_node_text(name, source), "mod")] if name is not None else []
        if node.type == "extern_crate_declaration":
            name = node.child_by_field_name("name")
            return [(_node_text(name, source), "extern")] if name is not None else []
        argument = node.child_by_field_name("argument")
        if argument is None:
            return []
        if argument.type == "use_as_clause":
            argument = argument.child_by_field_name("path") or argument
        return [(p, "use") for p in _expand_rust_use(_node_text(argument, source))]

    if lang in ("typescript", "javascript"):
        src = node.child_by_field_name("source")
        if src is None:
            return []  # e.g. `export const x = 1` — not a dependency
        kind = "export-from" if node.type == "export_statement" else "import"
        return [(_strip_quotes(_node_text(src, source)), kind)]

    if lang == "python":
        if node.type == "import_statement":
            return [
                (_node_text(child, source), "import")
                for child in node.children
                if child.type == "dotted_name"
            ]
        module = node.child_by_field_name("module_name")
        if module is None:
            return []
        base = _node_text(module, source)
        specs = [(base, "import")]
        if base.startswith("."):
            # `from . import x` names a sibling module, not the package itself.
            imported = node.child_by_field_name("name")
            if imported is not None and imported.type == "dotted_name":
                first = _node_text(imported, source).split(".")[0]
                specs.append((base + first, "import"))
        return specs

    if lang == "go":
        return [
            (_strip_quotes(_node_text(n, source)), "import")
            for n, _ in _walk_nodes(node)
            if n.type == "interpreted_string_literal"
        ]

    if lang == "java":
        for n, _ in _walk_nodes(node):
            if n.type == "scoped_identifier":
                return [(_node_text(n, source), "import")]
        return []

    return []


def extract_tree_sitter(
    path: str, source: bytes, lang: str
) -> tuple[list[dict], list[dict], list[dict]]:
    """Extract defs, refs, and imports using tree-sitter."""
    parser = _get_parser(lang)
    if parser is None:
        return [], [], []

    try:
        tree = parser.parse(source)
    except Exception:
        return [], [], []

    defs = []
    refs = []
    imports = []

    def_types = set(DEF_TYPES.get(lang, []))
    ident_types = set(IDENTIFIER_TYPES.get(lang, []))
    import_types = set(IMPORT_TYPES.get(lang, []))
    module_types = set(MODULE_TYPES.get(lang, []))

    # First pass: collect all definition names for reference matching
    all_def_names: set[str] = set()

    for node, _ in _walk_nodes(tree.root_node):
        if node.type in def_types:
            name = _extract_name(node, source)
            if name and name not in ("_", "self", "super", "crate", "mod"):
                line = node.start_point[0] + 1
                end_line = node.end_point[0] + 1
                kind = node.type
                parent = None

                # For methods, find the enclosing class/impl
                p = node.parent
                while p:
                    if p.type in ("impl_item", "class_declaration",
                                  "class", "trait_item", "trait_declaration",
                                  "struct_item", "object"):
                        parent = _extract_name(p, source)
                        break
                    p = p.parent

                sig = _extract_signature(node, source)

                defs.append({
                    "file": path,
                    "name": name,
                    "kind": kind,
                    "line": line,
                    "end_line": end_line,
                    "parent": parent,
                    "signature": sig,
                })
                all_def_names.add(name)

        elif node.type in import_types or node.type in module_types:
            # Only the module specifier is recorded, never the raw statement.
            for target, kind in _import_specs(node, source, lang):
                if not target:
                    continue
                imports.append({
                    "file": path,
                    "target_raw": target,
                    "kind": kind,
                    "target_resolved": None,  # resolved later
                })

        elif node.type in ident_types:
            name = _node_text(node, source)
            if name and len(name) > 1 and name not in ("_", "self", "super", "crate", "mod", "pub", "fn", "let", "mut", "const", "struct", "enum", "impl", "trait", "type", "use", "mod", "return", "if", "else", "for", "while", "loop", "match", "true", "false", "as", "in", "where", "async", "await", "move"):
                refs.append({
                    "file": path,
                    "name": name,
                    "line": node.start_point[0] + 1,
                    "kind": node.type,
                })

    return defs, refs, imports


# ---------------------------------------------------------------------------
# Regex fallback extraction (for when tree-sitter is unavailable)
# ---------------------------------------------------------------------------

_RUST_DEF = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?"
    r"(?:extern\s+\"[^\"]*\"\s+)?(?:fn|struct|enum|trait|type|static|const|mod)\s+"
    r"([A-Za-z_]\w*)",
    re.MULTILINE,
)

_TS_DEF = re.compile(
    r"^\s*(?:export\s+)?(?:default\s+)?(?:declare\s+)?(?:async\s+)?"
    r"(?:function|class|interface|type|enum|const)\s+([A-Za-z_$][\w$]*)",
    re.MULTILINE,
)

_PY_DEF = re.compile(
    r"^\s*(?:def|class)\s+([A-Za-z_]\w*)",
    re.MULTILINE,
)

_GO_DEF = re.compile(
    r"^\s*(?:func|type|var|const)\s+(?:\([^)]*\)\s+)?([A-Za-z_]\w*)",
    re.MULTILINE,
)

_RUST_IMPORT = re.compile(r"^\s*(?:pub\s+)?use\s+([^;]+);", re.MULTILINE)
_RUST_MOD = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_]\w*)\s*;", re.MULTILINE
)
_TS_IMPORT = re.compile(
    r"""^\s*(?:import|export)\b[^;\n'"]*?\bfrom\s*['"]([^'"]+)['"]""",
    re.MULTILINE,
)
_TS_IMPORT_BARE = re.compile(r"""^\s*import\s*['"]([^'"]+)['"]""", re.MULTILINE)
_PY_IMPORT = re.compile(r"^\s*(?:from\s+(\S+)\s+import|import\s+(\S+))", re.MULTILINE)


def _collapse_import_statements(text: str) -> str:
    """Join multi-line `import` / `export ... from` statements onto one line.

    A statement is joined until its braces/brackets/parens balance, a `;` is
    seen, or the file ends, so the statement-local regexes above can match
    imports whose specifier sits on a later line.
    """
    lines = text.split("\n")
    out: list[str] = []
    i = 0
    while i < len(lines):
        if re.match(r"\s*(?:import|export)\b", lines[i]):
            pieces = [lines[i]]
            depth = 0
            while i < len(lines):
                depth += lines[i].count("{") + lines[i].count("[") + lines[i].count("(")
                depth -= lines[i].count("}") + lines[i].count("]") + lines[i].count(")")
                if depth <= 0 or ";" in lines[i] or i + 1 >= len(lines):
                    break
                i += 1
                pieces.append(lines[i])
            out.append(" ".join(p.strip() for p in pieces))
        else:
            out.append(lines[i])
        i += 1
    return "\n".join(out)


def extract_regex(
    path: str, source: bytes, lang: str
) -> tuple[list[dict], list[dict], list[dict]]:
    """Fallback regex extraction."""
    text = source.decode(errors="replace")
    defs = []
    refs = []
    imports = []

    if lang == "rust":
        for m in _RUST_DEF.finditer(text):
            line = text[: m.start()].count("\n") + 1
            defs.append({
                "file": path, "name": m.group(1), "kind": m.group(0).split()[-2],
                "line": line, "end_line": None, "parent": None, "signature": "",
            })
        for m in _RUST_IMPORT.finditer(text):
            for target in _expand_rust_use(m.group(1)):
                imports.append({"file": path, "target_raw": target, "kind": "use",
                                "target_resolved": None})
        for m in _RUST_MOD.finditer(text):
            imports.append({"file": path, "target_raw": m.group(1), "kind": "mod",
                            "target_resolved": None})

    elif lang in ("typescript", "javascript"):
        for m in _TS_DEF.finditer(text):
            line = text[: m.start()].count("\n") + 1
            defs.append({
                "file": path, "name": m.group(1), "kind": m.group(0).split()[-2],
                "line": line, "end_line": None, "parent": None, "signature": "",
            })
        joined = _collapse_import_statements(text)
        for pattern in (_TS_IMPORT, _TS_IMPORT_BARE):
            for m in pattern.finditer(joined):
                imports.append({"file": path, "target_raw": m.group(1), "kind": "import",
                                "target_resolved": None})

    elif lang == "python":
        for m in _PY_DEF.finditer(text):
            line = text[: m.start()].count("\n") + 1
            defs.append({
                "file": path, "name": m.group(1), "kind": "def" if "def" in m.group(0) else "class",
                "line": line, "end_line": None, "parent": None, "signature": "",
            })
        for m in _PY_IMPORT.finditer(text):
            target = m.group(1) or m.group(2)
            imports.append({"file": path, "target_raw": target, "kind": "import",
                            "target_resolved": None})

    elif lang == "go":
        for m in _GO_DEF.finditer(text):
            line = text[: m.start()].count("\n") + 1
            defs.append({
                "file": path, "name": m.group(1), "kind": m.group(0).split()[0],
                "line": line, "end_line": None, "parent": None, "signature": "",
            })

    return defs, refs, imports


# ---------------------------------------------------------------------------
# Import resolution
# ---------------------------------------------------------------------------

# Confidence tiers (the skill's precision ladder):
#   A = compiler/LSP/SCIP-backed, B = deterministic AST + language rules,
#   C = framework-aware heuristic, D = lexical/text heuristic.
CONF_RULES = "B"
CONF_HEURISTIC = "C"


def _load_jsonc(path: Path) -> dict:
    """Parse a JSON file, tolerating full-line `//` comments and trailing commas."""
    try:
        raw = path.read_text(errors="replace")
    except OSError:
        return {}
    raw = re.sub(r"(?m)^\s*//.*$", "", raw)
    raw = re.sub(r",(\s*[}\]])", r"\1", raw)
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError:
        return {}
    return parsed if isinstance(parsed, dict) else {}


def _cargo_package_name(text: str) -> str | None:
    """Return the `[package] name` declared in a Cargo manifest."""
    section = re.search(r"(?ms)^\[package\](.*?)(?=^\[|\Z)", text)
    match = re.search(r'(?m)^\s*name\s*=\s*"([^"]+)"', section.group(1) if section else text)
    return match.group(1) if match else None


def _cargo_crate_dirs(
    all_files: set[str], root: Path | None = None
) -> dict[str, str]:
    """Map Rust package names to the module root of the crate they declare.

    A dependency on another crate is written by package name in code
    (`agentcowork_guard::ticket::…`), never as a path, so the name has to be read
    back from each tracked `Cargo.toml`. Declared names use `-` while code uses
    `_`, so the key is normalised. Member manifests of a virtual workspace root
    are skipped: only a real `[package]` declares a crate.
    """
    base_path = Path(root) if root is not None else Path(".")
    crates: dict[str, str] = {}
    for path in sorted(all_files):
        if posixpath.basename(path) != "Cargo.toml":
            continue
        try:
            text = (base_path / path).read_text(errors="replace")
        except OSError:
            continue
        if not re.search(r"(?m)^\[package\]", text):
            continue
        name = _cargo_package_name(text)
        if not name:
            continue
        manifest_dir = posixpath.dirname(path)
        module_root = posixpath.join(manifest_dir, "src")
        if not any(f.startswith(module_root + "/") for f in all_files):
            module_root = manifest_dir
        crates.setdefault(name.replace("-", "_"), module_root)
    return crates


def _rust_module_dir(source_file: str) -> str:
    """Directory that a Rust `mod` / `self` / `super` path resolves against."""
    parent = posixpath.dirname(source_file)
    stem = posixpath.basename(source_file)[:-3] if source_file.endswith(".rs") else ""
    if stem in ("lib", "main", "mod"):
        return parent
    return posixpath.join(parent, stem)


def _find_crate_root(source_file: str, all_files: set[str]) -> str | None:
    """Directory that `crate::` paths in `source_file` resolve against.

    Walks up to the nearest tracked `Cargo.toml` and returns the package's
    module root: `<manifest_dir>/src` when that directory holds the crate root
    (`lib.rs` / `main.rs`), otherwise the manifest directory itself. Returns
    None when no manifest is tracked, so `crate::` paths stay unresolved rather
    than being guessed.
    """
    directory = posixpath.dirname(source_file)
    while True:
        manifest = posixpath.join(directory, "Cargo.toml") if directory else "Cargo.toml"
        if manifest in all_files:
            for crate_root in ("lib.rs", "main.rs"):
                if posixpath.join(directory, "src", crate_root) in all_files:
                    return posixpath.join(directory, "src")
            return directory
        if not directory:
            return None
        directory = posixpath.dirname(directory)


def _resolve_module_path(
    base_dir: str, parts: list[str], all_files: set[str]
) -> str | None:
    """Probe `<base_dir>/<parts>` from full length down to one segment.

    `use crate::guard::tickets` names a module, while `use crate::guard::Ticket`
    names an item inside one, and the two are indistinguishable from the path
    alone. Truncating a trailing segment at a time resolves both without
    inventing an edge: a path that matches no file returns None instead of
    collapsing to the crate root.
    """
    real = [p for p in parts if p]
    if not real or (len(real) == 1 and real[0][:1].isupper()):
        # `crate::*` names the module root, and `<Crate>::<Item>` names an item
        # the module root re-exports. Both land on the root file, and only if
        # that file actually exists.
        for crate_root in ("lib.rs", "main.rs"):
            candidate = posixpath.join(base_dir, crate_root)
            if candidate in all_files:
                return candidate
        return None

    for end in range(len(real), 0, -1):
        base = posixpath.join(base_dir, *real[:end])
        for candidate in (
            base + ".rs",
            posixpath.join(base, "mod.rs"),
            posixpath.join(base, "lib.rs"),
        ):
            if candidate in all_files:
                return candidate
    return None


def _resolve_rust_module(
    parts: list[str], source_file: str, all_files: set[str]
) -> str | None:
    """Resolve a `crate::`-relative Rust module path to its declaring file."""
    crate_root = _find_crate_root(source_file, all_files)
    if crate_root is None:
        return None
    return _resolve_module_path(crate_root, parts, all_files)


def _probe_module(base: str, all_files: set[str]) -> str | None:
    """Probe `base` for a module file, trying extensions then index stems."""
    for ext in ("",) + TS_EXTS + (".py", ".rs", ".java"):
        if base + ext in all_files:
            return base + ext
    for stem in TS_INDEX_STEMS:
        for ext in TS_EXTS:
            candidate = posixpath.join(base, stem + ext)
            if candidate in all_files:
                return candidate
    return None


def _ts_path_aliases(
    all_files: set[str], root: Path | None = None
) -> dict[str, tuple[str, str]]:
    """Map declared TypeScript path-alias patterns to repo-relative prefixes.

    Reads `compilerOptions.baseUrl` + `paths` from every tracked
    `tsconfig*.json`, so `@/lib/tauri` in `ui/` resolves to `ui/src/lib/tauri`
    because `ui/tsconfig.json` declares it, not because the name looks like a
    path. Patterns without a wildcard are skipped: a bare alias is ambiguous
    with the package specifiers this resolver deliberately leaves unresolved.

    Each value is `(config_dir, prefix)`, so an alias only applies to importers
    under the config that declares it — two packages may both define `@/*`.
    """
    base_path = Path(root) if root is not None else Path(".")
    aliases: dict[str, tuple[str, str]] = {}
    for path in sorted(all_files):
        if not path.endswith(".json"):
            continue
        if not posixpath.basename(path).startswith("tsconfig"):
            continue
        options = _load_jsonc(base_path / path).get("compilerOptions") or {}
        base_url = options.get("baseUrl") or "."
        base_dir = posixpath.join(posixpath.dirname(path), base_url)
        for pattern, targets in (options.get("paths") or {}).items():
            if "*" not in pattern or not isinstance(targets, list) or not targets:
                continue
            target = targets[0]
            if not isinstance(target, str):
                continue
            # Only one wildcard is substituted, so a target with several
            # placeholders cannot be expanded faithfully — skip it.
            if target.count("*") != 1:
                continue
            aliases.setdefault(pattern, (
                posixpath.dirname(path),
                posixpath.normpath(
                    posixpath.join(base_dir, target.replace("*", ""))
                ).rstrip("/"),
            ))
    return aliases


def _workspace_package_map(
    all_files: set[str], root: Path | None = None
) -> dict[str, str]:
    """Map TS workspace package names to their source directories.

    Reads `name` from every tracked ``packages/*/package.json`` and maps it to
    the package's ``src/`` directory, so ``@personal-ai/core-domain`` resolves to
    ``packages/core-domain/src/``. Packages whose ``src/`` dir doesn't exist are
    skipped — they are stubs or purely published packages with no in-tree source.
    """
    base_path = Path(root) if root is not None else Path(".")
    pkgs: dict[str, str] = {}
    for path in sorted(all_files):
        parts = path.split("/")
        if len(parts) != 3 or parts[0] != "packages" or parts[2] != "package.json":
            continue
        try:
            config = json.loads((base_path / path).read_text(errors="replace"))
        except (OSError, json.JSONDecodeError):
            continue
        name = config.get("name")
        if not isinstance(name, str) or not name.startswith("@"):
            continue
        pkg_dir = posixpath.dirname(path)
        src_dir = posixpath.join(pkg_dir, "src")
        if any(f.startswith(src_dir + "/") for f in all_files):
            pkgs[name] = src_dir
    return pkgs


def resolve_import(
    target_raw: str,
    source_file: str,
    all_files: set[str],
    kind: str | None = None,
    aliases: dict[str, tuple[str, str]] | None = None,
    crates: dict[str, str] | None = None,
    ws_pkgs: dict[str, str] | None = None,
) -> str | None:
    """Resolve a raw import target to a repo-relative file path.

    `kind` is the declaration kind recorded by the extractor (`use`, `mod`,
    `extern`, `import`, `export-from`). It disambiguates targets whose specifier
    alone is not enough, such as a bodiless `mod foo;`.

    `aliases`, `crates`, and `ws_pkgs` are computed once per index run and
    passed in to keep them off the hot path; when omitted they are derived from
    `all_files`.
    """
    source_dir = posixpath.dirname(source_file)

    # Strip wildcard suffix: use crate::foo::* → crate::foo
    clean = target_raw.rstrip("*").rstrip()
    if not clean:
        return None

    # Rust: crate::foo::bar → resolve within crate's src/
    if clean.startswith("crate::"):
        parts = clean.replace("crate::", "").split("::")
        return _resolve_rust_module(parts, source_file, all_files)

    # Rust: super::foo → resolve against the parent module's directory
    if clean.startswith("super::"):
        super_count = 0
        rest = clean
        while rest.startswith("super::"):
            super_count += 1
            rest = rest[len("super::"):]
        base = _rust_module_dir(source_file)
        for _ in range(super_count):
            base = posixpath.dirname(base)
        return _resolve_module_path(base, rest.split("::"), all_files)

    # Rust: self::foo → resolve against this module's own directory
    if clean.startswith("self::"):
        return _resolve_module_path(
            _rust_module_dir(source_file), clean[len("self::"):].split("::"), all_files
        )

    # Rust workspace crates are addressed by package name, never by path.
    if crates is None:
        crates = _cargo_crate_dirs(all_files)
    head, _, remainder = clean.partition("::")
    crate_root = crates.get(head)
    if crate_root is not None:
        return _resolve_module_path(crate_root, remainder.split("::"), all_files)

    # Rust sibling modules: a bodiless `mod foo;` (and a bare `use foo;`) names a
    # file beside the declaring module. An `extern crate` names a dependency,
    # which has no repo file to point at.
    if kind == "extern":
        return None
    if kind == "mod" or (
        source_file.endswith(".rs") and "::" not in clean and not clean.startswith(".")
    ):
        base = posixpath.join(_rust_module_dir(source_file), clean)
        for candidate in (base + ".rs", posixpath.join(base, "mod.rs")):
            if candidate in all_files:
                return candidate
        return None

    # Rust: bare path like AgentTemplate::X or messages
    # (not crate:: or super:: prefixed — could be a use alias or local path)
    if "::" in clean and not clean.startswith("."):
        parts = clean.split("::")
        # If last part looks like a type (starts with uppercase), try the module name before it
        if parts[-1][0:1].isupper() and len(parts) > 1:
            parts = parts[:-1]
        return _resolve_rust_module(parts, source_file, all_files)

    # Python/JS/TS relative imports: ./foo or ../foo
    if clean.startswith("."):
        # Count leading dots for parent traversal
        dot_count = 0
        rest = clean
        while rest.startswith("."):
            dot_count += 1
            rest = rest[1:]
        base = source_dir
        for _ in range(dot_count - 1):  # . = 1 level, .. = 2 levels
            base = posixpath.dirname(base)
        parts = [p for p in rest.strip("/").split("/") if p]
        resolved = posixpath.join(base, *parts) if parts else base
        return _probe_module(resolved, all_files)

    # Workspace TS packages (`@personal-ai/core-domain` → packages/core-domain/src/)
    if ws_pkgs is None:
        ws_pkgs = _workspace_package_map(all_files)
    for pkg_name, pkg_src in ws_pkgs.items():
        if clean == pkg_name or clean.startswith(pkg_name + "/"):
            subpath = clean[len(pkg_name):].lstrip("/")
            if subpath:
                return _probe_module(posixpath.join(pkg_src, subpath), all_files)
            return _probe_module(posixpath.join(pkg_src, "index"), all_files)

    # Declared TypeScript path aliases (`@/*` → `src/*`) take precedence over the
    # package-style probe, which would otherwise treat `@/lib/x` as a package.
    if aliases is None:
        aliases = _ts_path_aliases(all_files)
    for pattern, (config_dir, prefix) in aliases.items():
        if config_dir and not source_file.startswith(config_dir + "/"):
            continue
        head, _, tail = pattern.partition("*")
        if not clean.startswith(head) or not clean.endswith(tail):
            continue
        middle = clean[len(head): len(clean) - len(tail)] if tail else clean[len(head):]
        resolved = _probe_module(posixpath.join(prefix, middle), all_files)
        if resolved is not None:
            return resolved

    # Absolute imports (package-style, TS/JS/Python)
    joined = "/".join(clean.split("/"))
    probed = _probe_module(joined, all_files)
    if probed is not None:
        return probed
    for ext in ("/mod.rs", "/__init__.py"):
        if joined + ext in all_files:
            return joined + ext

    return None


# ---------------------------------------------------------------------------
# Graph construction + PageRank
# ---------------------------------------------------------------------------

def build_graph(conn: sqlite3.Connection) -> dict[str, float]:
    """Build file dependency graph and compute PageRank. Returns {file: rank}."""
    # Clear old edges
    conn.execute("DELETE FROM edges")

    # Build edges from imports
    all_files = {r[0] for r in conn.execute("SELECT path FROM files")}
    file_set = all_files

    for row in conn.execute("SELECT file, target_resolved FROM imports WHERE target_resolved IS NOT NULL"):
        src, dst = row
        # A file cannot depend on itself; a resolved self-reference means the
        # specifier matched a same-named local module while actually naming an
        # external crate, so it is dropped rather than recorded as an edge.
        if dst == src:
            continue
        if dst in file_set:
            conn.execute(
                "INSERT OR REPLACE INTO edges (src_file, dst_file, kind, weight) VALUES (?, ?, 'import', 1.0)",
                (src, dst),
            )

    # Also add intra-module edges based on shared parent (encourage locality)
    for row in conn.execute("SELECT path FROM files"):
        path = row[0]
        parts = Path(path).parts
        if len(parts) > 1:
            parent_module = str(Path(path).parent)
            # Connect sibling files lightly
            # (This is done via a lighter weight; the main signal is imports)

    conn.commit()

    # Build networkx graph
    if HAS_NX:
        G = nx.DiGraph()
        for row in conn.execute("SELECT path FROM files"):
            G.add_node(row[0])
        for row in conn.execute("SELECT src_file, dst_file, weight FROM edges"):
            G.add_edge(row[0], row[1], weight=row[2])

        if len(G.nodes) > 0:
            try:
                ranks = nx.pagerank(G, alpha=0.85, max_iter=100)
            except Exception:
                ranks = {n: 1.0 / len(G.nodes) for n in G.nodes}
        else:
            ranks = {}
    else:
        # Pure Python PageRank (power iteration)
        ranks = _pagerank_python(conn)

    return ranks


def _pagerank_python(conn: sqlite3.Connection, damping: float = 0.85,
                     iterations: int = 50, tol: float = 1e-6) -> dict[str, float]:
    """Pure Python PageRank — no networkx needed."""
    files = [r[0] for r in conn.execute("SELECT path FROM files")]
    n = len(files)
    if n == 0:
        return {}

    file_idx = {f: i for i, f in enumerate(files)}
    rank = [1.0 / n] * n

    # Build adjacency: for each file, which files it imports
    out_edges: dict[int, list[int]] = defaultdict(list)
    in_edges: dict[int, list[int]] = defaultdict(list)
    for row in conn.execute("SELECT src_file, dst_file FROM edges"):
        if row[0] in file_idx and row[1] in file_idx:
            s, d = file_idx[row[0]], file_idx[row[1]]
            out_edges[s].append(d)
            in_edges[d].append(s)

    for _ in range(iterations):
        new_rank = [(1 - damping) / n] * n
        for i in range(n):
            for src in in_edges.get(i, []):
                out_count = len(out_edges.get(src, []))
                if out_count > 0:
                    new_rank[i] += damping * rank[src] / out_count
        # Dangling nodes: distribute their rank equally
        dangling = [i for i in range(n) if not out_edges.get(i)]
        dangling_sum = sum(rank[i] for i in dangling) * damping / n
        for i in range(n):
            new_rank[i] += dangling_sum
        # Convergence check
        diff = sum(abs(new_rank[i] - rank[i]) for i in range(n))
        rank = new_rank
        if diff < tol:
            break

    return {files[i]: rank[i] for i in range(n)}


# ---------------------------------------------------------------------------
# Index command
# ---------------------------------------------------------------------------

def cmd_index(root: Path, force: bool = False) -> None:
    t0 = time.time()
    conn = open_db(root)
    extractor = "tree-sitter" if HAS_TREE_SITTER else "regex"

    # Enumerate tracked files
    all_files = git_tracked_files(root)
    included = [f for f in all_files if should_include(f)]
    all_files_set = set(included)
    aliases = _ts_path_aliases(all_files_set, root)
    crates = _cargo_crate_dirs(all_files_set, root)
    ws_pkgs = _workspace_package_map(all_files_set, root)

    # Load cached Merkle tree
    cached_merkle: dict[str, str] = {}
    for row in conn.execute("SELECT path, hash FROM merkle"):
        cached_merkle[row[0]] = row[0]  # just path for now

    # Phase 1: stat + hash changed files
    files_to_parse: list[str] = []
    files_unchanged = 0
    files_new = 0

    for path in included:
        full = root / path
        try:
            stat = full.stat()
        except OSError:
            continue
        mtime_ns = int(stat.st_mtime_ns)
        size = stat.st_size

        if not force:
            row = conn.execute(
                "SELECT sha256, mtime_ns, size FROM files WHERE path = ?", (path,)
            ).fetchone()
            if row and row[1] == mtime_ns and row[2] == size:
                files_unchanged += 1
                continue

        # File changed or new — read and hash
        try:
            content = full.read_bytes()
        except OSError:
            continue
        file_hash = sha256(content)
        files_to_parse.append(path)
        files_new += 1

    # Phase 2: parse changed files
    defs_inserted = 0
    refs_inserted = 0
    imports_inserted = 0

    for path in files_to_parse:
        full = root / path
        lang = detect_lang(path)
        try:
            content = full.read_bytes()
        except OSError:
            continue
        stat = full.stat()

        # Extract
        if lang and HAS_TREE_SITTER:
            defs, refs, imps = extract_tree_sitter(path, content, lang)
        elif lang:
            defs, refs, imps = extract_regex(path, content, lang)
        else:
            defs, refs, imps = [], [], []

        # Delete old data for this file
        conn.execute("DELETE FROM defs WHERE file = ?", (path,))
        conn.execute("DELETE FROM refs WHERE file = ?", (path,))
        conn.execute("DELETE FROM imports WHERE file = ?", (path,))
        conn.execute("DELETE FROM edges WHERE src_file = ? OR dst_file = ?", (path, path))

        # Insert new
        for d in defs:
            conn.execute(
                "INSERT INTO defs (file, name, kind, line, end_line, parent, signature) "
                "VALUES (?, ?, ?, ?, ?, ?, ?)",
                (d["file"], d["name"], d["kind"], d["line"],
                 d["end_line"], d["parent"], d.get("signature", "")),
            )
            defs_inserted += 1

        for r in refs:
            conn.execute(
                "INSERT INTO refs (file, name, line, kind) VALUES (?, ?, ?, ?)",
                (r["file"], r["name"], r["line"], r["kind"]),
            )
            refs_inserted += 1

        for imp in imps:
            resolved = resolve_import(
                imp["target_raw"], path, all_files_set, imp.get("kind"), aliases, crates,
                ws_pkgs,
            )
            conn.execute(
                "INSERT INTO imports (file, target_raw, target_resolved) VALUES (?, ?, ?)",
                (imp["file"], imp["target_raw"], resolved),
            )
            imports_inserted += 1

        # Update file record
        file_hash = sha256(content)
        conn.execute(
            "INSERT OR REPLACE INTO files (path, sha256, lang, size, mtime_ns, extractor, indexed_at) "
            "VALUES (?, ?, ?, ?, ?, ?, ?)",
            (path, file_hash, lang, stat.st_size, int(stat.st_mtime_ns),
             extractor, time.time()),
        )

    # Phase 3: remove files no longer tracked
    tracked_set = set(included)
    for row in conn.execute("SELECT path FROM files"):
        if row[0] not in tracked_set:
            conn.execute("DELETE FROM files WHERE path = ?", (row[0],))
            conn.execute("DELETE FROM defs WHERE file = ?", (row[0],))
            conn.execute("DELETE FROM refs WHERE file = ?", (row[0],))
            conn.execute("DELETE FROM imports WHERE file = ?", (row[0],))
            conn.execute("DELETE FROM edges WHERE src_file = ? OR dst_file = ?", (row[0], row[0]))

    # Phase 4: build graph + PageRank
    t_graph = time.time()
    ranks = build_graph(conn)
    graph_time = time.time() - t_graph

    # Phase 5: update Merkle tree
    merkle_leaves = {}
    for row in conn.execute("SELECT path, sha256 FROM files"):
        merkle_leaves[row[0]] = merkle_leaf(row[0], row[1])

    # Build directory Merkle
    dir_children: dict[str, list[str]] = defaultdict(list)
    for path in sorted(merkle_leaves.keys()):
        parts = Path(path).parts
        if len(parts) > 1:
            parent = str(Path(path).parent)
            dir_children[parent].append(merkle_leaves[path])
        # Root-level files contribute to root hash

    root_hash = merkle_internal(sorted(merkle_leaves.values())) if merkle_leaves else ""

    conn.execute("DELETE FROM merkle")
    for path, h in merkle_leaves.items():
        conn.execute("INSERT INTO merkle (path, kind, hash) VALUES (?, 'leaf', ?)", (path, h))
    for dir_path, children in dir_children.items():
        conn.execute("INSERT INTO merkle (path, kind, hash) VALUES (?, 'dir', ?)",
                     (dir_path, merkle_internal(sorted(children))))
    conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('last_root_hash', ?)",
                 (root_hash,))
    conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('last_index_time', ?)",
                 (str(time.time()),))

    conn.commit()
    total_time = time.time() - t0

    # Write stats
    db_dir = root / ".code-intelligence"
    db_dir.mkdir(exist_ok=True)
    total_defs = conn.execute("SELECT COUNT(*) FROM defs").fetchone()[0]
    total_refs = conn.execute("SELECT COUNT(*) FROM refs").fetchone()[0]
    total_imports = conn.execute("SELECT COUNT(*) FROM imports").fetchone()[0]
    total_edges = conn.execute("SELECT COUNT(*) FROM edges").fetchone()[0]
    total_files = conn.execute("SELECT COUNT(*) FROM files").fetchone()[0]

    stats = {
        "total_files": total_files,
        "total_defs": total_defs,
        "total_refs": total_refs,
        "total_imports": total_imports,
        "total_edges": total_edges,
        "files_parsed": len(files_to_parse),
        "files_unchanged": files_unchanged,
        "files_new": files_new,
        "defs_inserted": defs_inserted,
        "refs_inserted": refs_inserted,
        "imports_inserted": imports_inserted,
        "extractor": extractor,
        "tree_sitter_available": HAS_TREE_SITTER,
        "networkx_available": HAS_NX,
        "graph_time_s": round(graph_time, 3),
        "total_time_s": round(total_time, 3),
        "merkle_root": root_hash[:16] + "..." if root_hash else "",
        "schema_version": SCHEMA_VERSION,
    }
    (db_dir / STATS_NAME).write_text(json.dumps(stats, indent=2))

    conn.close()

    print(f"Index complete in {total_time:.1f}s")
    print(f"  Files: {total_files} tracked, {len(files_to_parse)} parsed, {files_unchanged} unchanged")
    print(f"  Extracted: {total_defs} defs, {total_refs} refs, {total_imports} imports")
    print(f"  Graph: {total_edges} edges, extractor={extractor}")
    print(f"  Merkle root: {stats['merkle_root']}")


# ---------------------------------------------------------------------------
# Report command
# ---------------------------------------------------------------------------

def cmd_report(root: Path, top_n: int = 20) -> None:
    conn = open_db(root)

    total_files = conn.execute("SELECT COUNT(*) FROM files").fetchone()[0]
    total_defs = conn.execute("SELECT COUNT(*) FROM defs").fetchone()[0]
    total_refs = conn.execute("SELECT COUNT(*) FROM refs").fetchone()[0]

    print(f"\n=== Codebase Graph Report ===")
    print(f"Files: {total_files} | Definitions: {total_defs} | References: {total_refs}\n")

    # PageRank
    print("--- PageRank Top Files (most important/central) ---")
    G = nx.DiGraph() if HAS_NX else None
    if G:
        for row in conn.execute("SELECT path FROM files"):
            G.add_node(row[0])
        for row in conn.execute("SELECT src_file, dst_file FROM edges"):
            G.add_edge(row[0], row[1])
        ranks = nx.pagerank(G, alpha=0.85) if len(G.nodes) > 0 else {}
    else:
        ranks = _pagerank_python(conn)

    sorted_ranks = sorted(ranks.items(), key=lambda x: -x[1])[:top_n]
    for i, (path, rank) in enumerate(sorted_ranks, 1):
        defs_count = conn.execute("SELECT COUNT(*) FROM defs WHERE file = ?", (path,)).fetchone()[0]
        refs_count = conn.execute("SELECT COUNT(*) FROM refs WHERE file = ?", (path,)).fetchone()[0]
        in_edges = conn.execute("SELECT COUNT(*) FROM edges WHERE dst_file = ?", (path,)).fetchone()[0]
        out_edges = conn.execute("SELECT COUNT(*) FROM edges WHERE src_file = ?", (path,)).fetchone()[0]
        print(f"  {i:3d}. [{rank:.4f}] {path} ({defs_count} defs, {refs_count} refs, "
              f"in={in_edges} out={out_edges})")

    # Cycles (SCCs with >1 node)
    if G and len(G.nodes) > 0:
        sccs = list(nx.strongly_connected_components(G))
        cycles = [s for s in sccs if len(s) > 1]
        if cycles:
            print(f"\n--- Dependency Cycles ({len(cycles)} found) ---")
            for i, scc in enumerate(sorted(cycles, key=len, reverse=True)[:5], 1):
                print(f"  Cycle {i}: {len(scc)} files")
                for f in sorted(scc)[:5]:
                    print(f"    - {f}")
                if len(scc) > 5:
                    print(f"    ... and {len(scc) - 5} more")

    # Orphans (no in or out edges)
    orphan_files = []
    for row in conn.execute("SELECT path FROM files"):
        p = row[0]
        in_e = conn.execute("SELECT COUNT(*) FROM edges WHERE dst_file = ?", (p,)).fetchone()[0]
        out_e = conn.execute("SELECT COUNT(*) FROM edges WHERE src_file = ?", (p,)).fetchone()[0]
        if in_e == 0 and out_e == 0:
            orphan_files.append(p)

    if orphan_files:
        print(f"\n--- Orphan Files ({len(orphan_files)} with no edges) ---")
        for f in orphan_files[:15]:
            print(f"  - {f}")
        if len(orphan_files) > 15:
            print(f"  ... and {len(orphan_files) - 15} more")

    # Most referenced definitions
    print(f"\n--- Most Referenced Definitions (top 15) ---")
    rows = conn.execute(
        "SELECT name, COUNT(*) as cnt FROM refs GROUP BY name ORDER BY cnt DESC LIMIT 15"
    ).fetchall()
    for name, cnt in rows:
        defs = conn.execute(
            "SELECT file, kind, line FROM defs WHERE name = ? LIMIT 3", (name,)
        ).fetchall()
        def_str = ", ".join(f"{f}:{l}" for f, k, l in defs) if defs else "no def found"
        print(f"  {cnt:4d}x  {name}  ({def_str})")

    # Language breakdown
    print(f"\n--- Language Breakdown ---")
    rows = conn.execute(
        "SELECT lang, COUNT(*) as cnt FROM files WHERE lang IS NOT NULL GROUP BY lang ORDER BY cnt DESC"
    ).fetchall()
    for lang, cnt in rows:
        print(f"  {lang:15s} {cnt:4d} files")

    conn.close()


# ---------------------------------------------------------------------------
# Query command
# ---------------------------------------------------------------------------

def cmd_query(root: Path, symbol: str, refs_only: bool = False) -> None:
    conn = open_db(root)

    if not refs_only:
        print(f"\n=== Definitions of '{symbol}' ===")
        rows = conn.execute(
            "SELECT file, kind, line, parent, signature FROM defs WHERE name = ? ORDER BY file",
            (symbol,),
        ).fetchall()
        if not rows:
            print("  (none found)")
        for file, kind, line, parent, sig in rows:
            parent_str = f" in {parent}" if parent else ""
            sig_str = f"\n    {sig}" if sig else ""
            print(f"  {file}:{line} — {kind}{parent_str}{sig_str}")

    print(f"\n=== References to '{symbol}' ===")
    rows = conn.execute(
        "SELECT file, line, kind FROM refs WHERE name = ? ORDER BY file, line",
        (symbol,),
    ).fetchall()
    if not rows:
        print("  (none found)")
    # Group by file
    by_file: dict[str, list] = defaultdict(list)
    for file, line, kind in rows:
        by_file[file].append((line, kind))
    for file in sorted(by_file):
        lines = by_file[file]
        line_strs = [str(l) for l, _ in lines]
        print(f"  {file}: lines {', '.join(line_strs)} ({len(lines)} refs)")

    conn.close()


# ---------------------------------------------------------------------------
# Path command
# ---------------------------------------------------------------------------

def cmd_path(root: Path, from_file: str, to_file: str) -> None:
    conn = open_db(root)

    if not HAS_NX:
        print("Error: networkx is required for path commands.")
        print("Install with: pip install networkx")
        conn.close()
        return

    G = nx.DiGraph()
    for row in conn.execute("SELECT path FROM files"):
        G.add_node(row[0])
    for row in conn.execute("SELECT src_file, dst_file FROM edges"):
        G.add_edge(row[0], row[1])

    try:
        path = nx.shortest_path(G, from_file, to_file)
        print(f"\n=== Shortest path ({len(path)} hops) ===")
        for i, f in enumerate(path):
            prefix = "  " if i < len(path) - 1 else "  "
            arrow = "  →" if i < len(path) - 1 else ""
            print(f"{prefix}{f}{arrow}")
    except nx.NetworkXNoPath:
        print(f"\nNo path from {from_file} to {to_file}")
    except nx.NodeNotFound as e:
        print(f"\n{e}")

    conn.close()


# ---------------------------------------------------------------------------
# Stats command
# ---------------------------------------------------------------------------

def cmd_stats(root: Path) -> None:
    db_dir = root / ".code-intelligence"
    stats_path = db_dir / STATS_NAME

    if not stats_path.exists():
        print("No index found. Run: codegraph.py index")
        return

    stats = json.loads(stats_path.read_text())
    print(f"\n=== Codegraph Stats ===")
    for k, v in stats.items():
        print(f"  {k:25s} {v}")


# ---------------------------------------------------------------------------
# Export command
# ---------------------------------------------------------------------------

def cmd_export(root: Path, fmt: str = "json") -> None:
    conn = open_db(root)
    db_dir = root / ".code-intelligence"
    db_dir.mkdir(exist_ok=True)

    if fmt == "json":
        data = {
            "files": [],
            "defs": [],
            "refs": [],
            "edges": [],
        }
        for row in conn.execute("SELECT * FROM files"):
            data["files"].append({
                "path": row[0], "sha256": row[1], "lang": row[2],
                "size": row[3], "mtime_ns": row[4], "extractor": row[5],
            })
        for row in conn.execute("SELECT * FROM defs"):
            data["defs"].append({
                "id": row[0], "file": row[1], "name": row[2],
                "kind": row[3], "line": row[4], "end_line": row[5],
                "parent": row[6], "signature": row[7],
            })
        for row in conn.execute("SELECT * FROM refs"):
            data["refs"].append({
                "id": row[0], "file": row[1], "name": row[2],
                "line": row[3], "kind": row[4],
            })
        for row in conn.execute("SELECT * FROM edges"):
            data["edges"].append({
                "src": row[0], "dst": row[1], "kind": row[2], "weight": row[3],
            })

        out_path = db_dir / EXPORT_NAME
        out_path.write_text(json.dumps(data, indent=1))
        print(f"Exported to {out_path} ({len(data['files'])} files, "
              f"{len(data['defs'])} defs, {len(data['edges'])} edges)")

    elif fmt == "graphml":
        if not HAS_NX:
            print("Error: networkx is required for GraphML export.")
            conn.close()
            return
        G = nx.DiGraph()
        for row in conn.execute("SELECT * FROM files"):
            # `lang` is NULL for files with no extractable language (markdown,
            # JSON, manifests). GraphML has no null value, so it is written as
            # an empty string rather than dropping the node.
            G.add_node(row[0], lang=row[2] or "", size=row[3] or 0)
        for row in conn.execute("SELECT * FROM edges"):
            G.add_edge(row[0], row[1], kind=row[2], weight=row[3])
        out_path = db_dir / "export.graphml"
        nx.write_graphml(G, str(out_path))
        print(f"Exported to {out_path}")

    conn.close()


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="codegraph — Agent-agnostic codebase graph builder"
    )
    parser.add_argument("--root", type=Path, default=Path("."),
                        help="Project root (default: cwd)")
    sub = parser.add_subparsers(dest="command")

    p_index = sub.add_parser("index", help="Build/update the code graph (delta-aware)")
    p_index.add_argument("--force", action="store_true",
                         help="Force full re-index (ignore cache)")

    p_report = sub.add_parser("report", help="Show PageRank, cycles, orphans, stats")
    p_report.add_argument("--top", type=int, default=20,
                          help="Number of top files to show")

    p_query = sub.add_parser("query", help="Find definitions and references for a symbol")
    p_query.add_argument("symbol", help="Symbol name to search for")
    p_query.add_argument("--refs", action="store_true",
                         help="Show only references (skip definitions)")

    p_path = sub.add_parser("path", help="Find shortest dependency path between files")
    p_path.add_argument("from_file", help="Source file path")
    p_path.add_argument("to_file", help="Target file path")

    sub.add_parser("stats", help="Show index statistics")

    p_export = sub.add_parser("export", help="Export the graph")
    p_export.add_argument("--format", choices=["json", "graphml"], default="json")

    args = parser.parse_args()
    root = args.root.resolve()

    if not (root / ".git").exists():
        # Try parent
        if (root.parent / ".git").exists():
            root = root.parent
        else:
            print(f"Error: not a git repository: {root}", file=sys.stderr)
            sys.exit(1)

    if args.command == "index":
        cmd_index(root, force=args.force)
    elif args.command == "report":
        cmd_report(root, top_n=args.top)
    elif args.command == "query":
        cmd_query(root, symbol=args.symbol, refs_only=args.refs)
    elif args.command == "path":
        cmd_path(root, from_file=args.from_file, to_file=args.to_file)
    elif args.command == "stats":
        cmd_stats(root)
    elif args.command == "export":
        cmd_export(root, fmt=args.format)
    else:
        parser.print_help()


if __name__ == "__main__":
    main()
