# Solana Pivot: Higher Flow, Better Candidate Surface

Snapshot: 2026-06-29 CST.

## Why Pivot

The Base/Aerodrome pilot proved the execution machinery, but it also exposed the
economics problem: the best researched Base pool had edge but limited capacity, while
the deeper pools had weak fee density. If the final strategy is only a few thousand
dollars of capacity at modest annualized return, it is not worth complex automation.

Solana is the right next research line because it has:

- more active retail/order-flow style DEX activity;
- lower transaction cost;
- more fragmented LP venues;
- more pools with organic base APY above 50%-100%;
- CLMM/DLMM-style venues where range management is the actual game.

This does **not** mean Solana is automatically better. It means the opportunity set is
large enough to justify building a Solana scanner before spending more time polishing
Base.

## First Data Entry

Added:

```bash
cargo run -p autopool-cli -- solana-universe \
  --min-tvl-usd 50000 \
  --min-volume-usd-1d 25000 \
  --min-base-apy 20 \
  --max-reward-share 0.25 \
  --concentrated-only \
  --limit 20
```

The command uses DeFiLlama yields as a first-pass filter and ranks by a deployability
score built from:

- organic `apyBase`;
- TVL;
- 1d volume;
- reward share;
- major-token vs long-tail inventory flag;
- fee tier parsed from pool metadata when available.

It is deliberately **not** a live-trading signal. It is a candidate generator.

## Current Conservative Scan

Concentrated-only, excluding DeFiLlama outliers:

| project | symbol | TVL | 1d volume | base APY | notes |
| --- | ---: | ---: | ---: | ---: | --- |
| Orca | JTO-JITOSOL | $1.58m | $1.27m | 87.4% | fee tier missing |
| Raydium | WSOL-CX | $654k | $816k | 113.8% | high turnover |
| Orca | UЅⅮT-USDT | $212k | $2.23m | 37.7% | suspicious symbol; validate |
| Raydium | WSOL-ARX | $115k | $1.28m | 40.7% | high turnover |
| Raydium | WSOL-TQQQ | $106k | $996k | 34.3% | high turnover |

This already looks more promising than Base if the data survives validation:
organic fee APR can be much higher, and several pools have enough daily volume to
support active monitoring.

## Aggressive Scan

Including DeFiLlama outliers and requiring at least one blue-chip leg surfaces:

| project | symbol | TVL | 1d volume | base APY | note |
| --- | ---: | ---: | ---: | ---: | --- |
| Orca | SOL-USDC | $23.7m | $80.4m | 49.6% | marked outlier; must verify |
| Raydium | CARDS-USDC | $3.28m | $3.63m | 161.2% | marked outlier |
| Raydium | WSOL-USDC | $5.21m | $17.6m | 49.2% | marked outlier |
| Orca | SOL-PUMP | $467k | $1.93m | 242.0% | marked outlier |
| Orca | JTO-JITOSOL | $1.58m | $1.27m | 87.4% | not marked outlier |

The outlier flag matters. These rows are exactly where the money might be, but also
where API anomalies, spoofed symbols, short-lived flow, or toxic inventory risk can
fool a strategy.

## Research Implication

The Base conclusion was: **high fee / low volatility / enough flow**.

The Solana search should be:

1. Start with **Orca Whirlpools** and **Raydium CLMM**.
2. Treat standard constant-product Raydium pools as discovery data only, not range
   execution targets.
3. Add Meteora DLMM once we have a reliable pool API.
4. Validate every candidate with independent pool stats:
   - fee tier;
   - tick/bin spacing;
   - TVL and active liquidity;
   - 24h/7d volume;
   - recent price path volatility;
   - range occupancy;
   - toxic flow / one-sided inventory risk.
5. Only then replay the same policy battery: static narrow, passive wide,
   adaptive/on-off, and delta hedge.

## Next Build

1. Add a Solana market-data crate with adapters:
   - DeFiLlama first-pass yields;
   - Orca Whirlpool pool stats;
   - Raydium CLMM pool stats;
   - Meteora DLMM pool stats.
2. Store Solana pool snapshots under `data/solana/...`.
3. Implement a Solana replay source from recent swaps / price candles.
4. Port the policy battery to non-EVM concentrated liquidity abstractions:
   tick/range for CLMM, bin/range for DLMM.
5. Only after replay edge is confirmed: build execution simulation.

Bottom line: Base remains the execution training ground. Solana is now the main
candidate-discovery frontier.
