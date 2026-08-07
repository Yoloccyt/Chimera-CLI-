#!/usr/bin/env bash
# =============================================================================
# check_doc_consistency.sh - Architecture document three-way reconciliation
# =============================================================================
# Purpose: detect drift between Cargo.toml (code), docs/ (index), CHANGELOG.md (changelog)
# Scope:   5 categories / 12 checks; all assertions back to Cargo.toml as canonical truth
# Author:  staff-engineer-mode (documentation-lifecycle specialist)
# Refs:    docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md
#
# Categories:
#   A. Structural invariants   - Cargo.toml members count == disk crates count
#                              - workspace.package.version field present
#   B. Index document freshness - 4 crate-index docs contain current crate count
#                                - 5 main docs contain current baseline string
#   C. Changelog reconciliation - CHANGELOG.md has `## vX.Y.Z-omega` header for current version
#   D. ADR physical vs index   - ADR-*.md files on disk match adr_index.md declaration
#   E. Policy compliance       - CONVENTIONS.md-declared subdirs exist
#                              - DOCUMENT_LIFECYCLE_POLICY.md (SoT) exists
#
# Exit code: 0 = clean, 1 = gap found
# Note:     Linux/CI uses this; Windows uses check_doc_consistency.ps1. Keep in sync.
# =============================================================================
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

status=0
report=()

# Helper: discover file by ASCII basename under known parent dir.
# Avoids hardcoding CJK characters that may corrupt on Windows IDE write.
nuxus_rules="$(find .trae/rules -name '*nuxus*.md' -type f 2>/dev/null | head -1 || true)"
[ -z "$nuxus_rules" ] && nuxus_rules=".trae/rules/nuxus-rule-not-found.md"

# =============================================================================
# A. Structural invariants (Cargo.toml as canonical source)
# =============================================================================

# A1. canonical crate count: Cargo.toml members == disk crates/*/Cargo.toml
n_members=$(awk '/members = \[/{f=1} f; /\]/{if(f)exit}' Cargo.toml | grep -oE '"crates/[^"]+"' | wc -l | tr -d ' ')
n_dirs=$(find crates -mindepth 2 -maxdepth 2 -name Cargo.toml 2>/dev/null | wc -l | tr -d ' ')
report+=("[A1] canonical crate count (Cargo.toml members) = ${n_members}")
if [ "$n_members" != "$n_dirs" ]; then
    report+=("[GAP-A1] Cargo.toml members(${n_members}) vs disk crates/*/Cargo.toml(${n_dirs}) mismatch")
    status=1
fi

# A2. version field: Cargo.toml must have workspace.package.version
current_version=$(grep -E '^version\s*=\s*"[^"]+"' Cargo.toml | head -1 | sed -E 's/^version\s*=\s*"([^"]+)"/\1/' | tr -d '\n\r')
if [ -z "$current_version" ]; then
    report+=("[GAP-A2] Cargo.toml missing workspace.package.version field")
    current_version="UNKNOWN"
    status=1
else
    report+=("[A2] canonical version (Cargo.toml workspace.package.version) = ${current_version}")
fi

# =============================================================================
# B. Index document freshness (derived from code layer)
# =============================================================================

# B1. main docs contain current crate count (4 crate-index docs)
# Note: adr_index.md is an ADR index, not a crate index, so NOT checked here.
b1_docs=(
    "docs/architecture/README.md"
    "docs/architecture/CODE_WIKI.md"
    ".claude/CLAUDE.md"
    "${nuxus_rules}"
)
for f in "${b1_docs[@]}"; do
    if [[ "$f" == *not-found* ]]; then
        # 2026-08-07 适配: *.md 在 gitignore 策略下仅存于本地(bb471f9 移除跟踪),
        # CI checkout 必然缺失 nuxus 规则文档 —— 降级为 warn 而非阻断。
        report+=("[B1-warn] nuxus rules file not discovered (gitignore *.md 策略,仅本地维护)")
        continue
    fi
    if [ ! -f "$f" ]; then
        report+=("[B1-warn] missing document: ${f} (gitignore *.md 策略,CI 环境无此文档,跳过)")
        continue
    fi
    if ! grep -qE "${n_members}[[:space:]]*crate|${n_members}个crate|${n_members}[[:space:]]*Crate" "$f"; then
        report+=("[GAP-B1] ${f} does not contain current crate count (${n_members}), possibly stale")
        status=1
    fi
done

# B2. main docs contain current baseline string (DOCUMENT_LIFECYCLE_POLICY 6.4 trigger b)
b2_docs=(
    "docs/architecture/CODE_WIKI.md"
    ".claude/CLAUDE.md"
    "${nuxus_rules}"
    "CHANGELOG.md"
    "docs/architecture/INDEX.md"
)
for f in "${b2_docs[@]}"; do
    if [[ "$f" == *not-found* ]]; then
        report+=("[B2-warn] nuxus rules file not discovered (gitignore *.md 策略,仅本地维护)")
        continue
    fi
    if [ ! -f "$f" ]; then
        report+=("[B2-warn] missing document: ${f} (gitignore *.md 策略,CI 环境无此文档,跳过)")
        continue
    fi
    if ! grep -qF "${current_version}" "$f"; then
        report+=("[GAP-B2] ${f} does not contain baseline string (${current_version})")
        status=1
    fi
done

# =============================================================================
# C. Changelog reconciliation
# =============================================================================

# C1. CHANGELOG.md must have ## vX.Y.Z-omega header for current version
if [ ! -f "CHANGELOG.md" ]; then
    # 2026-08-07 适配: CHANGELOG.md 在 gitignore *.md 策略下仅存于本地(bb471f9 移除跟踪),
    # CI checkout 必然缺失 —— 降级为 warn 而非阻断。
    report+=("[C1-warn] missing document: CHANGELOG.md (gitignore *.md 策略,仅本地维护,跳过)")
else
    # Dual-format compatible (PROBE R1): bracket style (## [2.20.0-omega]) and bare style (## v2.20.0-omega)
    if ! grep -qE "^##\s+\[?v?${current_version}\]?(\s|$)" CHANGELOG.md; then
        report+=("[GAP-C1] CHANGELOG.md missing ## v${current_version} header (should be first entry)")
        status=1
    else
        report+=("[C1] CHANGELOG.md contains ## v${current_version} header")
    fi
fi

# =============================================================================
# D. ADR physical files vs index reconciliation
# =============================================================================

# D1. ADR physical file main numbers (dedupe rev[0-4] multi-version)
# Use python3 for robust parsing of complex multi-version filenames
if compgen -G "docs/architecture/ADR-*.md" > /dev/null; then
    adr_total_files=$(find docs/architecture -maxdepth 1 -name 'ADR-*.md' -type f | wc -l | tr -d ' ')
    adr_main_count=$(python3 -c "
import re, os, glob
seen = set()
for p in glob.glob('docs/architecture/ADR-*.md'):
    base = os.path.basename(p)
    m = re.match(r'^ADR-(\d{3})(?:-(rev\d+))?', base)
    if m:
        seen.add(m.group(1))
print(len(seen))
")
    report+=("[D1] ADR physical file main numbers = ${adr_main_count} (with ${adr_total_files} files, some multi-version)")
else
    adr_main_count=0
    adr_total_files=0
    report+=("[WARN-D1] no ADR-*.md files found in docs/architecture/")
fi

# D2. adr_index.md must declare ADR total
# Logic:
#   - declared = the ADR total declared in adr_index.md (may include reserved/historical)
#   - physical = number of distinct ADR main numbers with physical files
#   - GAP only if declared < physical (real undercount)
if [ ! -f "docs/architecture/adr_index.md" ]; then
    # 2026-08-07 适配: 同 B/C —— gitignore *.md 策略下缺失文档降级为 warn。
    report+=("[D2-warn] missing document: docs/architecture/adr_index.md (gitignore *.md 策略,仅本地维护,跳过)")
else
    # Use python3 for robust CJK+ASCII regex matching
    declared_total=$(python3 -c "
import re, sys
with open('docs/architecture/adr_index.md', encoding='utf-8') as fh:
    for line in fh:
        m = re.search(r'(\d+)\s*个\s*ADR', line)
        if m:
            print(m.group(1))
            sys.exit(0)
sys.exit(1)
" 2>/dev/null || echo "")

    if [ -z "$declared_total" ]; then
        report+=("[WARN-D2] adr_index.md has no machine-readable ADR total declaration")
    elif [ "$declared_total" -lt "$adr_main_count" ]; then
        report+=("[GAP-D2] adr_index.md declares ${declared_total} ADRs, disk has ${adr_main_count} main numbers (index undercount)")
        status=1
    elif [ "$declared_total" -eq "$adr_main_count" ]; then
        report+=("[D2] adr_index.md declares ${declared_total} ADRs, matches disk ${adr_main_count} main numbers")
    else
        reserved=$((declared_total - adr_main_count))
        report+=("[D2-INFO] adr_index.md declares ${declared_total} ADRs, disk has ${adr_main_count} main numbers (${reserved} reserved/historical, expected)")
    fi
fi

# =============================================================================
# E. Policy compliance (CONVENTIONS.md declared subdirs + SoT file)
# =============================================================================

# E1. CONVENTIONS.md declared subdirs must exist
required_dirs=(
    "docs/architecture/audit:audit/governance signoff and review records"
    "docs/architecture/governance:governance/policy documents"
    "docs/architecture/_archive:_archive/historical snapshots"
    "docs/architecture/_blueprints:_blueprints/design blueprints (not yet implemented)"
)
for entry in "${required_dirs[@]}"; do
    dir="${entry%%:*}"
    purpose="${entry#*:}"
    if [ ! -d "$dir" ]; then
        # 2026-08-07 适配: 文档目录仅存于本地(gitignore *.md 策略,远程无 md 文件即无目录),
        # 降级为 warn 而非阻断。
        report+=("[E1-warn] CONVENTIONS.md declared subdir missing: ${dir} (${purpose}; gitignore *.md 策略,仅本地维护,跳过)")
    fi
done

# E2. SoT policy file existence
if [ ! -f "docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md" ]; then
    # 2026-08-07 适配: 同 E1 —— gitignore *.md 策略下仅本地维护,降级为 warn。
    report+=("[E2-warn] missing policy file: docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md (gitignore *.md 策略,仅本地维护,跳过)")
fi

# =============================================================================
# Report output
# =============================================================================

for line in "${report[@]}"; do echo "$line"; done

echo ""
if [ "$status" = 0 ]; then
    echo "[OK] three-way reconciliation all pass (5 categories / 12 checks): canonical version=${current_version}, ${n_members} crates, baseline aligned"
else
    echo "[FAIL] three-way reconciliation found gaps, see [GAP-*] lines above, fix and rerun"
fi
exit $status
