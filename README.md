# AILP Research

Rust-first research system for **AI-assisted autonomous liquidity provision**: DEX pool allocation, range selection, rebalance timing, and LP inventory risk management.

AILP here means an AI-assisted / autonomous LP strategy system. The internal crate names still use `autopool-*` for now to avoid churn while the research loop is changing quickly.

The project is intentionally split into two loops:

- **Research loop**: ingest public yield data, chain data, price paths, and wallet state; run simulations; produce rebalance proposals.
- **Execution loop**: simulate transactions, enforce risk limits, submit private or public transactions, and record realized outcomes.

The current scaffold favors one validation market first: **Base / Aerodrome Slipstream**. This keeps the first research loop focused on strategy quality instead of multi-chain integration breadth. Other high-volume ecosystems can be added after the Base pilot has a credible net-PnL record.

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
cargo run -p autopool-cli -- pilot-universe --min-tvl-usd 100000 --min-volume-usd-1d 100000
BASE_RPC_URL=https://your-base-rpc.example cargo run -p autopool-cli -- sample-base-network --eth-usd 3500
BASE_RPC_URL=https://your-base-rpc.example cargo run -p autopool-cli -- resolve-slipstream-pools --limit 8
BASE_RPC_URL=https://your-base-rpc.example cargo run -p autopool-cli -- sample-slipstream-events --lookback-blocks 100 --log-chunk-blocks 10 --limit 4
```

## External References

- DeFiLlama yields UI: https://defillama.com/yields
- DeFiLlama API docs: https://api-docs.defillama.com/
- DeFiLlama yield server schema/methodology: https://github.com/DefiLlama/yield-server
