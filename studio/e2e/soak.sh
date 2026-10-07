#!/bin/sh
# Nightly interface soak (T-482), for launchd or cron: a release build,
# a preview-only studio on a free port with a temporary data folder (never
# --device, never port 8080, never studio-data/), the scripted operator for
# SOAK_MINUTES (default 120). The report goes to docs/qa/soak-<date>.md
# when the checkout has a docs/ folder, else to SOAK_REPORT_DIR or the
# test's output folder. Exit status: 0 when every criterion holds.
set -eu
cd "$(dirname "$0")"
PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
export PATH
export SOAK_MINUTES="${SOAK_MINUTES:-120}"
exec npm run soak
