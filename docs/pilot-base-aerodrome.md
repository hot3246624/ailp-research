# Base / Aerodrome Pilot

## Decision

Validate the strategy on Base using Aerodrome Slipstream as the first venue.

This is not because Aerodrome is the only interesting DEX. It is because a low-cost, high-volume EVM venue is the fastest way to test whether active LP range management has real edge after transaction costs and inventory marking.

## Why This Pilot

- **Network**: Base has low execution cost relative to Ethereum mainnet, so rebalancing policies can be tested without gas dominating every decision.
- **Venue**: Aerodrome is the leading Base liquidity venue, and Slipstream is its concentrated-liquidity layer.
- **Mechanism**: Slipstream concentrated pools require liquidity ranges and tick spacing, so the strategy must solve the actual range-management problem.
- **Scope control**: Solana, Ethereum, BSC, and Hyperliquid may dominate broad DEX volume, but supporting them all before proving strategy quality would hide the real research question.

## Data To Collect

Pool data:

- pool address
- token0/token1
- tick spacing
- fee tier
- current tick and sqrt price
- active liquidity
- tick liquidity distribution around current price
- swap events
- fee growth

Wallet data:

- open positions
- range lower/upper ticks
- liquidity
- token balances
- uncollected fees
- realized collect/burn/mint/swap history

Network data:

- Base gas cost by action type
- block time and latency
- RPC disagreement
- transaction failure rate
- private route availability
- estimated sandwich/MEV loss

External discovery data:

- DeFiLlama Base DEX volume ranking
- DeFiLlama Aerodrome yield rows
- Aerodrome pool metadata and emissions

## First Pool Universe

Start with Aerodrome Slipstream pools only:

- minimum TVL
- minimum 24h volume
- non-outlier yield row
- avoid pools where APY is mostly emissions unless reward liquidation is modeled
- prioritize majors/stables before long-tail meme pairs

Candidate types:

- stable-stable: lower price volatility, lower IL, more tick precision
- ETH/stable: high volume, meaningful directional inventory risk
- AERO/stable or AERO/ETH: high native incentive relevance, but higher token-specific risk

## Strategy Hypotheses

### H1: Gas-Aware Tight Ranges Can Work On Base

Because Base gas is relatively cheap, a tighter range may outperform a passive wide range when fee density is high and price occupancy is stable.

Reject this hypothesis if gas plus slippage consumes the fee edge or if out-of-range time is too high.

### H2: Fee Density Beats Headline APY

The best candidate is not necessarily the highest APY pool. It is the pool with the best expected fee per unit of inventory risk and rebalance cost.

Reject this hypothesis if fee-density ranking does not beat simple TVL/volume/APY ranking.

### H3: Reward-Heavy Pools Need Separate Accounting

Aerodrome emissions can make APY look attractive, but rewards only count if liquidation cost and token risk are modeled.

Reject reward-heavy pools from the first strategy unless reward liquidation is part of the replay.

## Baselines

Every policy must beat:

- hold token inventory
- passive wide range
- fixed-width range with naive out-of-range rebalance
- volatility-scaled range with gas threshold

Performance is measured after gas, swap cost, slippage, inventory mark-to-market, and reward liquidation assumptions.

## Acceptance Criteria

Do not move to live execution until a dry-run strategy has:

- positive net PnL against hold and passive LP baselines
- PnL attribution showing fees are the main edge, not accidental token beta
- stable performance across at least high-volatility and low-volatility windows
- bounded one-sided inventory exposure
- realistic rebalance failure and latency assumptions

## Implementation Order

1. Scan Aerodrome Slipstream pools from public sources.
2. Select a small pool universe: stable-stable, ETH/stable, and one AERO pair.
3. Build Base network-regime recorder.
4. Build Slipstream state reader.
5. Reconstruct swap/tick/fee history.
6. Replay baseline policies.
7. Add fee-density and gas-aware policies.
8. Run dry-run proposals without signing.

## Current Research Commands

Build the first pool universe:

```bash
cargo run -p autopool-cli -- pilot-universe \
  --min-tvl-usd 100000 \
  --min-volume-usd-1d 100000 \
  --max-reward-share 0.5
```

Use JSON when handing candidates to downstream replay tooling:

```bash
cargo run -p autopool-cli -- pilot-universe \
  --min-tvl-usd 100000 \
  --min-volume-usd-1d 100000 \
  --max-reward-share 0.5 \
  --format json
```

Sample the Base network regime from an RPC endpoint:

```bash
BASE_RPC_URL=https://your-base-rpc.example \
cargo run -p autopool-cli -- sample-base-network \
  --rebalance-gas-units 900000 \
  --eth-usd 3500
```

Resolve candidate pools to on-chain Slipstream pools and read current pool state:

```bash
BASE_RPC_URL=https://your-base-rpc.example \
cargo run -p autopool-cli -- resolve-slipstream-pools \
  --min-tvl-usd 100000 \
  --min-volume-usd-1d 100000 \
  --max-reward-share 0.5 \
  --limit 8
```

Sample recent `Swap/Mint/Burn/Collect` events:

```bash
BASE_RPC_URL=https://your-base-rpc.example \
cargo run -p autopool-cli -- sample-slipstream-events \
  --lookback-blocks 100 \
  --log-chunk-blocks 10 \
  --min-tvl-usd 100000 \
  --min-volume-usd-1d 100000 \
  --max-reward-share 0.5 \
  --limit 4
```

The provided Alchemy free-tier endpoint currently limits `eth_getLogs` to 10 blocks per request, so `--log-chunk-blocks 10` is required for longer windows. Larger historical backfills should use a paid RPC tier, an archive/indexer endpoint, or a dedicated event ingestion job with rate limiting and checkpointing.

## Official Protocol Constants

The pilot uses the current latest Aerodrome Slipstream Gauges V3 deployment from the official `aerodrome-finance/slipstream` repository:

- PoolFactory: `0xf8f2eB4940CFE7d13603DDDD87f123820Fc061Ef`
- NonfungiblePositionManager: `0xe1f8cd9AC4e4A65F54f38a5CdAfCA44f6dD68b53`
- SwapRouter: `0x698Cb2b6dd822994581fEa6eA4Fc755d1363A92F`

For pool lookup, the resolver tries Gauges V3 first, then Gauge Caps, then the Initial deployment. Current high-volume DeFiLlama candidates resolve through the Initial factory, so the fallback order is required.
