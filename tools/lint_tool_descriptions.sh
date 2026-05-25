#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
# Copyright (c) 2026 Havy.tech, LLC
#
# Lint daemon8 MCP tool descriptions against the required section format.
#
# Every file under crates/mcp/tool_descriptions/*.md must contain the
# following section headers (in any order, on a line of their own with
# either a leading "<header>:" or "<header>" + colon + content):
#
#   Purpose:   one-sentence summary
#   When:      trigger conditions
#   Args:      schema-supplementing description (or "Args: none")
#   Returns:   shape including envelope hints
#   Next:      typical follow-up tool ("none" if terminal)
#
# `Prereq:` and `Errors:` are recommended but not required (some tools have
# no prerequisites and no expected errors beyond the generic envelope shape).
#
# `instructions.md` and the `help/` topic markdown are exempt — those are
# narrative documents, not per-tool prompts.

set -uo pipefail

ROOT_DIR=$(cd "$(dirname "$0")/.." && pwd)
TARGET_DIR="$ROOT_DIR/crates/mcp/tool_descriptions"

if [ ! -d "$TARGET_DIR" ]; then
    echo "tool_descriptions directory not found: $TARGET_DIR" >&2
    exit 2
fi

REQUIRED_HEADERS=("Purpose" "When" "Args" "Returns" "Next")
EXEMPT_FILES=("instructions.md")

errors=0
checked=0

shopt -s nullglob
for file in "$TARGET_DIR"/*.md; do
    name=$(basename "$file")
    skip=false
    for exempt in "${EXEMPT_FILES[@]}"; do
        if [ "$name" = "$exempt" ]; then
            skip=true
            break
        fi
    done
    if [ "$skip" = true ]; then
        continue
    fi

    checked=$((checked + 1))
    missing=()
    for header in "${REQUIRED_HEADERS[@]}"; do
        if ! grep -Eq "^(##[[:space:]]+)?$header(:|[[:space:]]*$)" "$file"; then
            missing+=("$header")
        fi
    done

    if [ ${#missing[@]} -gt 0 ]; then
        errors=$((errors + 1))
        echo "tool_descriptions/$name missing required headers: ${missing[*]}" >&2
    fi
done

if [ "$errors" -gt 0 ]; then
    echo "" >&2
    echo "$errors tool description file(s) violate the required section format." >&2
    echo "Required headers per file: ${REQUIRED_HEADERS[*]}" >&2
    exit 1
fi

echo "tool_descriptions: $checked files conform to the required section format."
