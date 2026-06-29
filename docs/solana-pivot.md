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
  --limit 20 \
  --output data/solana/universe/latest.json
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
The optional `--output` flag writes the limited ranked rows as JSON so later replay,
stability checks, and execution adapters consume the same candidate set.

The protocol-owned API pass is:

```bash
cargo run -p autopool-cli -- solana-discover \
  --min-tvl-usd 50000 \
  --min-volume-usd-24h 25000 \
  --min-fee-apr 20 \
  --max-fee-apr 1000 \
  --page-size 100 \
  --limit 30 \
  --output data/solana/discovery/latest.json
```

This reads Orca Whirlpools, Raydium CLMM, and Meteora DLMM directly. It normalizes
fee units, tick/bin spacing, 24h volume, fee APR, verification flags, and warnings.
The default `--max-fee-apr 1000` is an anomaly guard, not a strategy target; pools
above that are usually short-lived, toxic, or API-unit edge cases until proven
otherwise.

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

## Current Protocol Scan

Orca/Raydium-only, capped to 20%-300% fee APR:

| venue | symbol | TVL | 24h volume | fee APR | spacing | notes |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Orca | SOL-USDC | $23.5m | $84.6m | 52.5% | tick 4 | high turnover |
| Raydium | CARDS-USDC | $3.30m | $3.78m | 167.0% | tick 60 | tail inventory |
| Raydium | WSOL-USDC | $5.20m | $18.4m | 51.6% | tick 1 | high turnover |
| Orca | SOL-PUMP | $482k | $1.93m | 234.8% | tick 16 | high turnover |
| Orca | JTO-JitoSOL | $1.56m | $1.28m | 89.4% | tick 64 | correlated inventory |
| Raydium | WSOL-CX | $651k | $866k | 121.4% | tick 60 | tail inventory |

This is the first scan that looks strategically relevant: it has pools with real
fee density, enough TVL to deploy small test capital, and protocol-level parameters.
It is still not a live signal because we do not yet know active-liquidity distribution,
range occupancy, swap toxicity, or whether the APR survives across multiple days.

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

1. Extend the Solana market-data crate:
   - keep DeFiLlama as first-pass yields;
   - keep Orca/Raydium/Meteora protocol APIs as discovery;
   - add pool-specific RPC/SDK enrichment for the top N rows.
2. Store Solana pool snapshots under `data/solana/...`.
3. Enrich top pools from RPC/SDK accounts:
   - active tick/bin;
   - tick-array or bin liquidity distribution;
   - vault balances;
   - reward emissions;
   - observation/candle data.
4. Implement a Solana replay source from recent swaps / price candles.
5. Port the policy battery to non-EVM concentrated liquidity abstractions:
   tick/range for CLMM, bin/range for DLMM.
6. Only after replay edge is confirmed: build execution simulation.

Current implementation boundary:

- `hot-pool-experiment-plan` writes replay specs and blocks pools without the right
  adapter.
- `solana-proxy-replay` runs the first Solana business-flow estimate from protocol
  pool stats: range-width assumption, fee capture, churn cost, net APR proxy, and
  risk grade.
- Orca Whirlpool and Raydium CLMM should emit normalized `SwapObs` JSONL first;
  this can reuse the existing tick/range replay engine.
- Meteora DLMM needs a bin replay engine and should stay out of `SwapObs`/v3 math
  until bin liquidity and swap accounting are modeled.

Bottom line: Base remains the execution training ground. Solana is now the main
candidate-discovery frontier.
