# Walk-Forward Calibration

The adaptive policy's trend-exit threshold is regime/noise-sensitive: tuned on clean
synthetics it overfits and churns on real flow (see `docs/tail-risk-scenarios.md`).
Walk-forward calibration removes the hand-picked parameter: choose parameters using
only *past* data, apply them out-of-sample, roll forward.

## Method

For each fold:

1. **Train** on `train_swaps` of past swaps: grid-search `(trend_exit_threshold,
   half_width)` and pick the pair maximizing a risk-adjusted score
   `net_pnl − drawdown_penalty · max_drawdown`.
2. **Test** on the next `test_swaps` (out-of-sample) with the chosen pair.
3. Roll the train window forward by `test_swaps` and repeat.

Parameters are never scored on the data they are applied to. Test segments are
contiguous and non-overlapping, so their nets sum to an honest OOS total. Three
baselines run on the *same* test segments: fixed-parameter adaptive (median grid),
narrow static, and hold.

`autopool-backtest::walk_forward` + `autopool-cli walk-forward`.

## Result on real WETH-AERO

```
walk-forward WETH-AERO swaps=4987 folds=8 train=1000 test=500
grid: thr=[2,4,6,8,10] hw=[100,300] penalty=0.5

fold  test_blk     chosen_T   hw   test_net   test_DD  rebals  tick_span
 0    1000..1500     6.0      100   -124.49    179.45    0       204
 1    1500..2000     6.0      300     35.98     16.60    2       121
 2    2000..2500     6.0      300    -20.61     59.23    1       269
 3    2500..3000     6.0      100     41.58     34.00    2       283
 4    3000..3500     2.0      300    108.23     19.16    4       385
 5    3500..4000     2.0      100   -186.12    186.17   15       299
 6    4000..4500     8.0      300     83.55     30.54    0       135
 7    4500..4987     2.0      300    -53.93     77.97   12       295

OOS net (walk-forward adaptive):  -115.81   maxDD 186.17
OOS baselines  fixed_adaptive: -273.81   static: -194.41   hold: -113.79
```

## Reading

1. **Calibration adds value out-of-sample.** Walk-forward (-$116) beat both
   fixed-parameter adaptive (-$274) and static (-$194). Letting the threshold/width
   adapt per fold — chosen only from past data — is better than any single fixed
   setting. The chosen threshold genuinely varies (6,6,6,6,2,2,8,2) as the local
   regime changes.

2. **But it only ties hold (-$116 vs -$114).** Over this calm, slightly-adverse
   window, LP fees did **not** beat simply holding the two tokens — even with the
   best calibrated policy. This is the system working as intended: it says *do not
   LP this pool in this regime*. Headline APY would have implied the opposite.

3. **The objective still lets churn through.** Fold 5 trained into `T=2`, did 15
   rebalances and lost $186 — the train window misled the grid into a churny param.
   The drawdown penalty (0.5) damps this but does not eliminate it.

## Next

- Add an explicit **rebalance/turnover penalty** (or a cost-per-action term) to the
  train objective so folds stop selecting churny low thresholds.
- Calibrate over a richer grid (window length, vol_k) and compound capital across
  folds for a true equity curve.
- Re-run once a **real volatile/crash window** is backfilled — the current data is
  all low-vol, so the down-tail protection the adaptive policy is built for has not
  yet been exercised on real flow.
- Decision gate: only deploy capital to a pool when walk-forward OOS net beats hold
  by a margin after costs.
