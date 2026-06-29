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
