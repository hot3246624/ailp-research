#!/usr/bin/env bash
set -euo pipefail

# Read-only Meteora DLMM snapshot helper.
#
# The repo stays Rust-first and does not vendor Node dependencies. This wrapper
# installs the official Meteora DLMM SDK into a temp cache and exposes it via
# NODE_PATH for scripts/meteora-dlmm-snapshot.cjs.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SDK_DIR="${METEORA_DLMM_NODE_DIR:-/tmp/ailp-meteora-dlmm-node}"

mkdir -p "${SDK_DIR}"
if [[ ! -d "${SDK_DIR}/node_modules/@meteora-ag/dlmm" ]]; then
  (
    cd "${SDK_DIR}"
    npm init -y >/dev/null
    npm install @meteora-ag/dlmm@1.9.10 @solana/web3.js@latest >/dev/null
  )
fi

export NODE_PATH="${SDK_DIR}/node_modules${NODE_PATH:+:${NODE_PATH}}"
exec node "${ROOT}/scripts/meteora-dlmm-snapshot.cjs" "$@"
