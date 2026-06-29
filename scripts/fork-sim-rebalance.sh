#!/usr/bin/env bash
set -euo pipefail

# Local Base fork simulation for the dry-run calldata.
#
# Requires:
#   BASE_RPC_URL  read-only Base RPC URL
#   foundry tools: anvil, cast
#
# The script never signs or broadcasts on mainnet. It starts a local fork, funds an
# anvil dev account, executes the dry-run's real SwapRouter + NPM calldata, and
# writes receipts/plans under WORKDIR.

: "${BASE_RPC_URL:?set BASE_RPC_URL to a Base RPC endpoint}"

POOL="${POOL:-0x4e506648d493c8870f55e870480f92f2f33ece51}" # WETH-AERO GaugesV3
WETH="${WETH:-0x4200000000000000000000000000000000000006}"
AERO="${AERO:-0x940181a94A35A4569E4529A3CDfB74e38FD98631}"
PORT="${PORT:-8547}"
FORK_RPC="http://127.0.0.1:${PORT}"
CUPS="${CUPS:-80}"
WORKDIR="${WORKDIR:-/tmp/ailp-fork-sim}"
FORK_BLOCK_NUMBER="${FORK_BLOCK_NUMBER:-}"

SENDER="${SENDER:-0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266}"
CAPITAL_USD="${CAPITAL_USD:-10000}"
TOKEN0_USD="${TOKEN0_USD:-3000}"
FRESH_HALF_WIDTH_TICKS="${FRESH_HALF_WIDTH_TICKS:-600}"
REBALANCE_HALF_WIDTH_TICKS="${REBALANCE_HALF_WIDTH_TICKS:-600}"
SLIPPAGE_BPS="${SLIPPAGE_BPS:-30}"
RISK_TOKEN_SIDE="${RISK_TOKEN_SIDE:-token1}"
MAX_RISK_TOKEN_SHARE="${MAX_RISK_TOKEN_SHARE:-0.8}"
WETH_DEPOSIT="${WETH_DEPOSIT:-20ether}"
GAS_LIMIT="${GAS_LIMIT:-5000000}"
MAX_UINT="115792089237316195423570985008687907853269984665640564039457584007913129639935"

ANVIL_PID=""

cleanup() {
  if [[ "${KEEP_ANVIL_ON_FAIL:-0}" == "1" && "${SCRIPT_FAILED:-0}" == "1" ]]; then
    echo "keeping anvil alive for debugging: pid=${ANVIL_PID}, rpc=${FORK_RPC}" >&2
    disown "${ANVIL_PID}" 2>/dev/null || true
    return
  fi
  if [[ -n "${ANVIL_PID}" ]] && kill -0 "${ANVIL_PID}" >/dev/null 2>&1; then
    kill "${ANVIL_PID}" >/dev/null 2>&1 || true
    wait "${ANVIL_PID}" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing required command: $1" >&2
    exit 1
  }
}

need anvil
need cast
need jq

mkdir -p "${WORKDIR}"
rm -f "${WORKDIR}"/*.json "${WORKDIR}"/*.log

echo "starting anvil fork on ${FORK_RPC}"
ANVIL_ARGS=(
  --fork-url "${BASE_RPC_URL}"
  --compute-units-per-second "${CUPS}"
  --port "${PORT}"
  --silent
)
if [[ -n "${FORK_BLOCK_NUMBER}" ]]; then
  ANVIL_ARGS+=(--fork-block-number "${FORK_BLOCK_NUMBER}")
fi
anvil "${ANVIL_ARGS[@]}" >"${WORKDIR}/anvil.log" 2>&1 &
ANVIL_PID=$!

for _ in $(seq 1 60); do
  if cast block-number --rpc-url "${FORK_RPC}" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
cast block-number --rpc-url "${FORK_RPC}" >/dev/null

send_json() {
  local label="$1"
  shift
  local out="${WORKDIR}/${label}.json"
  local err="${WORKDIR}/${label}.err.log"
  echo "tx ${label}"
  if ! cast send --rpc-url "${FORK_RPC}" --unlocked --from "${SENDER}" --gas-limit "${GAS_LIMIT}" --json "$@" >"${out}" 2>"${err}"; then
    SCRIPT_FAILED=1
    echo "tx ${label} failed" >&2
    cat "${err}" >&2
    if [[ -s "${out}" ]]; then
      cat "${out}" >&2
    fi
    return 1
  fi
  local status
  status="$(jq -r '.status // .receipt.status // "unknown"' "${out}" 2>/dev/null || echo unknown)"
  local gas
  gas="$(jq -r '.gasUsed // .receipt.gasUsed // "unknown"' "${out}" 2>/dev/null || echo unknown)"
  echo "tx ${label} status=${status} gasUsed=${gas}"
  if [[ "${status}" != "0x1" && "${status}" != "1" ]]; then
    SCRIPT_FAILED=1
    echo "tx ${label} reverted" >&2
    jq -r '.revertReason // .receipt.revertReason // empty' "${out}" >&2 || true
    return 1
  fi
}

sanitize_rpc() {
  sed -E 's#https://base-mainnet\.g\.alchemy\.com/v2/[A-Za-z0-9_-]+#<BASE_RPC_URL>#g'
}

write_plan() {
  local label="$1"
  shift
  local out="${WORKDIR}/${label}.json"
  local err="${WORKDIR}/${label}.err.log"
  echo "build ${label}"
  if ! BASE_RPC_URL="${FORK_RPC}" cargo run -q -p autopool-cli -- dry-run-rebalance "$@" \
    --skip-quoter \
    --format json \
    >"${out}" 2>"${err}"; then
    SCRIPT_FAILED=1
    echo "build ${label} failed" >&2
    sanitize_rpc <"${err}" >&2
    return 1
  fi
}

gas_used() {
  jq -r '.gasUsed // .receipt.gasUsed // "unknown"' "$1"
}

gas_used_dec() {
  local gas
  gas="$(gas_used "$1")"
  if [[ "${gas}" == 0x* ]]; then
    cast to-dec "${gas}"
  else
    echo "${gas}"
  fi
}

first_transfer_token_id() {
  local receipt="$1"
  local zero_topic="0x0000000000000000000000000000000000000000000000000000000000000000"
  local topic
  topic="$(jq -r --arg zero "${zero_topic}" '
    (.logs // .receipt.logs // [])
    | .[]
    | select((.topics[0] // "") == "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef")
    | select((.topics[1] // "") == $zero)
    | .topics[3] // empty
  ' "${receipt}" | head -n 1)"
  if [[ -z "${topic}" ]]; then
    return 1
  fi
  cast to-dec "${topic}"
}

echo "fund sender with WETH and approvals"
send_json deposit_weth "${WETH}" "deposit()" --value "${WETH_DEPOSIT}"

write_plan fresh-plan \
  --pool-address "${POOL}" \
  --capital-usd "${CAPITAL_USD}" \
  --token0-usd "${TOKEN0_USD}" \
  --decimals0 18 \
  --decimals1 18 \
  --half-width-ticks "${FRESH_HALF_WIDTH_TICKS}" \
  --recipient "${SENDER}" \
  --slippage-bps "${SLIPPAGE_BPS}" \
  --risk-token-side "${RISK_TOKEN_SIDE}" \
  --max-risk-token-share "${MAX_RISK_TOKEN_SHARE}" \
  --staked false

ROUTER="$(jq -r '.actions[] | select(.step == "swap") | .contract' "${WORKDIR}/fresh-plan.json")"
NPM="$(jq -r '.npm_multicall.contract' "${WORKDIR}/fresh-plan.json")"
SWAP_CALLDATA="$(jq -r '.actions[] | select(.step == "swap") | .calldata' "${WORKDIR}/fresh-plan.json")"
FRESH_NPM_CALLDATA="$(jq -r '.npm_multicall.calldata' "${WORKDIR}/fresh-plan.json")"
CURRENT_LOWER="$(jq -r '.target_range.lower' "${WORKDIR}/fresh-plan.json")"
CURRENT_UPPER="$(jq -r '.target_range.upper' "${WORKDIR}/fresh-plan.json")"

send_json approve_weth_router "${WETH}" "approve(address,uint256)" "${ROUTER}" "${MAX_UINT}"
send_json approve_weth_npm "${WETH}" "approve(address,uint256)" "${NPM}" "${MAX_UINT}"
send_json approve_aero_npm "${AERO}" "approve(address,uint256)" "${NPM}" "${MAX_UINT}"

echo "execute fresh dry-run calldata: swap + mint"
send_json fresh_swap "${ROUTER}" "${SWAP_CALLDATA}"
send_json fresh_npm_multicall "${NPM}" "${FRESH_NPM_CALLDATA}"

TOKEN_ID="$(first_transfer_token_id "${WORKDIR}/fresh_npm_multicall.json")"
echo "minted tokenId=${TOKEN_ID}"

echo "snapshot minted position"
BASE_RPC_URL="${FORK_RPC}" cargo run -q -p autopool-cli -- monitor-position \
  --token-id "${TOKEN_ID}" \
  --pool-address "${POOL}" \
  --output "${WORKDIR}/position-monitor.jsonl" \
  --iterations 1 \
  --token0-usd "${TOKEN0_USD}" \
  --risk-token-side "${RISK_TOKEN_SIDE}" \
  --max-risk-token-share "${MAX_RISK_TOKEN_SHARE}" \
  --format json \
  >"${WORKDIR}/position-monitor.stdout.json"

write_plan rebalance-plan \
  --pool-address "${POOL}" \
  --capital-usd "${CAPITAL_USD}" \
  --token0-usd "${TOKEN0_USD}" \
  --decimals0 18 \
  --decimals1 18 \
  --half-width-ticks "${REBALANCE_HALF_WIDTH_TICKS}" \
  --current-lower "${CURRENT_LOWER}" \
  --current-upper "${CURRENT_UPPER}" \
  --token-id "${TOKEN_ID}" \
  --recipient "${SENDER}" \
  --slippage-bps "${SLIPPAGE_BPS}" \
  --risk-token-side "${RISK_TOKEN_SIDE}" \
  --max-risk-token-share "${MAX_RISK_TOKEN_SHARE}" \
  --staked false

REBALANCE_NPM_CALLDATA="$(jq -r '.npm_multicall.calldata' "${WORKDIR}/rebalance-plan.json")"
REBALANCE_CALLS="$(jq -r '.npm_multicall.calls' "${WORKDIR}/rebalance-plan.json")"

echo "execute rebalancing NPM multicall (${REBALANCE_CALLS} calls)"
send_json rebalance_npm_multicall "${NPM}" "${REBALANCE_NPM_CALLDATA}"

cat >"${WORKDIR}/summary.json" <<JSON
{
  "pool": "${POOL}",
  "sender": "${SENDER}",
  "fresh_target_range": {"lower": ${CURRENT_LOWER}, "upper": ${CURRENT_UPPER}},
  "minted_token_id": "${TOKEN_ID}",
  "fresh_swap_gas": "$(gas_used "${WORKDIR}/fresh_swap.json")",
  "fresh_swap_gas_dec": "$(gas_used_dec "${WORKDIR}/fresh_swap.json")",
  "fresh_npm_multicall_gas": "$(gas_used "${WORKDIR}/fresh_npm_multicall.json")",
  "fresh_npm_multicall_gas_dec": "$(gas_used_dec "${WORKDIR}/fresh_npm_multicall.json")",
  "rebalance_npm_calls": ${REBALANCE_CALLS},
  "rebalance_npm_multicall_gas": "$(gas_used "${WORKDIR}/rebalance_npm_multicall.json")",
  "rebalance_npm_multicall_gas_dec": "$(gas_used_dec "${WORKDIR}/rebalance_npm_multicall.json")",
  "position_monitor_jsonl": "${WORKDIR}/position-monitor.jsonl",
  "workdir": "${WORKDIR}"
}
JSON

cat "${WORKDIR}/summary.json"
