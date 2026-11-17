#!/usr/bin/env bash
# Pages compose DesignSystem components and tokens; raw sizes, radii and tints belong in DesignSystem/ only.
# See docs/specs/design-system-v1.md §10. A line drawing content (not a control) may opt out with
# `// style-exempt: <reason>`.
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="$ROOT_DIR/Sources/Candid"
PATTERN='\.font\(\.system\((size:|\.)|cornerRadius: *[0-9]|Color\.primary\.opacity\(|\.font\(\.(largeTitle|title|title2|title3|headline|subheadline|body|callout|footnote|caption|caption2)\b'
# System-default controls the spec replaces with components (§6): grouped forms, boxes, dialogs, checkboxes.
SYSTEM='Form \{|GroupBox|confirmationDialog|\.alert\(|\.sheet\(|buttonStyle\(\.bordered|toggleStyle\(\.checkbox|ContentUnavailableView|DisclosureGroup|pickerStyle\(\.segmented|menuStyle\(\.borderlessButton\)|\bDivider\(\)'
HITS="$( (grep -rnE "$PATTERN" "$APP" --include='*.swift'; grep -rnE "$SYSTEM" "$APP" --include='*.swift') | grep -v "/DesignSystem/" | grep -v 'style-exempt:' || true)"
if [[ -n "$HITS" ]]; then
  echo "$HITS" | sed "s|$ROOT_DIR/||"
  echo "Raw styles outside DesignSystem: $(echo "$HITS" | wc -l | tr -d ' ') line(s). Use TypeScale/Radius/Surface or add a component." >&2
  exit 1
fi
echo "Style check passed."
