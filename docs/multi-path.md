# Multi-Path Bootstrap: What Survives the Distribution

Single windows mislead. `narrow_static` won one calm window; `narrow_rebalance` won
the (up-)trend window on favorable beta. To see what actually has positive
expectancy we resample the real swap stream into many alternate histories
(moving-block bootstrap) and look at the *distribution* of each policy's net PnL.

`multi_path_eval` / `autopool-cli multi-path`:
- resample contiguous blocks of (tick-increment, amounts, liquidity, block-gap),
  accumulate into new tick paths, rebuild swaps;
- `--demean` removes the source's mean drift so paths are **martingale** — the
  regime under which LP economics are isolated from the directional bet a one-way
  source window would otherwise inject.

## Result — demeaned (martingale) bootstrap of the real trend window

200 paths, 13,259 swaps each, ±100 band, fee 21.25 bps, reward 22.49%, funding 10
bps/day, 3-block latency:

| policy | mean net | std net | p05 | p95 | fee−LVR | win% vs hold |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| hold_50_50 | +8 | 558 | -828 | 1217 | 0 | — |
| **passive_wide** | **+10** | 542 | -916 | 1028 | 31 | **72%** |
| narrow_static | -98 | 678 | -1492 | 695 | 174 | 48% |
| hard_exit_stop | -88 | **193** | -410 | 246 | 189 | 46% |
| narrow_rebalance | -545 | 486 | -1374 | 225 | 1140 | 0% |
| vol_scaled_rebalance | -1071 | 436 | -1757 | -430 | 1787 | 0% |
| **delta_hedged** | -561 | **215** | -957 | -255 | 1140 | 20% |

## Reading — three honest, important findings

1. **The delta hedge collapses directional variance — confirmed.** `delta_hedged`
   has the lowest net-PnL std of any active policy (215 vs hold's 558, narrow's 486)
   and the tightest p05–p95. The hedge does exactly what it is for: it removes the
   beta swing.

2. **But the hedge does not fix the bigger enemy — rebalance churn.** Every *narrow
   rebalancing* policy is net-negative across the distribution despite large positive
   *gross* alpha (`narrow_rebalance` fee−LVR +$1140, `vol_scaled` +$1787). In a
   realistically volatile, driftless market the price oscillates and a ±100 band is
   crossed constantly; the rebalance gas+slippage to chase it **exceeds** the gross
   fee-alpha. LVR theory assumes costless continuous rebalancing — on-chain it is not
   costless, and that cost is what kills these policies. `delta_hedged` inherits the
   same churn (it is narrow_rebalance + hedge), so its lower variance comes with the
   same negative mean.

3. **What survives is *low churn*.** `passive_wide` beats hold on **72%** of paths
   (a small, consistent fee edge with almost no rebalancing); `hard_exit_stop` has
   the lowest churn-policy variance. The high-density narrow strategies that looked
   best on single calm windows do **not** survive the volatility×cost distribution.

## Revised strategic conclusion

Earlier single-window results overstated narrow bands. Across the realistic
distribution:

- **Minimize churn.** The robust positive-expectancy LP on this pool is a wide,
  rarely-touched band, not a tight rebalancing one. On-chain rebalance cost is the
  dominant adversary, ahead of even adverse selection.
- **Hedge for variance/tail, not for mean.** Delta-hedging is a genuine
  variance/drawdown reducer and crash insurance, but it cannot turn a churn-losing
  policy into a winner.
- **21 bps is too thin.** Even optimally (low churn), WETH-AERO's net edge over hold
  is small (passive_wide mean ≈ +$10 on $10k). The fee density simply does not pay
  for much active management. This is the strongest quantitative case yet for the
  `scan-pool-activity` pivot: **find higher-fee / genuinely ranging pools** where the
  gross alpha is large enough to survive rebalance costs.

## Caveats

- The bootstrap preserves local microstructure but assumes blocks are exchangeable;
  it cannot create volatility regimes absent from the source.
- `--demean` removes drift to isolate LP economics; the *raw* (drifted) bootstrap is
  also available and shows the direction-inherited picture.
- Rebalance slippage is a flat bps model; a depth-aware model could change the churn
  cost magnitude but not the direction of the conclusion.
