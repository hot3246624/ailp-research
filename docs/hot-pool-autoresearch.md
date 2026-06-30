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
  --active-liquidity 267836504483179 \
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
- Orca Whirlpool `Traded` event fields and a normalized `SwapObs` preview when the
  program is `whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc`. Orca events do not emit
  per-swap active liquidity, so the first adapter pass uses `--active-liquidity`
  from the discovery/spec snapshot until historical account-state snapshots are added.
- paginated signature scans via `--max-signature-pages` and
  `--before-signature`, plus `--min-normalized-swaps` so replay collection can
  target decoded rows rather than raw transaction count.

Current result: both Raydium `CARDS-USDC` and Orca `SOL-PUMP` produce clean recent
swap samples with one program-data payload per swap. Raydium CLMM now decodes into
post-swap sqrt price, tick, active liquidity, and signed amount preview. Orca
Whirlpool now decodes `Traded` into post-swap sqrt price, inferred tick, signed
amounts, and a snapshot-liquidity normalized preview. Orca still needs historical
active-liquidity reconstruction before its fee-share estimates have Raydium-level
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

The hedge-grid report now also prints a `by regime` breakdown. On the same sample:

| regime | best observed hedge readout |
| --- | --- |
| `range` | higher hedge fractions kept p05 APR positive and reduced drawdown; full hedge had the strongest range-window left tail in this short sample |
| `trend_down_money` | higher fixed hedge lagged hold badly because the directional move favored inventory; lower hedge kept more beta but with weaker tail control |

Interpretation: the 0.75 hedge result is a compromise across a tiny regime mix, not
a universal hedge setting. The deployable path is a regime-conditioned hedge rule:
more hedge in range/noisy windows, less static hedge when the window is moving to
the money side, and separate crash tests for risk-side trends. The `trend_*` labels
assume the normalized replay convention where the stable/numeraire leg is token0 and
the risk leg is token1.

The hedge-grid command now also evaluates a no-lookahead `lagged_regime_rule`: skip
the first window, use the prior window's regime label to choose the current window's
fixed hedge fraction, and then summarize the selected windows. Default rule map:
`range=1.00`, `volatile=1.00`, `money_trend=0.25`, `risk_trend=1.00`.

The first 77-row sample was too small and favored a 0.75 range hedge in lagged
40-swap windows. A fresh 80-row public-RPC refresh then flipped the range result
toward 1.00. The sampler now prints progress every 50 scanned signatures and ends
with a `next_before_signature` cursor; `merge-normalized-swaps` dedupes overlapping
JSONL files and rewrites a stable sorted replay stream. Merging the original sample,
the fresh refresh, and one older paginated refresh produced 231 input rows, 171
unique normalized swaps, and 60 duplicates across overlapping samples.

Next cursor extensions added 28 then 19 normalized swaps and extended the span to
block/slot `429588129..429604170`. The five-file merge has 278 input rows, 201
unique normalized swaps, and 77 duplicates. This mostly extends the segment backward
a little and adds density around the existing regime; hit rate fell sharply in the
latest older page, so the CARDS burst appears bounded.

Current best read on the 201-row merged segment:

| combined sample | lagged rule map | windows | win vs hold | mean vs hold | p05 APR | worst DD |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 25 swaps / 10 step | range=1.00, volatile=1.00 | 17 | 59% | -$2.80 | ~660% | $9.99 |
| 40 swaps / 15 step | range=1.00, volatile=1.00 | 10 | 50% | -$4.16 | ~-1162% | $13.31 |
| 60 swaps / 20 step | range=1.00, volatile=1.00 | 7 | 57% | +$3.97 | ~-21% | $15.23 |
| 80 swaps / 25 step | range=1.00, volatile=1.00 | 4 | 75% | -$7.00 | ~1216% | $18.26 |

Interpretation: the conservative 1.00 range/volatile hedge is now the better default
for the lagged rule, but the evidence is still small, uses overlapping windows, and
mostly densifies one short wall-clock regime rather than extending far back in time.
The 25-swap view keeps positive p05 APR but loses to hold on average; the 40- and
60-swap views show negative left tails; the 80-swap view has too few windows and
still loses to hold on average. Treat the rule as a candidate state machine, not a
deployable strategy. The next bottleneck remains broader pool coverage or a true
shadow monitor, not more in-sample CARDS-only rule tweaking.

### Promotion gate

The replay stack now has a strict deployability gate:

```bash
cargo run -p autopool-cli -- replay-promotion-gate \
  --spec data/solana/hot-pool/specs/raydium-cardsusdc-hnhpjpjg.json \
  --swaps data/solana/hot-pool/swaps/raydium-cards-usdc/swaps-merged-5.jsonl \
  --min-p05-net-apr-pct 500 \
  --min-mean-vs-hold-usd 0 \
  --min-win-rate-vs-hold-pct 60 \
  --max-drawdown-pct 0.05
```

Default windows are 25:10, 40:15, 60:20, and 80:25 swaps. The default gate policy is
`lagged-regime-rule`; the command can also gate defensive controls directly with
`--gate-policy hedged-wide`, `--gate-policy delta-hedged`, or `--gate-policy
delta-trend-stop`. `delta-trend-stop` is a narrow dynamic-delta LP that stands down to
the money leg when a short intra-window trend signal fires. `--gate-policy
lagged-policy-switch` evaluates a no-lookahead regime switch that chooses the current
window's policy from the prior window's regime. The default switch is
`range=delta_hedged`, `volatile=hedged_wide`, `money_trend=hedged_wide`, and
`risk_trend=hedged_wide`; the four `--rule-*-policy` flags can override that map.
`--gate-policy lagged-policy-blend` evaluates a smoother two-sleeve allocation between
`delta_hedged` and `hedged_wide`, again using only the prior window's regime. The
`--rule-*-wide-fraction` flags set the capital share allocated to `hedged_wide`.
A candidate only promotes to `candidate_shadow` when every window family clears
win-rate, mean edge over hold, left-tail net APR, and drawdown gates. On the 201-row
CARDS-USDC merged replay, the current lagged regime rule returns `reject_replay`:
short windows nearly work on left-tail APR but lose to hold on average, while longer
windows expose unstable left tails. This is the intended behavior for the current
research stage: headline APR is a detector, not a deployment claim.

Latest scout/proxy read: the active hot-pool surface has shifted toward Meteora DLMM
and Orca Whirlpool candidates. Meteora proxy APRs can be much higher than the current
Raydium set, but they are not actionable until DLMM bin/liquidity replay exists. Orca
`SOL-PUMP` is now replayable through the Whirlpool event adapter, while Raydium is no
longer the bottleneck for process quality; it is mainly a solved adapter path with no
currently promoted hot pool.

### Orca SOL-PUMP Replay

The first Whirlpool normalized replay used two public-RPC pages:

```text
sample A: scanned 408 signatures, kept 80 normalized swaps, tx_errors=0
sample B: scanned 303 signatures, kept 80 normalized swaps, tx_errors=0
merged:   160 unique swaps, slot span 429614430..429620972, tick span 39003..39161
```

The replay uses `SOL` as token0 and a spot USD anchor near `$72.28`; this is enough
for research PnL marking but not a trading oracle. With `narrow_half_width=100`,
`wide_half_width=2500`, `$10k` capital, and the snapshot active liquidity from the
Orca discovery row, the full merged replay shows the real shape of the opportunity:

| policy | net PnL | vs hold | fees | fee-LVR | net APR window | max DD |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| hold_50_50 | -$76.33 | $0.00 | $0.00 | $0.00 | ~-9199% | $78.29 |
| narrow_rebalance | -$98.55 | -$22.22 | $16.76 | $6.43 | ~-11877% | $100.75 |
| hedged_narrow | -$21.74 | +$54.60 | $16.76 | $6.43 | ~-2619% | $27.63 |
| delta_hedged | -$6.02 | +$70.31 | $16.76 | $6.43 | ~-726% | $12.99 |
| hedged_wide | -$1.54 | +$74.79 | $0.96 | $0.52 | ~-186% | $1.63 |

Interpretation: the hedge layer clearly reduces directional damage versus hold, but
this is not yet a positive-fee-alpha strategy. The pool moved through a sharp
directional segment; pure narrow LP collected fees but was dominated by inventory
loss. `delta_hedged` and `hedged_wide` are useful risk controls, not evidence of a
500%+ deployable APR.

Promotion gate on the same 160-row replay returns `reject_replay`:

| window | lagged windows | win vs hold | mean vs hold | p05 APR | worst DD | result |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| 20 swaps / 10 step | 14 | 93% | +$10.50 | ~-20431% | $5.42 | fail left tail |
| 40 swaps / 15 step | 8 | 100% | +$19.01 | ~-38415% | $13.71 | fail left tail |
| 60 swaps / 20 step | 5 | 100% | +$24.69 | ~-12693% | $25.95 | fail left tail |

This result is valuable because it upgrades Orca from proxy APR to replay-based
rejection. Next useful work is not more tuning on this exact 43-minute segment; it is
either more Whirlpool coverage across different pools/regimes or historical
liquidity reconstruction to remove the snapshot-liquidity approximation.

### Orca SOL-GRASS Replay

Refreshing the hot-pool queue promoted `SOL-GRASS` as the cleanest current Orca P0
candidate: about `314%` 24h fee APR, `30bps` fee tier, about `$72k` TVL, about
`$207k` 24h volume, and no discovery warnings. The Whirlpool adapter collected two
public-RPC pages cleanly:

```text
sample A: scanned 86 signatures, kept 80 normalized swaps, tx_errors=0
sample B: scanned 85 signatures, kept 80 normalized swaps, tx_errors=0
merged:   160 unique swaps, slot span 429623681..429644467, tick span 49488..49876
```

With `narrow_half_width=384`, `wide_half_width=2500`, `$10k` capital, SOL marked near
`$72.93`, and snapshot active liquidity, the full merged replay was again risk-control
evidence rather than a deployable strategy:

| policy | net PnL | vs hold | fees | fee-LVR | net APR window | max DD |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| hold_50_50 | -$159.08 | $0.00 | $0.00 | $0.00 | ~-6034% | $190.02 |
| narrow_rebalance | -$247.37 | -$88.29 | $13.56 | $8.51 | ~-9383% | $278.42 |
| hedged_narrow | -$88.16 | +$70.92 | $13.56 | $8.51 | ~-3344% | $89.63 |
| delta_hedged | -$21.39 | +$137.69 | $13.56 | $8.51 | ~-811% | $22.91 |
| hedged_wide | -$7.61 | +$151.47 | $3.33 | $2.51 | ~-289% | $12.65 |

Rolling 20-swap windows gave `hedged_wide` mean net about `+$0.12`, p05 APR about
`-101%`, and worst drawdown about `$0.52`. The lagged promotion gate returned
`reject_replay`: 20/40/60-swap windows all had positive mean-vs-hold but p05 APR
around `-1399%`, `-1238%`, and `-1486%`.

Interpretation: `SOL-GRASS` is more active and cleaner than `SOL-PUMP`, but the same
strategic issue remains. Hedging reduces directional loss dramatically; it does not
yet turn Whirlpool hot-pool LP into stable positive APR. Current strategy status is
not fixed/deployable. The only defensible shape today is:

```text
hot-pool detector -> normalized replay -> promotion gate -> reject or shadow
```

The deployable strategy still needs either better pool selection, historical
liquidity reconstruction, or a different range/hedge rule that clears left-tail APR
instead of merely improving loss versus hold.

### Orca SOL-CARDS Replay

A later hot-pool refresh moved Orca `SOL-CARDS` into the P0 slot: about `642%` 24h
fee APR, `100bps` fee tier, about `$62k` TVL, about `$109k` 24h volume, and no
discovery warnings. The sample was thinner than `SOL-GRASS`, but still landed enough
normalized rows for a first replay:

```text
sample A: scanned 770 signatures, kept 65 normalized swaps, tx_errors=0
span:     slot 429658908..429669695, tick -11468..-10842
```

With SOL marked near `$73.33`, `narrow_half_width=384`, and snapshot active liquidity,
the full 65-row replay again showed strong hedge protection but no deployable edge:

| policy | net PnL | vs hold | fees | fee-LVR | net APR window | max DD |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| hold_50_50 | -$303.18 | $0.00 | $0.00 | $0.00 | ~-22159% | $303.18 |
| narrow_rebalance | -$402.45 | -$99.28 | $49.55 | $36.14 | ~-29415% | $404.85 |
| hedged_narrow | -$98.71 | +$204.47 | $49.55 | $36.14 | ~-7215% | $103.97 |
| delta_hedged | -$3.11 | +$300.06 | $49.55 | $36.14 | ~-228% | $12.01 |
| hedged_wide | -$22.34 | +$280.84 | $14.05 | $11.64 | ~-1632% | $24.97 |

The only encouraging slice was defensive: with 15-swap windows, `hedged_wide` had
mean net about `+$1.20`, mean mechanical APR about `653%`, p05 APR about `106%`, and
worst drawdown about `$1.88`. That is not enough: larger 25-swap windows flipped
negative. The promotion gate can now evaluate defensive policies directly; on this
sample `--gate-policy hedged-wide` still returned `reject_replay` with p05 APR about
`106%`, `-545%`, and `-1665%` across 15/25/40-swap families. `--gate-policy
delta-hedged` also rejected, with p05 APR about `-802%`, `-2214%`, and `-574%`.

Interpretation: this is the first Whirlpool sample that produced a positive
short-window defensive APR read, but it is still not a strategy. It is a reason to
collect more `SOL-CARDS` data and keep gating `hedged_wide`/defensive policies
directly, not a reason to promote the pool.

### Orca SOL-ORCA Replay

The next Whirlpool coverage pass refreshed the Orca queue and selected `SOL-ORCA`
because it had better heat than the already rejected SOL-PUMP/SOL-GRASS samples while
still being replayable: about `201%` 24h fee APR, `16bps` fee tier, about `$763k` TVL,
about `$2.6m` 24h volume, and volume/TVL around `3.45`. Sampling quality was high:

```text
sample A: scanned 106 signatures, kept 100 normalized swaps, tx_errors=0
sample B: scanned 103 signatures, kept 100 normalized swaps, tx_errors=0
merged:   200 unique swaps, slot span 429673585..429678365, tick span -28026..-27957
```

With SOL marked near `$73.38`, `narrow_half_width=384`, and snapshot active liquidity,
the full 200-row replay looked superficially good but did not survive rolling-window
gating:

| policy | net PnL | vs hold | fees | fee-LVR | net APR window | max DD |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| hold_50_50 | +$1.72 | $0.00 | $0.00 | $0.00 | ~283% | $29.28 |
| narrow_rebalance | +$5.47 | +$3.76 | $3.76 | $2.11 | ~903% | $25.79 |
| vol_scaled_rebalance | +$8.86 | +$7.15 | $7.16 | $3.88 | ~1462% | $22.43 |
| delta_hedged | +$3.76 | +$2.04 | $3.76 | $2.11 | ~619% | $1.03 |
| hedged_wide | +$0.63 | -$1.08 | $0.63 | $0.37 | ~104% | $0.16 |

The expanded sample rejected the candidate under all gate views. The lagged rule
failed 25/40/60/80-swap windows with win rates of about `47%`, `50%`, `43%`, and
`50%`, negative mean edge versus hold, and p05 APR between about `-426%` and `-248%`.
Direct `--gate-policy delta-hedged` also rejected with p05 APR about `-413%`, `-473%`,
`-428%`, and `-217%`; `--gate-policy hedged-wide` reduced drawdown but had p05 APR
only about `-61%`, `-71%`, `-65%`, and `-31%`.

Interpretation: `SOL-ORCA` is useful because it demonstrates the current failure mode
cleanly. A single full-window replay can show 600%-1400% mechanical APR while rolling
windows still lose to hold on average. The promotion gate is doing real work here:
this is a replay-based rejection, not a data-landing failure.

### Orca SOL-USDC Replay

`SOL-USDC` is not the highest APR Orca pool, but it is the best current capacity and
throughput benchmark: about `$24m` TVL, about `$231m` 24h volume, volume/TVL near
`9.55`, `4bps` fee tier, and about `139%` 24h fee APR. It tests whether a very large
and very busy pool can compensate for low fee tier with turnover alone.

The first public-RPC sample was dense in time but more expensive to land than
SOL-ORCA:

```text
sample A: scanned 759 signatures, kept 187 normalized swaps, tx_errors=0
span:     slot 429682709..429683021, tick span 26160..26162 after numeraire inversion
window:   about 2.1 minutes
```

With `narrow_half_width=100`, `wide_half_width=1000`, `$10k` capital, and snapshot
active liquidity, the full replay again looked attractive only before rolling-window
checks:

| policy | net PnL | vs hold | fees | fee-LVR | net APR window | max DD |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| hold_50_50 | -$0.86 | $0.00 | $0.00 | $0.00 | ~-2161% | $9.95 |
| narrow_rebalance | +$0.25 | +$1.10 | $1.11 | $0.55 | ~630% | $9.11 |
| vol_scaled_rebalance | +$1.34 | +$2.20 | $2.20 | $1.09 | ~3386% | $8.26 |
| delta_hedged | +$1.10 | +$1.96 | $1.11 | $0.55 | ~2781% | $0.21 |
| hedged_wide | +$0.11 | +$0.97 | $0.11 | $0.06 | ~286% | $0.02 |

The promotion gate rejected the sample. The lagged rule failed 25/40/60/80-swap
windows with p05 APR about `-2854%`, `-1144%`, `-728%`, and `130%`. Direct
`--gate-policy delta-hedged` rejected with p05 APR about `-2843%`, `-1032%`, `-687%`,
and `159%`; `--gate-policy hedged-wide` reduced drawdown but only reached p05 APR
about `-285%`, `-104%`, `-69%`, and `18%`.

Interpretation: this is a high-capacity microburst sample, not a long-regime
verdict. It does show that enormous volume is not enough by itself. Low fee tier can
leave the fee-LVR edge too thin for the 500% left-tail gate once rolling windows and
hold-edge checks are applied.

### Orca SOL-Fartcoin Replay

After covering the larger and cleaner Whirlpool candidates, the next pass tested a
higher-risk meme pair to see whether volatility plus a 16bps fee tier creates better
fee alpha than SOL-USDC. The refreshed Orca queue showed `SOL-Fartcoin` around
`116%` 24h fee APR, about `$527k` TVL, about `$1.05m` 24h volume, and volume/TVL near
`2.00`.

Sampling landed cleanly:

```text
sample A: scanned 192 signatures, kept 160 normalized swaps, tx_errors=0
span:     slot 429680775..429687571, tick span roughly -6715..-6567
window:   about 45.3 minutes
```

With SOL marked near `$73.47`, `narrow_half_width=384`, `wide_half_width=2500`, and
snapshot active liquidity, the full replay was the best Whirlpool risk-pair result
so far but still not deployable:

| policy | net PnL | vs hold | fees | fee-LVR | net APR window | max DD |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| hold_50_50 | -$27.84 | $0.00 | $0.00 | $0.00 | ~-3230% | $73.98 |
| narrow_rebalance | -$19.88 | +$7.97 | $10.04 | $6.32 | ~-2306% | $71.06 |
| vol_scaled_rebalance | -$14.28 | +$13.57 | $17.70 | $10.29 | ~-1656% | $69.24 |
| delta_hedged | +$8.00 | +$35.84 | $10.04 | $6.32 | ~928% | $2.60 |
| hedged_wide | +$1.49 | +$29.34 | $1.83 | $1.22 | ~173% | $0.39 |

Rolling windows exposed the same left-tail failure. `delta_hedged` had positive mean
net and a 64% win rate on 25-swap windows, but p05 APR was about `-1710%`.
`hedged_wide` reduced drawdown and left-tail loss, with p05 APR about `-245%`, but
the mean net was only about `+$0.13` per 25-swap window.

Promotion gates all rejected. The lagged rule failed 25/40/60/80-swap windows with
p05 APR about `-1865%`, `-875%`, `-789%`, and `-1892%`. Direct
`--gate-policy delta-hedged` rejected with p05 APR about `-1710%`, `-813%`, `-605%`,
and `-653%`; `--gate-policy hedged-wide` rejected with p05 APR about `-245%`, `-106%`,
`-97%`, and `-263%`.

Interpretation: treat this as a promising rejection, not a promotion. Compared with
SOL-USDC and SOL-ORCA, the hedge layer is closer to extracting real fee alpha, but
the current policy still cannot turn volatility into a stable 500%+ left-tail APR.
The next strategy work should target this failure mode: preserve more of the
`delta_hedged` mean edge while using `hedged_wide`-style tail control around trend
windows.

The first no-lookahead policy-switch test did not solve the tail. The default
`lagged-policy-switch` map (`range=delta_hedged`, all non-range regimes
`hedged_wide`) still rejected `SOL-Fartcoin`: 25/40/60/80-swap p05 APR was about
`-1865%`, `-906%`, `-708%`, and `-771%`. An all-`hedged_wide` map improved the left
tail materially to about `-267%`, `-119%`, `-108%`, and `-285%`, but that still falls
far below the 500% left-tail gate and sacrifices mean return. A beta-participation
map using `narrow_rebalance` after trend regimes made the tail much worse.

Interpretation: policy switching is now measurable, and the result is a useful
rejection. The issue is not just choosing between `delta_hedged` and `hedged_wide`
from coarse prior-window labels; the next strategy needs a sharper adverse-trend
signal or a smoother hedge-width control.

The first smoother blend test also rejected. `lagged-policy-blend` splits capital
between `delta_hedged` and `hedged_wide` sleeves. On `SOL-Fartcoin`, the default
range blend (`range_wide=0.50`, all non-range regimes at `1.00`) produced p05 APR
about `-1066%`, `-507%`, `-408%`, and `-528%` across 25/40/60/80-swap windows.
Increasing `range_wide` to `0.75`, `0.90`, and `0.95` improved the 25-swap p05 APR
to about `-667%`, `-427%`, and `-347%`; the all-wide boundary was still only about
`-267%`, `-119%`, `-108%`, and `-285%`. Mean net remained positive but small.

Interpretation: continuous capital blending is a better risk-control surface than
hard switching, but it still cannot produce a positive 500% left tail on the current
Whirlpool samples. The next useful strategy work is not another coarse blend; it is a
more timely adverse-trend signal, a different fee source, or a venue with structurally
higher fee density such as Meteora DLMM.

The first intra-window stop test was also a hard rejection. Code now exposes
`delta_trend_stop` / `--gate-policy delta-trend-stop`: a narrow `delta_hedged` LP
that stands down to the money leg when recent tick displacement crosses a short trend
threshold. On `SOL-Fartcoin`, the aggressive threshold run rejected every gate with
p05 APR about `-36738%`, `-21422%`, `-15534%`, and `-9876%` across 25/40/60/80-swap
windows. Looser thresholds helped but did not come close: the threshold-20 check still
had p05 APR about `-29691%`, `-12417%`, `-5460%`, and `-4683%`.

Window decomposition shows why: the stop often fired twice inside windows that the
slower classifier still labels `range`, turning small `delta_hedged` drawdowns into
double-digit dollar drawdowns and destroying fee mean. Interpretation: Whirlpool
on/off timing is not the missing edge. The evidence now pushes us away from more gate
micro-tuning and toward either historical active-liquidity reconstruction or a
higher-fee venue. A bounded Meteora DLMM replay skeleton is now in
`autopool_backtest::dlmm`; it models bin-step price ratios, active-bin fee share,
range occupancy, recenter costs, drawdown, and APR for normalized bin observations.
It is not deployable until real Meteora swap events and historical bin-liquidity
snapshots are decoded.

### Meteora SOL-USDC DLMM Snapshot

The first read-only Meteora SDK ingestion is now wired:

```bash
scripts/meteora-dlmm-snapshot.sh \
  --spec data/solana/hot-pool/specs/meteora-solusdc-5rcf1dm8.json \
  --out data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-bin-snapshots.jsonl \
  --raw-out data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-bin-snapshot.latest.json \
  --raw-jsonl-out data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-bin-snapshots.raw.jsonl \
  --append \
  --bins-left 8 \
  --bins-right 8 \
  --volume-window 30m

cargo run -q -p autopool-cli -- replay-dlmm-bins \
  --spec data/solana/hot-pool/specs/meteora-solusdc-5rcf1dm8.json \
  --bins data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-bin-snapshots.jsonl \
  --half-width-bins 5 \
  --capital-usd 10000
```

Fresh snapshot:

```text
pool:      Meteora SOL-USDC 5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6
slot:      429710432
activeBin: -6457
bin step:  4 bps
active-bin liquidity: ~$5,060
30m volume from Meteora Data API: ~$1.83m
```

The one-row replay returns about `+$486` gross fee for a `$10k` position in the
active bin, but this is not an APR result and not scalable evidence: capital is
about `2.0x` the active-bin liquidity, so the modeled fee share is a capacity alarm,
not a deployment proposal. This is still valuable because it proves the business
flow is now DLMM-native: protocol API metadata -> official SDK active bin/bin
liquidity -> `DlmmBinObs` JSONL -> Rust DLMM replay. Next evidence step is repeated
snapshots or decoded swap/bin history, then rolling windows.

The append mode is for heartbeat-style sampling and dedupes by Solana slot. Treat
Data API rolling-window volume (`30m`, `1h`, and similar) as a capacity/fee-density
probe only: `amount_in_usd` must be non-overlapping interval flow before the replay
APR or rolling promotion gate is meaningful. The deployable Meteora target remains
decoded swap flow plus matched historical active-bin liquidity snapshots.

Append smoke on 2026-06-30 produced two SOL-USDC rows:

```text
slots:        429714568 -> 429714639
active bins:  -6468 -> -6465
active liq:   ~$10.3k -> ~$10.0k
30m volume:   ~$1.18m -> ~$0.99m
cap/active:   ~1.0x for $10k capital
```

The Rust DLMM replay accepts the stream and emits the expected caveats, but this is
not promotion evidence: the volume fields are overlapping rolling windows, and the
position is still roughly equal to active-bin liquidity.

### Meteora SOL-USDC Swap Flow Probe

The first real non-overlapping Meteora swap-flow sampler is now wired:

```bash
scripts/meteora-dlmm-swap-flow.sh \
  --spec data/solana/hot-pool/specs/meteora-solusdc-5rcf1dm8.json \
  --out data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-swap-flow.jsonl \
  --raw-out data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-swap-flow.latest.json \
  --limit 25 \
  --signature-scan-limit 120 \
  --max-signature-pages 2 \
  --append
```

2026-06-30 probe result:

```text
scanned signatures: 99
decoded swap txs:   17
tx errors:          0
flow notional:      ~$40,048
directions:         11 USDC->SOL, 6 SOL->USDC
avg-exec bin proxy: -6466..-6463
next cursor:        3Lsoxn3oMZ7eSsT88TB1GpRent8pmM8ug3u1D86JKskc6zxFMHWikWhrEWXVdvLKodtKkTqWxQJP6yUN1rMaVgV3
```

This is better evidence than rolling Data API volume because each row comes from
pool-owned reserve deltas in one successful swap transaction. The hard blocker is
also now concrete: sampled `LBUZ...` Meteora swap logs did not emit parseable Anchor
`Swap` / `Swap2Evt` event payloads for the pool, and the decoded `swap` instruction
contains only `amountIn` / `minAmountOut`, not historical post-swap active bin or
active-bin liquidity. Therefore replay-grade DLMM observations now need one of:

- join recent swap-flow rows to repeated SDK active-bin snapshots for a bounded
  live shadow monitor;
- use an archival/indexed account-state source for historical `lbPair` and bin-array
  state at each swap slot;
- add a documented approximation that maps average execution price to a bin id, but
  gate it as proxy-only until active-bin liquidity is reconstructed.

### Meteora Account Reconstruction Probe

The account-level feasibility probe is now scripted:

```bash
scripts/meteora-dlmm-account-probe.sh \
  --spec data/solana/hot-pool/specs/meteora-solusdc-5rcf1dm8.json \
  --flow data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-swap-flow.jsonl \
  --limit 5 \
  --raw-out data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-account-probe.latest.json
```

2026-06-30 result:

```text
sampled flow rows:       5
decoded swap ix:         5
parseable events:        0
touched bin arrays:      3, indexes -93/-92/-91
current active id:       -6462, active bin array -93
status:                  blocked_without_archival_account_state
```

This is useful because it separates two questions. We can identify which DLMM
program accounts a swap touched, and the official SDK can decode current `lbPair`,
`binArray`, `oracle`, and bitmap-extension accounts. But Solana `getTransaction`
does not return historical account data, public `getAccountInfo` returns current
state only, and these sampled logs still produce no official `Swap` / `Swap2Evt`
events with historical active-bin liquidity. Replay-grade active liquidity therefore
requires either an archival account-state/indexer source, or a live same-slot/near-slot
snapshot pipeline that is explicitly gated as shadow evidence.

### Meteora Flow/Snapshot Proxy Join

The first bounded live-shadow join is now available:

```bash
node scripts/meteora-dlmm-join-flow-snapshots.cjs \
  --flow data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-swap-flow.jsonl \
  --snapshots data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-bin-snapshots.jsonl \
  --out data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-bin-flow-proxy.jsonl \
  --raw-out data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-bin-flow-proxy.latest.json \
  --max-slot-distance 400 \
  --active-bin-source flow-price

cargo run -q -p autopool-cli -- replay-dlmm-bins \
  --spec data/solana/hot-pool/specs/meteora-solusdc-5rcf1dm8.json \
  --bins data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-bin-flow-proxy.jsonl \
  --half-width-bins 5 \
  --capital-usd 1000

cargo run -q -p autopool-cli -- replay-dlmm-bin-windows \
  --spec data/solana/hot-pool/specs/meteora-solusdc-5rcf1dm8.json \
  --bins data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-bin-flow-proxy.jsonl \
  --window-observations 15 \
  --step-observations 5 \
  --min-windows 5 \
  --half-width-bins 5 \
  --capital-usd 1000
```

2026-06-30 live-shadow smoke:

```text
snapshot A: slot 429723287, active bin -6465, active liq ~$9,971
flow scan:  scanned 51 signatures, decoded 20 swaps, tx_errors=0
snapshot B: slot 429723404, active bin -6462, active liq ~$9,239
join gate:  19 joined rows, 18 stale rows skipped, max distance 364 slots, avg 252 slots
flow:       ~$68,467 joined notional, active-bin proxy -6465..-6461
capacity:   $1k capital / ~$9,971 active-liq ~= 0.10x
proxy PnL:  static/centered +$2.10 net, $2.50 fees, ~$0.39 maxDD over 19 rows
```

Do not read the printed `46988%` annualized APR as strategy performance: the window is
only 19 swaps over roughly 140 seconds, and active liquidity is from nearest snapshots
rather than historical bin-array state. This result matters because the data plumbing
finally separates real non-overlapping flow from active-liquidity approximation and
keeps old flow out via a slot-distance gate.

Second live-shadow refresh expanded the proxy stream:

```text
snapshot C: slot 429728059, active bin -6454, active liq ~$5,777
flow scan:  scanned 194 signatures, decoded 27 swaps, tx_errors=0
snapshot D: slot 429728324, active bin -6453, active liq ~$5,748
join gate:  47 joined rows, 17 stale rows skipped, max distance 417 slots, avg 208 slots
flow:       ~$104,159 joined notional, active-bin proxy -6465..-6453
capacity:   $1k capital / ~$7,562 avg active-liq ~= 0.13x
full proxy: centered +$6.18 net, $4.28 fees, $0.39 maxDD, 1 rebalance
```

Rolling proxy windows now run through `replay-dlmm-bin-windows`, including a
promotion-style proxy gate. The default gate requires p05 mechanical net APR >= `500%`,
positive mean edge versus hold, win rate versus hold >= `60%`, worst drawdown <= `5%`
of capital, and mean capital/active-liquidity <= `0.25x`.

On 10-row/5-step windows (`8` windows), centered/static both had `100%` win rate versus
hold and p05 mechanical APR around `12.6k%`; on 15-row/5-step windows (`7` windows),
centered had `100%` win rate, mean net about `+$2.00`, p05 mechanical APR about
`4,976%`, and worst drawdown about `$0.39`. With `$1k` capital, the 15-row run now
marks `centered_bin_rebalance` and `static_bin_range` as `pass_proxy_gate`; hold is
`reject_proxy` due to `win_rate` and `left_tail_apr`. This is a stronger live-shadow
signal than the prior full-only smoke, but still not deployable: the sample is
minutes-long, active liquidity is nearest-snapshot proxy, and the active-bin liquidity
fell sharply as price moved.

### Meteora Live Shadow Runner

The live shadow flow is now one command:

```bash
scripts/meteora-dlmm-live-shadow.sh \
  --flow-limit 25 \
  --signature-scan-limit 180 \
  --max-signature-pages 2 \
  --max-slot-distance 250 \
  --request-sleep-ms 100
```

The runner takes a pre-flow snapshot, samples real swap flow, takes a post-flow
snapshot, joins with a strict slot-distance gate, and then runs full + rolling DLMM
proxy replay. It writes the latest text report to
`data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-live-shadow.latest.txt`.

2026-06-30 strict refresh:

```text
snapshot before: slot 429741140, active bin -6465, active-liq ~$11,354
flow scan:       scanned 52 signatures, decoded 25 swaps, tx_errors=0
snapshot after:  slot 429741214, active bin -6465, active-liq ~$11,354
join gate:       89 flow rows, 8 snapshots, 40 joined, 49 stale skipped
slot distance:   max 242, avg 133.5, max allowed 250
full proxy:      centered +$2.03 net, $2.83 fees, $3.30 maxDD, 2 rebalances
rolling gate:    centered/static/hold all reject_proxy
```

The key change is the left tail: under 15-row/5-step windows, centered still had
positive mean edge (`+$0.64` versus hold) and acceptable capacity (`~0.14x`), but p05
mechanical APR was about `-1,250%`, so the default gate rejected it for
`left_tail_apr`. Static also rejected on left tail. This supersedes the prior
`pass_proxy_gate` as the current SOL-USDC read: the pool remains interesting, but not
strategy-approved.

Second strict refresh kept the same failure mode:

```text
snapshot before: slot 429745397, active bin -6459, active-liq ~$9,673
flow scan:       scanned 93 signatures, decoded 25 swaps, tx_errors=0
snapshot after:  slot 429745519, active bin -6459, active-liq ~$9,700
join gate:       114 flow rows, 10 snapshots, 50 joined, 64 stale skipped
slot distance:   max 242, avg 128.0, max allowed 250
full proxy:      centered +$4.29 net, $4.39 fees, $3.30 maxDD, 3 rebalances
rolling gate:    centered/static/hold all reject_proxy
```

Centered still had positive mean edge (`+$0.84` versus hold), but failed
`left_tail_apr` again with p05 mechanical APR about `-1,245%`; static failed the same
left-tail gate with p05 about `-1,115%`. Two consecutive strict refreshes now reject
SOL-USDC, so the next useful path is either a different Meteora USDC pair or a longer
strict live-shadow sample that proves the left tail recovers.

Current Meteora candidate refresh put `JUP-USDC` at the top of the USDC-numeraire queue
after excluding SOL-USDC: about `$916k` TVL, `$5.7m` 24h volume, `717%` fee APR, and
volume/TVL around `6.22`. Initial JUP live-shadow sampling decoded 25 swaps from 67
signatures with `0` tx errors and active-bin liquidity around `$25k`, but all flow rows
were outside the 250-slot join gate. A second small run skipped replay cleanly:

```text
JUP-USDC snapshots: 4
flow rows:          25
joined rows:        0 under 250-slot max distance
status:             replay skipped; no live-shadow window
```

That is not a strategy rejection; it is a near-slot data availability rejection under
the strict live-shadow gate. The runner now exits cleanly when joined rows cannot
produce enough rolling windows.

JUP 400-slot preliminary run:

```text
flow scan:       scanned 28 signatures, decoded 25 swaps, tx_errors=0
join gate:       50 flow rows, 6 snapshots, 27 joined, 23 stale skipped
slot distance:   max 331, avg 324, max allowed 400
full proxy:      centered/static +$0.28 net, 100% in range, 0 drawdown
rolling gate:    3 windows, centered/static pass_proxy_gate
```

This is only scout evidence. The join is wider than the strict 250-slot gate, the
sample has only 3 rolling windows, active bin stayed flat at `-305`, and the positive
PnL is only about `$0.14` mean per window before mechanical annualization. JUP remains
a live-shadow candidate, not a strategy-approved pool.

MET-USDC and HYPE-USDC broadening:

```text
MET-USDC 20bps: 25 decoded swaps, active-liq ~$998, 0 joined under 250-slot gate
HYPE-USDC 20bps: active-liq ~$33.5k, 25 decoded swaps, 1 joined under 250-slot gate
HYPE 400-slot scout: 23 joined, 2 windows, full proxy centered -$0.06 net
```

MET is capacity-poor for `$1k` capital at the active bin. HYPE has better capacity
than JUP and SOL, but the 250-slot gate still lacks enough near-slot rows; the 400-slot
scout is internally mixed and below promotion sample size. Treat HYPE as the next pool
to keep sampling, not as promoted.

The next HYPE refresh added only 5 fresh swap rows but widened the 400-slot proxy to 28
joined rows and 3 rolling windows. It exposed a gate bug: the old p05 calculation used
linear interpolation, so a 3-window sample with one deeply negative window could still
show a positive p05. `percentile_f64` now uses conservative nearest-rank lower-tail
percentiles. After the fix, the same HYPE 400-slot scout correctly rejects:

```text
centered: reject_proxy, meanNet -$1.20, meanVsH +$1.11, p05APR -11134%, cap/liq 0.03x
static:   reject_proxy, meanNet -$1.10, meanVsH +$1.21, p05APR -10580%, cap/liq 0.03x
```

This is a genuine research improvement: the promotion gate now catches small-sample
left tails instead of interpolating them away.

Follow-up HYPE live-shadow refresh on the same `ANCx...` pool improved strict
near-slot coverage but did not change the strategy verdict. With a 250-slot join
gate, the accumulated stream reached 80 flow rows, 10 joined rows, and 10 snapshots,
still below the 15-observation minimum for a rolling window. The 400-slot preliminary
join reached 32 rows and 4 windows, but both DLMM policies again rejected on
`left_tail_apr`:

```text
centered: reject_proxy, meanNet -$1.72, meanVsH +$1.01, p05APR -11134%, cap/liq 0.03x
static:   reject_proxy, meanNet -$1.68, meanVsH +$1.05, p05APR -10580%, cap/liq 0.03x
```

HYPE remains useful for capacity/live-shadow plumbing because active-bin liquidity is
around `$33k`, but the current evidence is not a promotion signal. The next useful
bounded run is either one more strict 250-slot HYPE collection to reach the first
near-slot window, or a switch to another high-capacity Meteora USDC pool if HYPE keeps
replaying the same left-tail failure.

The next heartbeat reran HYPE and confirmed the stall: strict 250-slot still had 10
joined rows after 95 flow rows and 14 snapshots, so it remained below the first rolling
window. The 400-slot scout also stayed on the same 32 joined rows and 4 windows, with
the same `left_tail_apr` rejection. That makes further HYPE sampling lower priority
unless fresh flow starts joining near-slot.

Two additional Meteora USDC candidates were then checked with the same live-shadow
runner:

```text
cbBTC-USDC 4bps: active-liq ~$4.1k, 25 decoded swaps, 0 joined under 250 slots, 2 joined under 400 slots
SPCX-USDC 10bps: active-liq ~$8.6k, 25 decoded swaps, 4 joined under 250 slots, 6 joined under 400 slots
```

cbBTC is too close to the capacity ceiling for `$1k` capital (`~0.24x` cap/active-liq)
and still lacks replay windows. SPCX has better capacity (`~0.12x`) and clean decoding
but also lacks enough joined rows for a promotion gate. The next useful heartbeat is
to continue SPCX same-slot sampling, or refresh the Meteora queue for a higher
active-liquidity USDC-numeraire pool rather than spending more cycles on HYPE.

Follow-up sampling showed SPCX was also near-slot blocked: after a second strict run,
SPCX had 50 flow rows but still only 4 joined rows under 250 slots and 6 joined under
400 slots. Refreshing the Meteora queue moved attention to MU-USDC. Snapshot screening
found the older HYPE 10bps pool had only about `$2.3k` active-bin liquidity and both
MET-USDC pools were around `$0.9k-$1.1k`, while `MU-USDC` 50bps had about `$14.3k`
and the larger `MU-USDC` 20bps pool had about `$16.0k`.

The 20bps MU pool became the best current live-shadow target: it decoded 9 swaps from
267 scanned signatures with 0 tx errors, and all 9 joined under the strict 250-slot
gate. That is not enough for a 15-row replay window, but it is a better data-quality
signal than SPCX/HYPE because the blocker is sample count, not stale joining. The
live-shadow wrapper now exposes `--before-signature` so future heartbeats can use the
swap-flow cursor and collect older pages instead of repeating the newest signatures.
Large cursor scans were too slow on the current public RPC, so the next bounded step is
small cursor batches or a better RPC/indexer, not a wider blind scan.

The next MU run confirmed the sampling shape. A newest-page small batch added 10 fresh
MU rows and raised strict 250-slot joined rows from 9 to 12. An older-page cursor batch
added 5 more flow rows, but strict joined rows stayed at 12 because those older swaps
were outside the available live snapshot window. Rejoining the accumulated stream with
a wider 400-slot proxy admitted 15 rows and produced one tiny rolling window:

```text
MU-USDC 20bps, 400-slot proxy: observations=15, windows=1, cap/active-liq ~0.07x
centered: reject_proxy, net -$3.57, vs hold +$2.39, fees $2.89, p05APR -7388%, maxDD $6.46
static:   reject_proxy, net -$4.71, vs hold +$1.24, fees $1.24, p05APR -9770%, maxDD $5.96
```

This is not a promotion signal. It does show MU has real fee capture and acceptable
capacity, but inventory drift still dominates in the one available proxy window. The
more important engineering lesson is that cursor backfill can add flow rows but cannot
create strict historical active-liquidity snapshots. The live-shadow wrapper now also
supports `--cursor-file`, which reads and writes the `next_before_signature` cursor so
future heartbeats can run small continuous backfill batches without manually copying
signatures.

### Orca HYPE-USDC Replay

`HYPE-USDC` was the remaining replayable Orca P1 candidate after the SOL-pair coverage
pass: about `123%` 24h fee APR, `16bps` fee tier, about `$59k` TVL, about `$126k` 24h
volume, and volume/TVL around `2.12`. It is small, but it completes the current
Whirlpool coverage set before moving back to strategy or DLMM work.

Sampling quality was high:

```text
sample A: scanned 121 signatures, kept 120 normalized swaps, tx_errors=0
span:     slot 429687986..429691923, tick span 27496..27423 after numeraire inversion
window:   about 26.2 minutes
```

This sample was a directional up/beta segment rather than a fee-alpha segment. With
`narrow_half_width=384`, `wide_half_width=2500`, `$10k` capital, and snapshot active
liquidity, hold dominated the hedged LP policies:

| policy | net PnL | vs hold | fees | fee-LVR | net APR window | max DD |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| hold_50_50 | +$36.49 | $0.00 | $0.00 | $0.00 | ~7307% | $19.18 |
| narrow_rebalance | +$35.56 | -$0.92 | $2.63 | $2.12 | ~7122% | $18.93 |
| vol_scaled_rebalance | +$33.25 | -$3.24 | $3.84 | $2.82 | ~6659% | $18.95 |
| delta_hedged | -$0.86 | -$37.35 | $2.63 | $2.12 | ~-172% | $2.60 |
| hedged_wide | +$0.05 | -$36.44 | $0.61 | $0.53 | ~10% | $0.36 |

Promotion gates rejected every view. The lagged rule failed 25/40/60/80-swap windows
with win rates about `33%`, `20%`, `33%`, and `0%`, negative mean edge versus hold,
and p05 APR about `-882%`, `-452%`, `-754%`, and `224%` on the thin 80-swap view.
Direct `--gate-policy delta-hedged` rejected with p05 APR about `-859%`, `-413%`,
`-721%`, and `234%`; `--gate-policy hedged-wide` rejected with p05 APR about `-75%`,
`-13%`, `-79%`, and `71%`.

Interpretation: this is not an alpha lead. It is a clear trend-risk case: hedging
protects drawdown but gives up beta, and the resulting fee edge is not enough to beat
hold. Together with the SOL-Fartcoin result, it argues that the next useful strategy
iteration needs regime-aware beta participation, not just narrower ranges or heavier
hedges.

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
   - Meteora DLMM swaps/bin liquidity. The `autopool_backtest::dlmm` replay skeleton
     is available; the missing piece is real normalized `DlmmBinObs` ingestion.
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
