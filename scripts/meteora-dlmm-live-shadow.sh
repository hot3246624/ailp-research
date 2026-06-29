#!/usr/bin/env bash
set -euo pipefail

# Read-only Meteora DLMM live-shadow runner.
#
# This stitches the existing safe helpers into one repeatable flow:
# snapshot -> swap-flow -> snapshot -> flow/snapshot join -> DLMM replay gates.
# It never signs or broadcasts transactions.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SPEC="${SPEC:-${ROOT}/data/solana/hot-pool/specs/meteora-solusdc-5rcf1dm8.json}"
OUT_DIR="${OUT_DIR:-${ROOT}/data/solana/hot-pool/swaps/meteora-sol-usdc}"
RPC="${SOLANA_RPC_URL:-https://solana-rpc.publicnode.com}"
FLOW_LIMIT="${FLOW_LIMIT:-40}"
SIGNATURE_SCAN_LIMIT="${SIGNATURE_SCAN_LIMIT:-250}"
MAX_SIGNATURE_PAGES="${MAX_SIGNATURE_PAGES:-2}"
REQUEST_SLEEP_MS="${REQUEST_SLEEP_MS:-100}"
MAX_SLOT_DISTANCE="${MAX_SLOT_DISTANCE:-250}"
WINDOW_OBSERVATIONS="${WINDOW_OBSERVATIONS:-15}"
STEP_OBSERVATIONS="${STEP_OBSERVATIONS:-5}"
MIN_WINDOWS="${MIN_WINDOWS:-5}"
HALF_WIDTH_BINS="${HALF_WIDTH_BINS:-5}"
CAPITAL_USD="${CAPITAL_USD:-1000}"
REPORT_OUT="${REPORT_OUT:-${OUT_DIR}/dlmm-live-shadow.latest.txt}"

usage() {
  cat >&2 <<'EOF'
usage: scripts/meteora-dlmm-live-shadow.sh [options]

Options:
  --spec <path>                    Pool spec JSON.
  --out-dir <path>                 Output directory for flow/snapshot/proxy files.
  --rpc <url>                      Solana RPC URL.
  --flow-limit <n>                 New swap-flow rows to collect this run.
  --signature-scan-limit <n>       Signatures to scan per page.
  --max-signature-pages <n>        Signature pages to scan.
  --request-sleep-ms <n>           Sleep between transaction requests.
  --max-slot-distance <n>          Strict join distance in slots.
  --window-observations <n>        Rolling DLMM replay window size.
  --step-observations <n>          Rolling DLMM replay step size.
  --min-windows <n>                Minimum rolling windows required.
  --half-width-bins <n>            DLMM range half width in bins.
  --capital-usd <n>                Replay capital.
  --report-out <path>              Text report path.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --spec)
      SPEC="$2"
      shift 2
      ;;
    --out-dir)
      OUT_DIR="$2"
      shift 2
      ;;
    --rpc)
      RPC="$2"
      shift 2
      ;;
    --flow-limit)
      FLOW_LIMIT="$2"
      shift 2
      ;;
    --signature-scan-limit)
      SIGNATURE_SCAN_LIMIT="$2"
      shift 2
      ;;
    --max-signature-pages)
      MAX_SIGNATURE_PAGES="$2"
      shift 2
      ;;
    --request-sleep-ms)
      REQUEST_SLEEP_MS="$2"
      shift 2
      ;;
    --max-slot-distance)
      MAX_SLOT_DISTANCE="$2"
      shift 2
      ;;
    --window-observations)
      WINDOW_OBSERVATIONS="$2"
      shift 2
      ;;
    --step-observations)
      STEP_OBSERVATIONS="$2"
      shift 2
      ;;
    --min-windows)
      MIN_WINDOWS="$2"
      shift 2
      ;;
    --half-width-bins)
      HALF_WIDTH_BINS="$2"
      shift 2
      ;;
    --capital-usd)
      CAPITAL_USD="$2"
      shift 2
      ;;
    --report-out)
      REPORT_OUT="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown arg: $1" >&2
      usage
      exit 2
      ;;
  esac
done

mkdir -p "${OUT_DIR}" "$(dirname "${REPORT_OUT}")"

FLOW="${OUT_DIR}/dlmm-swap-flow.jsonl"
FLOW_LATEST="${OUT_DIR}/dlmm-swap-flow.latest.json"
SNAPSHOTS="${OUT_DIR}/dlmm-bin-snapshots.jsonl"
SNAPSHOT_LATEST="${OUT_DIR}/dlmm-bin-snapshot.latest.json"
SNAPSHOTS_RAW="${OUT_DIR}/dlmm-bin-snapshots.raw.jsonl"
PROXY="${OUT_DIR}/dlmm-bin-flow-proxy.jsonl"
PROXY_LATEST="${OUT_DIR}/dlmm-bin-flow-proxy.latest.json"

{
  echo "meteora dlmm live shadow"
  echo "started_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "spec=${SPEC}"
  echo "out_dir=${OUT_DIR}"
  echo "flow_limit=${FLOW_LIMIT} signature_scan_limit=${SIGNATURE_SCAN_LIMIT} max_signature_pages=${MAX_SIGNATURE_PAGES}"
  echo "max_slot_distance=${MAX_SLOT_DISTANCE} window_observations=${WINDOW_OBSERVATIONS} step_observations=${STEP_OBSERVATIONS} capital_usd=${CAPITAL_USD}"
  echo

  echo "== snapshot before =="
  "${ROOT}/scripts/meteora-dlmm-snapshot.sh" \
    --spec "${SPEC}" \
    --out "${SNAPSHOTS}" \
    --raw-out "${SNAPSHOT_LATEST}" \
    --raw-jsonl-out "${SNAPSHOTS_RAW}" \
    --rpc "${RPC}" \
    --append

  echo
  echo "== swap flow =="
  "${ROOT}/scripts/meteora-dlmm-swap-flow.sh" \
    --spec "${SPEC}" \
    --out "${FLOW}" \
    --raw-out "${FLOW_LATEST}" \
    --rpc "${RPC}" \
    --limit "${FLOW_LIMIT}" \
    --signature-scan-limit "${SIGNATURE_SCAN_LIMIT}" \
    --max-signature-pages "${MAX_SIGNATURE_PAGES}" \
    --request-sleep-ms "${REQUEST_SLEEP_MS}" \
    --append

  echo
  echo "== snapshot after =="
  "${ROOT}/scripts/meteora-dlmm-snapshot.sh" \
    --spec "${SPEC}" \
    --out "${SNAPSHOTS}" \
    --raw-out "${SNAPSHOT_LATEST}" \
    --raw-jsonl-out "${SNAPSHOTS_RAW}" \
    --rpc "${RPC}" \
    --append

  echo
  echo "== join flow/snapshots =="
  JOIN_SUMMARY="$(node "${ROOT}/scripts/meteora-dlmm-join-flow-snapshots.cjs" \
    --flow "${FLOW}" \
    --snapshots "${SNAPSHOTS}" \
    --out "${PROXY}" \
    --raw-out "${PROXY_LATEST}" \
    --max-slot-distance "${MAX_SLOT_DISTANCE}" \
    --active-bin-source flow-price)"
  echo "${JOIN_SUMMARY}"
  JOINED_ROWS="$(node -e 'const input=JSON.parse(process.argv[1]); console.log(Number(input.joined_rows || 0));' "${JOIN_SUMMARY}")"
  if [[ "${JOINED_ROWS}" -lt "${WINDOW_OBSERVATIONS}" ]]; then
    echo
    echo "== replay skipped =="
    echo "joined_rows=${JOINED_ROWS} is below window_observations=${WINDOW_OBSERVATIONS}; no replay-grade live-shadow window under current slot-distance gate"
    echo "finished_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "report=${REPORT_OUT}"
    exit 0
  fi
  POSSIBLE_WINDOWS="$((1 + (JOINED_ROWS - WINDOW_OBSERVATIONS) / STEP_OBSERVATIONS))"
  if [[ "${POSSIBLE_WINDOWS}" -lt "${MIN_WINDOWS}" ]]; then
    echo
    echo "== replay skipped =="
    echo "joined_rows=${JOINED_ROWS} can produce ${POSSIBLE_WINDOWS} windows, below min_windows=${MIN_WINDOWS}; collect more live-shadow rows before promotion gate"
    echo "finished_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "report=${REPORT_OUT}"
    exit 0
  fi

  echo
  echo "== full proxy replay =="
  cargo run -q -p autopool-cli -- replay-dlmm-bins \
    --spec "${SPEC}" \
    --bins "${PROXY}" \
    --half-width-bins "${HALF_WIDTH_BINS}" \
    --capital-usd "${CAPITAL_USD}"

  echo
  echo "== rolling proxy gate =="
  cargo run -q -p autopool-cli -- replay-dlmm-bin-windows \
    --spec "${SPEC}" \
    --bins "${PROXY}" \
    --window-observations "${WINDOW_OBSERVATIONS}" \
    --step-observations "${STEP_OBSERVATIONS}" \
    --min-windows "${MIN_WINDOWS}" \
    --half-width-bins "${HALF_WIDTH_BINS}" \
    --capital-usd "${CAPITAL_USD}"

  echo
  echo "finished_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "report=${REPORT_OUT}"
} | tee "${REPORT_OUT}"
