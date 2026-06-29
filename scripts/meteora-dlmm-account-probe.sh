#!/usr/bin/env bash
set -euo pipefail

# Read-only Meteora DLMM account reconstruction probe.
#
# This wrapper reuses the temp SDK cache from the other Meteora helpers. It does
# not sign or broadcast transactions; it only reads transaction metadata and
# current account state to evaluate whether historical bin-array replay can be
# reconstructed from public RPC alone.

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
exec node "${ROOT}/scripts/meteora-dlmm-account-probe.cjs" "$@"
