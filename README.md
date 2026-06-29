# AILP Research

Rust-first research system for **AI-assisted autonomous liquidity provision**: DEX pool allocation, range selection, rebalance timing, and LP inventory risk management.

AILP here means an AI-assisted / autonomous LP strategy system. The internal crate names still use `autopool-*` for now to avoid churn while the research loop is changing quickly.

The project is intentionally split into two loops:

- **Research loop**: ingest public yield data, chain data, price paths, and wallet state; run simulations; produce rebalance proposals.
- **Execution loop**: simulate transactions, enforce risk limits, submit private or public transactions, and record realized outcomes.

The current scaffold favors one validation market first: **Base / Aerodrome Slipstream**. This keeps the first research loop focused on strategy quality instead of multi-chain integration breadth. Other high-volume ecosystems can be added after the Base pilot has a credible net-PnL record.

USDC-USDT-style stable LPs are treated as control pools for validating the replay engine. The target opportunity set is higher APR, higher fee-density, higher inventory-risk pools such as WETH-USDC, WETH-AERO, and other active volatile pairs.

## Why Not Rank By APR Only

High APR in a narrow range is often compensation for inventory risk. Once price leaves the range, the LP position becomes increasingly one-sided, and the token left in inventory may be the token the market is selling. The system therefore optimizes for expected net edge:

```text
expected fees
- expected adverse inventory drift
- expected impermanent loss
- gas and swap costs
- slippage and MEV costs
- tail-risk and exposure penalties
```

## Workspace

- `crates/core`: shared domain types and strategy/provider traits.
- `crates/defillama`: DeFiLlama yields API client and mapper into internal yield snapshots.
- `crates/evm`: EVM chain, pool-reader, simulation, and execution interfaces, starting with Base.
- `crates/strategy`: scoring, risk filtering, and initial range optimizer primitives.
- `crates/backtest`: replay/simulation shell.
- `apps/cli`: operator CLI for discovery, dry-runs, and later execution commands.

## First Useful Commands

```bash
cargo check
cargo run -p autopool-cli -- architecture
cargo run -p autopool-cli -- scan-yields --chain Base --project aerodrome-slipstream --min-tvl-usd 100000 --lp-only
cargo run -p autopool-cli -- pilot-universe --profile opportunistic --min-tvl-usd 50000 --min-apy 10 --min-fee-bps 5 --include-symbol WETH-AERO --include-symbol WETH-USDC
cargo run -p autopool-cli -- pilot-universe --profile control --min-tvl-usd 100000 --max-reward-share 0.5
BASE_RPC_URL=https://your-base-rpc.example cargo run -p autopool-cli -- sample-base-network --eth-usd 3500
BASE_RPC_URL=https://your-base-rpc.example cargo run -p autopool-cli -- resolve-slipstream-pools --profile opportunistic --include-symbol WETH-AERO --include-symbol WETH-USDC --limit 8
BASE_RPC_URL=https://your-base-rpc.example cargo run -p autopool-cli -- sample-slipstream-events --profile opportunistic --include-symbol WETH-AERO --include-symbol WETH-USDC --lookback-blocks 100 --log-chunk-blocks 10 --limit 4
BASE_RPC_URL=https://your-base-rpc.example cargo run -p autopool-cli -- backfill-slipstream-events --profile opportunistic --include-symbol WETH-AERO --include-symbol WETH-USDC --lookback-blocks 7200 --max-blocks-per-run 200 --log-chunk-blocks 10 --poll-seconds 30 --iterations 1
cargo run -p autopool-cli -- summarize-slipstream-events --data-dir data/base/aerodrome
cargo run -p autopool-cli -- replay-events --symbol WETH-AERO --fee-bps 21.25 --token0-usd 1574 --narrow-half-width 100
cargo run -p autopool-cli -- replay-scenario --scenario crash --move-ticks 6000 --fee-bps 21.25 --token0-usd 1574 --narrow-half-width 300 --action-delay-blocks 3 --funding-bps-per-day 10
cargo run -p autopool-cli -- walk-forward --symbol WETH-AERO --fee-bps 21.25 --token0-usd 1574 --train-swaps 1000 --test-swaps 500 --action-delay-blocks 3
BASE_RPC_URL=https://your-base-rpc.example cargo run -p autopool-cli -- scan-pool-activity --min-tvl-usd 300000 --limit 8 --lookback-blocks 1000
```

The `replay-events` command turns collected swap events into LP profit-and-loss
for a battery of range policies (hold, passive-wide, narrow-static,
narrow-rebalance, vol-scaled, hard-exit-stop, hedged-narrow) with PnL attribution
into fees, inventory IL, gas, slippage, plus tail metrics (max drawdown, longest
forced risk-asset hold, toxic fees, hedge PnL) and an execution-latency model. It
is the strategy-research environment (architecture Milestone 3). `replay-scenario`
runs the same battery against synthetic calm/pump/crash/chop paths.

See `docs/first-data-analysis.md` for the first Base / Aerodrome event-readout;
`docs/replay-weth-aero.md` for the first range-policy replay (fee density dominates
a calm window; rebalancing-on-exit is a tax) plus the discovery of the real active
WETH-USDC pool; and `docs/tail-risk-scenarios.md` for the down-crash / chop /
hedging stress tests (one-way hard-exit and a short hedge cap the down-tail;
mechanical rebalancing is ruinous in crash and chop); and `docs/walk-forward.md`
for out-of-sample calibration of the adaptive policy (per-fold calibration beats
fixed-parameter and static, but only ties hold on the current calm window — i.e.
do not LP this pool in this regime); and `docs/pool-discovery.md` for ranking pools
by realized on-chain tick volatility (`scan-pool-activity`) to find liquid pairs
that actually move — USDC-AERO is the most active liquid Slipstream pool and a clean
USD-numeraire research target; and `docs/real-regime-replay.md` for the first replay
on real *trending* data (collected fast via the public `mainnet.base.org` endpoint,
which allows large `getLogs`): every LP policy loses to hold in a trend, so LP is a
ranging-regime strategy and the meta-decision is whether to be an LP at all; and
`docs/lvr-attribution.md` for LVR + reward attribution — the pool has real LP
fee-alpha (fee − LVR > 0 in every regime), but it only converts to net
outperformance in calm because inventory beta drags trends, which is the strongest
argument for a dynamic delta hedge; and `docs/multi-path.md` for the moving-block
bootstrap (`multi-path`) — across the realistic volatility×cost distribution the
delta hedge collapses net-PnL variance (lowest std) but does not fix rebalance
churn, so narrow rebalancing policies net-lose despite positive gross fee−LVR, and
the robust positive-expectancy LP is *low-churn* (passive-wide beats hold 72% of
paths). 21 bps is too thin — find higher-fee/ranging pools; and `docs/pool-pivot.md`
for the fee-density threshold (5 bps: fee−LVR < 0, no alpha; 21 bps: alpha but
churn-eaten; need higher fee) and the robust scan showing Slipstream's active pools
are all low-fee while high-fee pools are inactive — CTR-USDC (100 bps, real vol) is
the one candidate worth a dedicated test. The deployable shape that emerges is
`hedged_wide` (a wide, never-rebalanced band + dynamic delta hedge): on the real
CTR-USDC crash it stayed ~flat (−$15) versus hold's −$537 and beat hold on 85% of
paths with ~10× less drawdown — direction-robust, low-variance LP. Its mean edge is
small at 100 bps (fee-alpha ≈ hedge cost); higher fee density is the remaining lever.
Ranking the scan by `fee/vol` then found the **positive** result: **WETH-USDC at
200 bps** (high fee, *low* vol) is the first pool with clear positive LP expectancy —
a **delta-hedged narrow band nets ~+$90/$10k with std $20 and ~$1 drawdown regardless
of drift** (narrow-static +$109–142 if you accept directional variance). Completed
thesis: target **high fee × LOW volatility**, not high vol — churn cost *is*
volatility, so a calm high-fee pool lets a tight (optionally delta-hedged) band keep
its fee-alpha. Caveat: such pools are low-volume, so absolute alpha/capacity are modest.

`docs/execution-dry-run.md` starts the execution phase (Milestone 5): a
**Slipstream/v3-scoped** `dry-run-rebalance` planner that reads pool state, builds the
`collect→decreaseLiquidity→collect→swap→mint→stake` action sequence with
slippage-protected min amounts,
estimates gas, runs hard risk gates, and **never signs** — plus a clear account of how
Uniswap v2 / v3 / v4 differ for execution (the engine is a v3 model; v2/v4 need
separate adapters behind `EvmPoolKind`). The dry-run simulates the rebalance swap
against real in-range liquidity (`simulate_v3_swap`) and gates on price impact —
which **rejects $10k on the 200 bps fee-alpha pool (>30 bps impact; latest read was
~53 bps)**: execution makes the research's capacity caveat a hard, enforced limit
(alpha lives in thin pools, depth in zero-fee pools — capacity is a few $k).
`docs/live-readiness.md` is the current launch-readiness readout: the best researched
shape is high fee / low volatility with small capacity, WETH-USDC 200 bps passes
impact gates around $1k-$5k but not $10k, and the project is not ready for unattended
live trading. Read-only monitoring now emits USD exposure, risk-share alerts, and
kill-switch reasons, but shadow PnL, hedge integration, broader halt conditions, and
a manual guarded execution path are still required before real capital.
`docs/solana-pivot.md` opens the next research line: Base proved the execution
machinery but has a thin candidate surface, while Solana currently exposes many more
organic high-APY LP candidates. The new `solana-universe` command ranks Raydium/Orca/
Kamino candidates by organic base APY, TVL, volume, reward share, and concentrated
liquidity suitability, and can snapshot the ranked rows with `--output` for replay
and day-over-day validation. `solana-discover` is the stricter protocol-API layer:
it reads Orca Whirlpools, Raydium CLMM, and Meteora DLMM directly, normalizes fee
units/APR fields, applies an outlier cap, and writes the executable candidate rows
for replay/RPC enrichment.

## External References

- DeFiLlama yields UI: https://defillama.com/yields
- DeFiLlama API docs: https://api-docs.defillama.com/
- DeFiLlama yield server schema/methodology: https://github.com/DefiLlama/yield-server
