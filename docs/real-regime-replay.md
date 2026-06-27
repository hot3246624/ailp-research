# Real-Regime Replay: LP Loses to Hold in a Trend

First replay of the engine on **real, non-calm** on-chain data (previously the only
collected data was a flat window). It directly confirms the short-gamma thesis on
real flow.

## Unblock: free RPC with large getLogs

The Alchemy free tier hard-caps `eth_getLogs` at 10 blocks, which made historical
collection take hours. The official **`https://mainnet.base.org`** (and
`base.drpc.org`) accept **10,000-block** `getLogs` ranges for free — 1000× the cap.
Using it with `--swaps-only --log-chunk-blocks 2000`, the steepest 20k-block segment
(2,648 swaps) was collected in ~5 minutes instead of ~2 hours. Use the public
endpoint for historical/scan-heavy work; keep the Alchemy key for live indexing.

## Window

- Pool: WETH-AERO `0x4e50…ce51`, blocks `47527088..47546984` (~20k blocks ≈ 11h)
- 2,648 swaps; ticks `82720 → 81810` — AERO **appreciating ~9%** with internal chop.
  A genuine **trending** regime (not the flat slice we had).

## Result (capital $10k, WETH=$1,574, fee 21.25 bps, ±100, 3-block latency)

| policy | net PnL | vs hold | fees | max DD | in-range | rebals |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| **hold_50_50** | **+476** | 0 | 0 | 138 | 0% | 0 |
| passive_wide | +443 | -33 | 9 | 121 | 100% | 0 |
| narrow_rebalance | +143 | -333 | 376 | 159 | 100% | 16 |
| adaptive_regime | +84 | -392 | 69 | **21** | 15% | 2 |
| narrow_static | +62 | -414 | 37 | 21 | 15% | 0 |
| vol_scaled_rebalance | -84 | -560 | 603 | 289 | 99% | 57 |
| hard_exit_stop | -99 | -575 | 252 | 281 | 66% | 11 |
| hedged_narrow | -334 | -810 | 376 | 373 | 100% | 16 |

## Reading

1. **Every LP policy loses to hold in this trend.** AERO rose ~9%; an LP position
   sells AERO into the rise (price falls in tick terms → the position converts to
   WETH) and gives up the move. The best LP (`narrow_rebalance`, +$143) still
   trails hold by $333. This is the short-gamma reality on real data: in a
   sustained directional move, providing liquidity underperforms holding.

2. **Symmetric to the crash.** In the synthetic crash, LP held the *falling* asset
   and lost to hold; here LP gave up the *rising* asset and lost to hold. **LP only
   wins in ranging regimes** (the calm window, where narrow LP beat hold on fees).

3. **Tail control preserves capital but cannot manufacture trend alpha.** `adaptive`
   and `narrow_static` had the lowest drawdown ($21 vs hold's $138) by sitting out
   most of the move, but low drawdown ≠ beating a favorable trend. `hedged_narrow`
   was worst here: the short hedge *loses* when the risk asset rises (-$477), the
   mirror of how it *saved* the crash.

4. **Mechanical churn is still bad.** `vol_scaled` (57 rebalances) and `hard_exit`
   (11, chasing the benign down-tick trend) both went negative.

## The strategic conclusion

The meta-decision dominates the range-management decision: **LP is a ranging-regime
strategy.** A regime detector's first job is not to pick a width — it is to decide
*whether to be an LP at all*:

- ranging → LP (tight, fee density), the calm-window result;
- trending → **exit LP and hold** (which asset to hold is a separate directional
  view), because every LP structurally loses to hold in a trend.

The current `adaptive` policy exits to the *money* leg on a danger (down-crash)
trend but merely follows a benign (up) trend with recenters. The real-data result
says it should instead **stand down to hold** in *either* strong trend. That is the
next refinement: a "LP-on/off" gate driven by trend strength, sitting above the
range policy.

## The LP on/off gate (`regime_gated`) on real data

`RangeMode::RegimeGated` operationalizes "LP only in ranging regimes": on any strong
trend it stands the position down to the money leg, and resumes LPing when price
ranges again (enter/exit hysteresis). On synthetics it cleanly beats static LP over
a calm→crash path (earns fees in calm, de-risks in the crash, caps drawdown), and
matches the adaptive policy in calm/crash.

But the **full ~13k-swap real trend window** (hold +$1,145) is humbling for *all*
reactive policies:

| policy | net | vs hold | rebals |
| --- | ---: | ---: | ---: |
| hold_50_50 | +1,145 | 0 | 0 |
| passive_wide | +967 | -178 | 0 |
| narrow_rebalance | +416 | -729 | 66 |
| adaptive_regime | +228 | -917 | 11 |
| regime_gated | +197 | -948 | 8 |
| narrow_static (±100, held) | +62 | -1,083 | 0 |

And walk-forward (per-fold calibration, OOS) makes it starker:

```
OOS net  walk-forward adaptive: +487   fixed_adaptive: +604
         static(±300, recentered/fold): +866   hold: +809
```

## The honest, crystallized conclusion

Across everything collected, active LP edge for WETH-AERO is **marginal and entirely
regime-dependent**:

1. **Calm/ranging** → narrow LP beats hold (fee density wins). This is the only
   regime where active LP has clear edge.
2. **Steady trend** → *nothing beats hold*, and **reactive trend policies whipsaw
   and underperform even a simple periodically-recentered static band**. Standing
   down / re-entering against a smooth trend churns; a band that just rides each
   segment does better. The gate's conservative de-risk also forgoes the up-move.
3. **Sharp crash** (synthetic, not yet real) → reactive de-risk (hard-exit / gate)
   and a short hedge are decisively better than naive LP; this is where the tail
   machinery earns its keep.

So the gate is the right tool for the **crash tail and mixed paths**, but it is *not*
a win in a steady trend — there, the correct action is simply *hold* (or a wide,
rarely-touched band), not active management. The dominant lever remains the LP
on/off decision by regime; the second lesson is **don't over-react in a steady
trend** (re-entry whipsaw is a real cost, confirmed on real flow).

This tempers any enthusiasm for active range management on this pool: at 21.25 bps,
WETH-AERO's fee density only pays in calm markets. Higher-fee / higher-churn pools
(or pools with a real ranging-heavy price process) are where active LP is more
likely to clear the bar — which is what `scan-pool-activity` is for.

## Caveats

- The "hold" baseline is hold-50/50; "hold 100% AERO" would have done even better in
  this up-move, but that is a pure directional bet, not an LP strategy.
- Fees use the static 21.25 bps tier; rewards/LVR still unmodeled (rewards would
  lift all LP rows; LVR would clarify the calm-regime edge).
- The trend window is one sustained up-move; a real *crash* window (AERO down) has
  not yet been found in accessible history to validate the down-tail machinery on
  real flow.
