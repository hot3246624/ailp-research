# Tail Risk: Down-Crash, Rebalance Churn, Hedging

The calm-window result (`docs/replay-weth-aero.md`) showed a tight static range
winning by fee density. The obvious objection: **high-fee pools pump and dump, and
you cannot rebalance instantly.** A down-crash leaves the LP holding the falling
knife; a badly-set range forces frantic rebalancing that crystallizes loss. This
note stress-tests exactly that.

## What was added to the engine

`crates/backtest/replay.rs` now models:

- **Execution latency** (`--action-delay-blocks`): a triggered exit/rebalance
  executes N blocks later, at the (worse) delayed price. You cannot react instantly.
- **One-way hard-exit** (`hard_exit_stop`): narrow band, but on the *danger* side
  (price moving into the risk asset) it liquidates to the money leg and stands
  aside until price retraces, instead of chasing.
- **Static short hedge** (`hedged_narrow`): short the risk asset (token1) sized to
  the entry exposure, with funding cost (`--funding-bps-per-day`).
- **Tail metrics**: max drawdown, lowest equity, longest contiguous span stuck
  holding the risk asset (`max_one_sided_risk_blocks`), and *toxic fees* (fees
  earned while sitting in the risk-accumulation half of the range).
- **Scenario generator** (`replay-scenario --scenario calm|pump|crash|chop`),
  because the collected WETH-AERO data is calm and has no crash to replay.

## Result — the regime decides everything

Synthetic, WETH-AERO economics (fee 21.25 bps, WETH=$1,574, $10k capital, ±300
band, 3-block latency, funding 10 bps/day). Net PnL in USD:

### Crash — risk asset (AERO) down ~45% (tick +6000)

| policy | net PnL | vs hold | max DD | stuck-in-AERO blocks | hedge PnL |
| --- | ---: | ---: | ---: | ---: | ---: |
| narrow_static (patient) | **-4,469** | -2,214 | 4,484 | 1,474 | — |
| narrow_rebalance (chase) | -3,758 | -1,502 | 3,787 | (17 rebals) | — |
| hold 50/50 | -2,256 | 0 | 2,263 | — | — |
| **hard_exit_stop** | **-273** | **+1,982** | 274 | 0 | — |
| **hedged_narrow** | -1,502 | +753 | 1,525 | 0 | **+2,256** |

- The patient narrow band that **won the calm window is the worst in a crash** —
  stuck holding AERO 98% of the time, max drawdown ~45% of capital.
- **Chasing with rebalances does not help** (-3,758): it just realizes the loss
  step by step at ever-lower prices, paying gas/slippage on the way down.
- **One-way hard-exit caps the loss at -$273** (vs -$4,469): exit to WETH early,
  stop earning, wait. This is the single biggest lever for the down-tail.
- **The short hedge earns +$2,256**, almost exactly offsetting the inventory
  collapse; hedged net beats unhedged narrow by ~$2,250.

### Calm — fee density regime (for contrast)

| policy | net PnL | vs hold | max DD |
| --- | ---: | ---: | ---: |
| vol_scaled / narrow | +29 to +57 | +29 to +57 | ~21 |
| hard_exit_stop | +29 | +29 | 21 |
| hedged_narrow | +29 | +28 | **0.9** |

In calm markets the tail policies cost ~nothing: `hard_exit` never triggers, and
the hedge only shaves a little (funding) while crushing drawdown to ~$1.

### Chop — violent mean-reverting swings

Mechanical rebalancing is **ruinous**: `narrow_rebalance` did 1,323 rebalances and
lost ~100% of capital (the "疯狂rebalance" death spiral), while `narrow_static`
barely moved (it just waits out the oscillation). The hedge does **not** help chop
(it is a directional hedge; chop has no net direction).

## The decision matrix

| regime | best policy | why |
| --- | --- | --- |
| calm / mean-reverting | tight static / vol-scaled | fee density, no churn |
| one-way crash | **hard-exit + short hedge** | cap the tail, offset inventory |
| violent chop | static (do **not** rebalance) | rebalancing crystallizes loss |

The unifying lesson: **mechanical "rebalance on exit" is the worst rule in both
crash and chop.** What separates good from catastrophic is a *regime view* —
patient-static when the asset mean-reverts, hard-exit when it trends/crashes — plus
a short hedge to convert the down-tail from "falling knife" into "offset by perp".
This is the empirical backing for the earlier claim that high-fee pump/dump pools
should not be LP'd unhedged.

## The adaptive policy (`adaptive_regime`)

`RangeMode::Adaptive` operationalizes the matrix into one policy. Each swap it reads
a rolling regime from the tick path — trend strength = `|net move| / (vol·√window)`
— and acts:

- **trending into the risk asset** (tick rising, strength ≥ threshold): exit to the
  money leg and stand aside until the trend dies (the crash response);
- **trending to the money side**: follow with a vol-scaled recenter;
- **ranging** (low trend strength, incl. chop): **hold** — never chase wiggles.

Net PnL across the three synthetic regimes (±300 narrow floor, 3-block latency):

| policy | calm | crash | chop |
| --- | ---: | ---: | ---: |
| narrow_static | +29 | **-4,469** | +77 |
| narrow_rebalance | +29 | -3,758 | **-10,099** |
| **adaptive_regime** | **+56** | **-37** | -4,042 |

- **Calm: best** (+$56) — vol-scaled tight band captures more density than fixed ±300.
- **Crash: near-perfect** (-$37, +$2,219 vs hold) — detects the up-trend and exits
  early, even better than `hard_exit_stop` which waits for the range to break.
- **Chop: graceful** (-$4,042) — far better than mechanical rebalancing (-$10k)
  because it mostly holds; still trails pure static on this *pathological* synthetic
  (±6000 ticks every ~21 swaps), where occasional half-cycle misfires cost it. A
  longer window or higher threshold tames this; realistic chop is much milder.

The adaptive policy is the synthesis: it wins the two realistic regimes (calm,
crash) outright and degrades gracefully — not catastrophically — in extreme chop.
It is still unhedged; pairing it with the short hedge is the next obvious step.

## Caveats

- Scenario magnitudes are **stylized stress tests**, not calibrated to AERO's real
  distribution; read the rankings and mechanisms, not the absolute dollars (the
  chop amplitude in particular is deliberately extreme).
- Hedge is **static** (sized at entry); a dynamic delta hedge would track the
  growing exposure as price enters the danger zone but adds rehedge cost — next step.
- Slippage is a flat bps on rebalanced notional plus the latency mark; a real
  crash also collapses pool depth (worse fills) — not yet depth-aware.
- Still no reward-emissions income or LVR attribution (separate follow-ups).

## Reproduce

```bash
cargo run -p autopool-cli -- replay-scenario --scenario crash \
  --move-ticks 6000 --fee-bps 21.25 --token0-usd 1574 \
  --narrow-half-width 300 --action-delay-blocks 3 --funding-bps-per-day 10
```
