#!/usr/bin/env bash
# Stable install/update entrypoint; retain scripts/install.sh for compatibility.
set -euo pipefail
exec bash "$(dirname -- "${BASH_SOURCE[0]}")/scripts/install.sh" "$@"
