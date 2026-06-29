# Hot-Pool Autoresearch Protocol

Snapshot: 2026-06-29 CST.

## Thesis

The attractive target is not low-yield stable LP. The plausible edge is in hot,
high-fee, high-flow pools where a narrow or adaptive LP can earn dense fees. The
danger is that the same pools usually have high volatility, toxic flow, shallow
active bins/ticks, and one-sided inventory risk.

The working hypothesis is:

> Some hot pools can show gross fee APR above 2,000%, but sustainable deployable net
> APR will be much lower unless fee density beats LVR, rebalance churn, slippage,
> hedge cost, and downtime across repeated windows.

This is a hypothesis, not a belief. We keep it only if repeated experiments beat
baselines after costs.

## APR Sanity Check

Full-liquidity fee APR is approximately:

```text
fee_apr = daily_volume / tvl * fee_rate * 365
```

To reach 2,000% APR before LVR and costs:

| fee tier | required daily volume / TVL |
| ---: | ---: |
| 4 bps | 137.0x |
| 30 bps | 18.3x |
| 100 bps | 5.5x |
| 200 bps | 2.7x |

Concentrated liquidity can improve capital efficiency, but only while the position is
active. The missing multiplier is occupancy:

```text
net_edge = fee_apr * active_occupancy * capital_efficiency
         - LVR
         - rebalance_cost
         - swap_slippage
         - hedge_cost
         - downtime_cost
         - operational_risk_penalty
```

So a headline 2,000% pool is interesting only if the same narrow range stays active
long enough and does not accumulate the wrong token during the high-volume period.

## Current First-Pass Evidence

Protocol API scans already show the shape:

- Meteora DLMM frequently surfaces 500%-2,000%+ fee APR rows, but many carry
  `meteora_daily_ratio_disagrees_with_apy`, `fee_apr_outlier`, `high_turnover`, or
  long-tail inventory warnings.
- Cleaner Orca/Raydium CLMM rows are lower but more credible: recent scans surfaced
  pools like `SOL-PUMP`, `CARDS-USDC`, and `WSOL-CX` in the 100%-250%+ fee APR band.
- Major-pair pools such as SOL/USDC or WETH/USDC tend to be more executable but less
  explosive; long-tail pools are where the eye-catching APR lives, and where inventory
  risk is most likely to dominate.

Conclusion: the user's intuition is directionally right for **gross opportunity
discovery**, but 2,000%+ should be treated as an anomaly/long-tail candidate until
replay proves net edge.

## Candidate Queue Command

Use the hot-pool queue builder before spending time on replay:

```bash
cargo run -p autopool-cli -- hot-pool-candidates \
  --min-tvl-usd 50000 \
  --min-volume-usd-24h 25000 \
  --min-fee-apr 100 \
  --max-fee-apr 5000 \
  --target-fee-apr 2000 \
  --min-volume-tvl-24h 0.5 \
  --page-size 120 \
  --limit 30 \
  --output data/hot-pool/candidates/latest.json
```

The command ranks protocol-API candidates and assigns:

- `P1_replay_queue`: clean enough to replay from protocol/RPC data;
- `P1_verify_replay`: promising, but first freeze pool state or validate API fields;
- `P2_validate_api`: APR/token/API mismatch; do not replay until independently checked.

It also computes a formula sanity check:

```text
formula_fee_apr = volume_tvl_24h * fee_bps / 10_000 * 365 * 100
```

If reported APR is much larger than this formula, the row gets
`fee_apr_formula_mismatch` and is demoted to API validation. This already caught a
Meteora `JLP-USDC` row that reported ~985% APR even though 3 bps fee and ~0.88x
daily volume/TVL imply only about 9.6% fee APR from the simple formula.

The queue also carries price-risk warnings when protocol APIs expose them. For
Raydium CLMM, `wide_price_range_24h` is raised when day `priceMin/priceMax` imply
a 25%+ range. That does not discard the pool; it flags that any narrow strategy
must prove fee income beats LVR and rebalance risk.

Recent cleaner replay candidates:

| priority | venue | symbol | fee APR | volume/TVL | note |
| --- | --- | ---: | ---: | ---: | --- |
| P1 | Raydium | CARDS-USDC | ~222% | ~1.52x | clean protocol row |
| P1 | Orca | SOL-PUMP | ~264% | ~4.5x | high turnover |
| P1 | Raydium | WSOL-CX | ~122% | ~1.34x | clean protocol row |

These are not deployable yet; they are the next replay queue.

## Experiment Plan Command

Convert the candidate queue into replay work items:

```bash
cargo run -p autopool-cli -- hot-pool-experiment-plan \
  --input data/hot-pool/candidates/latest.json \
  --data-dir data/solana/hot-pool \
  --limit 12 \
  --output data/hot-pool/experiments/latest.json \
  --write-specs
```

The plan command does not create backtest results. It assigns each pool to a
replay model and blocks invalid transitions:

- `clmm_tick_replay`: Orca Whirlpool / Raydium CLMM; needs normalized `SwapObs`
  JSONL at the manifest path before replay can run.
- `dlmm_bin_replay`: Meteora DLMM; blocked until we implement bin-liquidity replay.
- `blocked_api_validation`: reported APR fails provider/fee-turnover sanity checks.

Once a CLMM decoder emits normalized swaps, run:

```bash
cargo run -p autopool-cli -- replay-normalized-swaps \
  --spec data/solana/hot-pool/specs/<experiment>.json \
  --swaps data/solana/hot-pool/swaps/<pool>/swaps.jsonl
```

Latest live scan result: clean CLMM rows are currently `Raydium CARDS-USDC`,
`Orca SOL-PUMP`, and `Raydium WSOL-CX`; all remain blocked on normalized swap
collection, not on strategy math. Meteora dominates the visible hot queue, but it
is DLMM/bin-based and must not be evaluated with the v3 tick engine.

## Solana Proxy Replay

Use this when the business question is "how much APR might this strategy make, and
how much risk is attached?" before tick-by-tick replay exists:

```bash
cargo run -p autopool-cli -- solana-proxy-replay \
  --min-tvl-usd 50000 \
  --min-volume-usd-24h 25000 \
  --min-fee-apr 100 \
  --max-fee-apr 5000 \
  --min-volume-tvl-24h 0.5 \
  --capital-usd 10000 \
  --output data/solana/proxy/latest.json
```

This is a protocol-API proxy, not a deployable backtest. It estimates:

- pool-wide fee APR;
- concentration from a chosen half-width versus the observed price range;
- daily rebalances needed to keep the range active;
- slippage/transaction churn cost;
- inventory drawdown proxy and risk grade.

Current CLMM proxy results:

| venue | pool | best half-width | net APR proxy | risk | interpretation |
| --- | --- | ---: | ---: | --- | --- |
| Raydium | CARDS-USDC | 2.5% | ~1000% | medium | strong candidate; needs real CLMM replay |
| Orca | SOL-PUMP | 2.5% | ~425% | low | cleaner but lower upside |
| Raydium | WSOL-CX | 10% | ~990% | severe | reject until replay; price range is extreme |

Meteora may show 1000%+ gross APR rows, but proxy risk is `unknown` until the DLMM
bin replay engine exists.

## Real Swap Sampling

Before decoding program events into normalized `SwapObs`, verify that public Solana
RPC can land recent successful pool swaps:

```bash
cargo run -p autopool-cli -- sample-solana-pool-swaps \
  --pool-address HnhpJPJgBG2KwniMTNW8cVBHvk1hFog3RC3kjnyc23tD \
  --program-id CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK \
  --token0-mint CARDSccUMFKoPRZxt5vt3ksUbxEFEcnZ3H2pd3dKxYjp \
  --token1-mint EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v \
  --signature-scan-limit 250 \
  --max-signature-pages 4 \
  --min-normalized-swaps 200 \
  --request-sleep-ms 250 \
  --output data/solana/swaps/raydium-cards-usdc-sample.json \
  --normalized-output data/solana/hot-pool/swaps/raydium-cards-usdc/swaps.jsonl
```

Replay the decoded Raydium rows:

```bash
cargo run -p autopool-cli -- replay-normalized-swaps \
  --spec data/solana/hot-pool/specs/raydium-cardsusdc-hnhpjpjg.json \
  --swaps data/solana/hot-pool/swaps/raydium-cards-usdc/swaps.jsonl
```

Replay rolling windows to test stability instead of trusting one short segment:

```bash
cargo run -p autopool-cli -- replay-normalized-windows \
  --spec data/solana/hot-pool/specs/raydium-cardsusdc-hnhpjpjg.json \
  --swaps data/solana/hot-pool/swaps/raydium-cards-usdc/swaps.jsonl \
  --window-swaps 25 \
  --step-swaps 10 \
  --min-windows 4
```

Sweep fixed hedge fractions for the hedged narrow policy:

```bash
cargo run -p autopool-cli -- replay-normalized-hedge-grid \
  --spec data/solana/hot-pool/specs/raydium-cardsusdc-hnhpjpjg.json \
  --swaps data/solana/hot-pool/swaps/raydium-cards-usdc/swaps.jsonl \
  --window-swaps 40 \
  --step-swaps 15 \
  --min-windows 3 \
  --grid-hedge-fraction 0 \
  --grid-hedge-fraction 0.25 \
  --grid-hedge-fraction 0.5 \
  --grid-hedge-fraction 0.75 \
  --grid-hedge-fraction 1
```

```bash
cargo run -p autopool-cli -- sample-solana-pool-swaps \
  --pool-address BofA2ViUSudPBTUms2KRuG6AHNeMawjNfwqTJDgx5BKW \
  --program-id whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc \
  --token0-mint So11111111111111111111111111111111111111112 \
  --token1-mint pumpCmXqMfrsAkQ5r49WcJnRayYRqmXz6ae8H7H9Dfn \
  --limit 8 \
  --signature-scan-limit 50 \
  --output data/solana/swaps/orca-sol-pump-sample.json
```

This command extracts:

- successful `Swap` / `SwapV2` transactions for the target program;
- target-program `Program data` payloads;
- signed pool-owned token vault deltas for both pool mints.
- Raydium CLMM `SwapEvent` fields and a normalized `SwapObs` preview when the
  program is `CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK`.
- paginated signature scans via `--max-signature-pages` and
  `--before-signature`, plus `--min-normalized-swaps` so replay collection can
  target decoded rows rather than raw transaction count.

Current result: both Raydium `CARDS-USDC` and Orca `SOL-PUMP` produce clean recent
swap samples with one program-data payload per swap. Raydium CLMM now decodes into
post-swap sqrt price, tick, active liquidity, and signed amount preview; the next
step is collecting a larger multi-window JSONL sample and comparing
`replay-normalized-swaps` results against proxy APR and passive baselines. Orca needs
a separate liquidity reconstruction step before it can be replayed with comparable
precision.

First smoke test: scanning 80 recent `CARDS-USDC` pool signatures landed 20 swaps,
decoded 19 Raydium `SwapEvent` rows into `SwapObs` JSONL, and `replay-normalized-swaps`
successfully ran the existing policy battery over those rows. The missing row was a
routed Jupiter transaction where the first Raydium invocation belonged to another
pool; the sampler now scans all Raydium swap invocations and keeps the event whose
`pool_state` matches the requested pool.

Latest real replay check: after adding a 20 second Solana HTTP timeout so public-RPC
sampling stays bounded, scanning 185 recent `CARDS-USDC` signatures across 4 pages
landed 77 target pool swaps, decoded 77/77 normalized rows with zero transaction
errors, and replayed a ~46.5 minute wall-clock segment (~45.9 replay minutes at the
current 0.4 second slot model). Mechanical annualization is not a forecast, but it
gives the current hot-pool magnitude:

| policy | net PnL | vs hold | fees | fee-LVR | net APR window | fee-LVR APR window | max DD |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| vol_scaled / adaptive | $78.22 | $16.62 | $42.10 | $23.08 | ~8966% | ~2645% | $1.82 |
| narrow_static | $70.86 | $9.27 | $22.05 | $12.51 | ~8123% | ~1434% | $11.32 |
| delta_hedged | $13.19 | -$48.41 | $22.05 | $12.51 | ~1512% | ~1434% | $3.07 |
| hedged_wide | $1.10 | -$60.50 | $2.45 | $1.43 | ~126% | ~164% | $0.79 |

Interpretation: the fee engine is still hot enough to justify continued work, but this
segment was also strongly directionally favorable to unhedged inventory. The better
headline PnL versus the prior 50-row sample does **not** mean the strategy got safer;
it means this specific window carried upside beta while staying in range.

Latest larger-window check: the same 77-row file was replayed as rolling 40-swap
windows with 15-swap steps, producing 3 windows. This is a bounded replay-quality
step versus the earlier 25-swap windows because it tests longer occupancy and reduces
the chance that one burst of routed retail flow dominates the result:

| policy | win rate vs hold | mean net | mean net APR window | p05 net APR window | mean fee-LVR APR window | worst DD |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| vol_scaled_rebalance | 66.7% | $26.16 | ~10464% | ~656% | ~3249% | $17.85 |
| adaptive_regime | 66.7% | $26.16 | ~10464% | ~656% | ~3249% | $17.85 |
| delta_hedged | 66.7% | $6.13 | ~1479% | ~1060% | ~1754% | $3.07 |
| hedged_narrow | 66.7% | $4.91 | ~885% | ~-543% | ~1754% | $4.68 |
| hedged_wide | 66.7% | $0.57 | ~108% | ~-36% | ~200% | $0.48 |

Interpretation: this is better than the prior 25-swap study because the left tail is
less absurd, but it is still not promotion-grade evidence. The sample only produced 3
windows, most policies never had to rebalance, and the same directional move helped
all inventory-long variants. Fixed hedging keeps drawdown small and now has a positive
left tail for `delta_hedged`, but it still lags hold on the full 77-row segment.
Unhedged/adaptive variants look economically attractive only if later windows confirm
the result outside this single short regime.

Latest hedge-grid check on the same 77-row sample:

| hedge fraction | policy | win rate vs hold | mean net | mean vs hold | mean APR window | p05 APR window | worst DD |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 0.75 | hedged_narrow | 66.7% | $9.21 | -$8.00 | ~3126% | ~1074% | $3.48 |
| dynamic | delta_hedged | 66.7% | $6.13 | -$11.07 | ~1479% | ~1060% | $3.07 |
| 0.50 | hedged_narrow | 66.7% | $13.50 | -$3.70 | ~5367% | ~408% | $8.82 |
| 0.25 | hedged_narrow | 66.7% | $17.79 | $0.59 | ~7608% | ~-258% | $14.16 |
| 0.00 | hedged_narrow | 66.7% | $22.08 | $4.88 | ~9849% | ~-924% | $19.49 |

Interpretation: the best left-tail/drawdown point in this short sample is around
`0.75` fixed hedge, while lower hedge fractions harvest more upside beta but degrade
p05 APR and drawdown. `delta_hedged` is a dynamic control and does not use the fixed
`hedge_fraction` parameter. This pushes the next research step toward regime-aware
hedge sizing, not a static all-or-nothing hedge.

## Autoresearch Rules Adapted To AILP

Inspired by `karpathy/autoresearch`, every strategy idea must use the same loop:

1. Keep a fixed evaluation budget.
2. Establish baselines first.
3. Change one strategy idea at a time.
4. Run the same replay/shadow harness.
5. Log the result.
6. Keep only improvements that clear complexity and risk costs.
7. Discard crashes, overfits, and data-source anomalies.

The metric is not headline APR. The primary metric is:

```text
risk_adjusted_net_usd = net_pnl_usd
                      - tail_loss_penalty
                      - max_drawdown_penalty
                      - execution_failure_penalty
                      - complexity_penalty
```

Secondary metrics:

- fee minus LVR;
- net PnL versus hold;
- net PnL versus passive wide LP;
- max drawdown;
- rebalance count;
- time in range;
- risk-token exposure share;
- capacity at 10/30/100 bps impact;
- result stability across at least 3-7 daily windows.

## Baselines

Every hot-pool experiment must beat:

1. Hold 50/50 inventory.
2. Passive wide LP.
3. Static narrow, no rebalance.
4. Mechanical recenter-on-exit.
5. Adaptive range with regime gate.
6. Hedged wide / delta-hedged variant when hedge data is available.

If a strategy only wins versus a bad churn baseline, it is not a strategy.

## Experiment Unit

Each experiment is one candidate pool plus one policy change:

```text
pool: venue + address
window: fixed 24h, 3d, or 7d block/time range
capital: fixed size, e.g. $1k / $3k / $10k
policy: exact range width, trigger, hedge, stop, and rebalance rules
costs: gas, slippage, price impact, hedge funding, failed tx buffer
metric: risk_adjusted_net_usd
```

A result is invalid if:

- the pool API has outlier flags and no independent confirmation;
- volume is mostly one burst that disappears in the replay window;
- the strategy changes multiple policy knobs at once;
- capital size silently changes;
- active liquidity/tick/bin distribution is missing;
- the improvement depends on unrealistic instant rebalance.

## Results Log

Use an untracked TSV, e.g. `research/hot-pool/results.tsv`:

```text
commit	pool	window	capital_usd	policy	net_usd	fee_minus_lvr_usd	max_dd_usd	rebalances	time_in_range_pct	status	description
```

Statuses:

- `keep`: beats all required baselines after costs and complexity.
- `discard`: worse, unstable, or complexity not justified.
- `crash`: data/logic/runtime failure.
- `needs_validation`: promising but missing pool-state or replay confirmation.

Do not treat a single `keep` as deployable. Promotion requires repeated keeps across
multiple windows and liquidity regimes.

## Promotion Gates

A hot-pool strategy can advance to shadow monitoring only if:

- net PnL beats hold and passive wide in at least 70% of replay windows;
- fee minus LVR is positive after realistic slippage;
- rebalance count is low enough that execution cost does not dominate;
- active range occupancy is high or the off/on gating is explainable;
- max risk-token share stays below configured limits;
- capacity is known at the intended capital size;
- no single data provider is the sole source of APR truth.

It can advance to tiny guarded live only if, in addition:

- shadow monitor produces clean exposure and kill-switch fields;
- funded fork simulation passes for the same action path;
- wallet balance and post-trade accounting are live;
- hedge adapter is ready when the policy depends on hedge;
- operator halt conditions are explicit.

## Near-Term Research Queue

1. Add replay ingestion for Solana hot pools:
   - Orca Whirlpool swaps/tick arrays;
   - Raydium CLMM swaps/tick arrays;
   - Meteora DLMM swaps/bin liquidity.
2. For each top pool, store a daily frozen dataset and replay all baselines.
3. Add `time_in_range_pct`, `fee_minus_lvr`, and capacity-at-impact metrics to every
   hot-pool report.
4. Test one policy idea at a time:
   - no-rebalance tight static;
   - volatility-scaled width;
   - off/on regime gate;
   - inventory-aware recenter;
   - delta hedge.
5. Promote only policies that beat passive wide and hold across multiple windows.

Bottom line: pursue hot pools aggressively, but assume headline 2,000% APR is a
debugging signal until replay, capacity, and shadow monitoring prove it is real.
