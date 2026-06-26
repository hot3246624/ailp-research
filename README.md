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
```

The `replay-events` command turns collected swap events into LP profit-and-loss
for a battery of baseline range policies (hold, passive-wide, narrow-static,
narrow-rebalance, vol-scaled) with PnL attribution into fees, inventory IL, gas,
and slippage. It is the first strategy-research environment (architecture
Milestone 3).

See `docs/first-data-analysis.md` for the first Base / Aerodrome event-readout, and
`docs/replay-weth-aero.md` for the first range-policy replay results (fee density
dominates a calm window; rebalancing-on-exit is a tax) and the discovery of the
real active WETH-USDC pool.

## External References

- DeFiLlama yields UI: https://defillama.com/yields
- DeFiLlama API docs: https://api-docs.defillama.com/
- DeFiLlama yield server schema/methodology: https://github.com/DefiLlama/yield-server
