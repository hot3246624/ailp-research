# Solana APR>=100 Stage Result

Snapshot: 2026-06-30 CST.

Business gate: do not advance pools with credible current APR below `100%`.
This note records the first bounded Solana pass after that gate was made
explicit. It is a stage result, not a deployment approval.

## Current Candidate Queue

Fresh protocol/API candidate refresh still points to Solana, not Base:

| rank | venue | pool | fee APR 24h | fee APR 7d | TVL | 24h volume | status |
| ---: | --- | --- | ---: | ---: | ---: | ---: | --- |
| 1 | Meteora DLMM | SOL-USDC 4 bps | 767.0% | n/a | $3.34m | $47.9m | P1 only; API APR disagrees with simple fee turnover |
| 2 | Orca Whirlpool | SOL-USDC 4 bps | 129.2% | ~93% prior read | $24.3m | $215.1m | P0 replay candidate; capacity/blue-chip target |
| 3 | Meteora DLMM | JUP-USDC 10 bps | 553.6% | n/a | $0.90m | $5.0m | P1 only; historical active-bin state unresolved |
| 4 | Raydium CLMM | SNDK-USDC 10 bps | 304.4% | 79.4% | $0.40m | $3.30m | P0 quick screen; rejected |
| 5 | Raydium CLMM | CARDS-USDC 40 bps | 262.5% | 168.5% | $3.31m | $5.96m | P0 replay; watchlist only after mixed evidence |

Meteora remains interesting, but still cannot be promoted from API APR alone:
the replay-grade blocker is historical active-bin liquidity/account state.

## Orca SOL-USDC

Pool: `Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE`.

Fresh bounded scan:

- scanned `773` signatures;
- kept `8` swaps;
- tx errors `0`;
- normalized rows `8`;
- oldest kept slot `429795756`.

This is not enough for rolling windows. It does not reject the pool by itself,
but it means the current address-signature sampling route is too sparse for a
fresh near-term replay decision.

The prior `187`-row replay remains the current decision evidence:

| window | windows | win vs hold | mean net | mean vs hold | p05 net APR | verdict |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| 25:10 | 16 | 44% | $0.03 | $0.33 | -991% | reject |
| 40:15 | 9 | 44% | $0.03 | $1.09 | -680% | reject |
| 60:20 | 6 | 67% | $0.05 | $2.18 | -309% | reject |
| 80:25 | 4 | 75% | $0.06 | $4.48 | 18% | reject |

Interpretation: Orca SOL-USDC is the right kind of mainstream/capacity pool to
watch, but it is not strategy-approved. The failure is not headline APR; it is
rolling left-tail fee-minus-LVR stability.

## Raydium CARDS-USDC

Pool: `HnhpJPJgBG2KwniMTNW8cVBHvk1hFog3RC3kjnyc23tD`.

Fresh bounded scan:

- scanned `448` signatures;
- kept `115` swaps;
- tx errors `0`;
- slot span `429790847..429795902`;
- replay wall-clock span about `33.7` minutes.

Fresh full-window replay was strong:

| policy | net PnL | vs hold | fees | fee-LVR | mechanical net APR | max DD |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| narrow_static | $8.22 | $18.82 | $19.16 | $13.59 | 1282% | $30.67 |
| vol_scaled_rebalance | $25.08 | $35.68 | $36.36 | $25.26 | 3912% | $25.46 |
| delta_hedged | $18.78 | $29.38 | $19.16 | $13.59 | 2930% | $0.01 |
| hedged_wide | $2.10 | $12.70 | $2.14 | $1.55 | 328% | $0.00 |

Fresh rolling gate was close, but not a pass:

| policy | window | windows | win vs hold | mean net | mean vs hold | p05 net APR | verdict |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| lagged_regime_rule | 25:10 | 9 | 56% | $3.98 | $9.41 | 610% | fail: win rate |
| lagged_regime_rule | 40:15 | 5 | 80% | $8.18 | $15.54 | 1896% | pass |
| lagged_regime_rule | 60:20 | 2 | 100% | $14.44 | $25.12 | 2389% | fail: too few windows |
| delta_hedged | 25:10 | 10 | 50% | $3.63 | $8.21 | 610% | fail: win rate |
| delta_hedged | 40:15 | 6 | 67% | $7.04 | $10.91 | 1896% | pass |
| delta_hedged | 60:20 | 3 | 67% | $10.16 | $11.58 | 1606% | pass |

Then the fresh `115` rows were merged with the previous `201` replay rows. The
merged stream had `316` unique rows over `429588129..429795902`. This cross-regime
test rejected the pool:

| policy | window | windows | win vs hold | mean net | mean vs hold | p05 net APR | worst DD | verdict |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| delta_hedged | 25:10 | 30 | 63% | -$33.83 | $19.15 | -1396% | $473.67 | reject |
| delta_hedged | 40:15 | 19 | 63% | -$42.44 | $42.51 | -1346% | $322.86 | reject |
| delta_hedged | 60:20 | 13 | 62% | -$75.09 | $50.88 | -1985% | $473.67 | reject |
| delta_hedged | 80:25 | 10 | 70% | -$109.58 | $110.52 | -1912% | $473.67 | reject |
| lagged_regime_rule | 25:10 | 29 | 66% | -$44.65 | $8.59 | -2090% | $507.68 | reject |
| lagged_regime_rule | 80:25 | 9 | 89% | -$212.13 | $32.25 | -2164% | $535.50 | reject |

Interpretation: CARDS has a real short-burst fee edge signal, especially with
delta hedging, but the current strategy does not survive a broader regime replay.
It is a watchlist/research candidate, not a deployable strategy.

## Raydium SNDK-USDC

Pool: `4vRC6Qne8HPUN98mJr88vRkRD9N4cadyrGtPwVU3CV86`.

Fresh bounded scan:

- scanned `397` signatures;
- kept `68` swaps;
- tx errors `1`;
- slot span `429792308..429797343`;
- replay wall-clock span about `33.6` minutes.

Reward APR was explicitly set to zero in replay. Only organic fee APR was counted.

Full-window replay showed that most visible profit was just holding the token
during an up move, not LP fee alpha:

| policy | net PnL | vs hold | fees | fee-LVR | mechanical net APR | max DD |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| hold_50_50 | $21.42 | $0.00 | $0.00 | $0.00 | 3354% | $5.20 |
| narrow_static | $21.03 | -$0.39 | $1.19 | $0.80 | 3293% | $4.21 |
| vol_scaled_rebalance | $20.39 | -$1.03 | $2.11 | $1.35 | 3192% | $3.25 |
| delta_hedged | -$0.35 | -$21.77 | $1.19 | $0.80 | -55% | $1.59 |
| hedged_wide | -$0.02 | -$21.44 | $0.14 | $0.10 | -3% | $0.16 |

Rolling gate rejected:

| policy | window | windows | win vs hold | mean net | mean vs hold | p05 net APR | verdict |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| delta_hedged | 25:10 | 5 | 0% | $0.26 | -$6.29 | 62% | reject |
| delta_hedged | 40:15 | 2 | 0% | $0.06 | -$12.92 | -16% | reject |
| lagged_regime_rule | 25:10 | 4 | 0% | $0.27 | -$6.31 | 62% | reject |

Interpretation: SNDK should be dropped unless a materially different regime appears.
It is a high-current-APR pool where the LP strategy is not beating hold/passive
exposure in the available sample.

## Stage Decision

No strategy is finalized.

The original idea is not disproved in principle. The current evidence says the
profitable version must satisfy all of these at the same time:

1. credible organic fee APR `>=100%`;
2. enough active/routed capacity at the intended capital size;
3. positive rolling fee-minus-LVR after hedging/rebalance cost;
4. not just "token went up while we happened to LP";
5. no left-tail windows that erase several positive bursts.

Current state:

- **SNDK-USDC**: reject.
- **Orca SOL-USDC**: not rejected by fresh sample, but not promoted; current sampler
  too sparse and prior replay fails left-tail gate.
- **CARDS-USDC**: best signal; fresh burst is attractive, merged cross-regime replay
  rejects. Keep on watchlist, do not trade.

Next bounded step, if continuing:

1. improve mainstream Solana sampling for Orca SOL-USDC / SOL-HYPE so high-capacity
   pools produce dense replay rows;
2. add a platform-detection / stand-down rule before re-entering CARDS-like pools,
   because cross-regime drift is what killed the merged replay;
3. do not spend time on pools whose credible fee APR is below `100%`;
4. do not promote Meteora API APR until historical active-bin liquidity is solved.

## Follow-up: Orca Resample and Stand-down Filter

Follow-up snapshot: 2026-06-30 CST.

The next bounded push tested two hypotheses:

1. the prior Orca `SOL-USDC` fresh sample was sparse because the sampler was broken;
2. CARDS-like pools can be rescued by a no-lookahead platform filter that stands
   down after trend/volatile windows and only re-enters after a prior range window.

### Orca SOL-USDC Resample

A direct diagnostic of the newest `200` pool-address signatures showed:

- `33` successful transactions;
- `25` Whirlpool swap logs among successful transactions;
- token balance owner deltas correctly included the pool address.

So the parser was not structurally broken. The previous `8`-row fresh sample was a
bad/quiet slice with many failed transactions and few successful swaps.

Rerunning the sampler later produced a valid current burst:

- scanned `468` successful signatures;
- kept `97` swaps;
- tx errors `0`;
- slot span `429799968..429800955`;
- replay wall-clock span about `6.6` minutes.

The current burst still rejected:

| policy | net PnL | vs hold | fees | fee-LVR | mechanical net APR | max DD |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| delta_hedged | -$0.12 | $8.08 | $0.11 | $0.07 | -92% | $0.14 |
| hedged_wide | -$0.01 | $8.18 | $0.01 | $0.01 | -10% | $0.02 |
| vol_scaled_rebalance | -$8.43 | -$0.23 | $0.22 | $0.15 | -6731% | $8.44 |

Rolling delta-hedged gate:

| window | windows | win vs hold | mean net | mean vs hold | p05 net APR | verdict |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| 25:10 | 8 | 100% | $0.01 | $2.09 | -107% | reject |
| 40:15 | 4 | 100% | $0.00 | $2.95 | -36% | reject |
| 60:20 | 2 | 100% | -$0.01 | $4.25 | -23% | reject |

Merging the old `187` rows with the new `97` rows produced `284` unique rows and a
stronger rejection:

| window | windows | win vs hold | mean net | mean vs hold | p05 net APR | verdict |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| 25:10 | 26 | 54% | -$2.15 | -$8.33 | -926% | reject |
| 40:15 | 17 | 47% | -$5.07 | -$19.64 | -680% | reject |
| 60:20 | 12 | 50% | -$7.15 | -$27.57 | -309% | reject |
| 80:25 | 9 | 44% | -$9.57 | -$36.60 | -198% | reject |

Decision: **Orca SOL-USDC stays unpromoted.** It has capacity and credible
`>=100%` 24h APR, but the current fee density is not enough after rolling
fee-minus-LVR and hold comparison.

### Stand-down Cash Filter

The CLI now supports `stand-down-cash` inside `replay-promotion-gate
--gate-policy lagged-policy-switch`. It is a strict no-lookahead window control:
use the previous window's regime, and if the rule selects `stand_down_cash`, model
the next window as holding cash with:

- net PnL `0`;
- fees `0`;
- drawdown `0`;
- `vs_hold = -hold_50_50_window_pnl`.

This intentionally keeps the benchmark hard. Standing down only helps if avoiding
inventory beats holding the underlying tokens.

CARDS merged sample (`316` rows) results:

| rule | window | windows | win vs hold | mean net | mean vs hold | p05 net APR | worst DD | verdict |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| range=delta, others=stand_down | 25:10 | 29 | 62% | -$8.21 | $45.02 | 0% | $319.01 | reject |
| range=delta, others=stand_down | 40:15 | 18 | 67% | -$9.80 | $80.33 | -1128% | $275.65 | reject |
| range=delta, others=stand_down | 60:20 | 12 | 58% | $4.61 | $142.23 | 0% | $10.91 | reject |
| range=delta, others=stand_down | 80:25 | 9 | 67% | -$17.03 | $227.34 | -804% | $202.11 | reject |
| all=stand_down | 40:15 | 18 | 61% | $0.00 | $90.13 | 0% | $0.00 | reject |

Decision: **simple platform filtering is not enough.** It reduces drawdown, but it
also converts left-tail APR to `0%` or leaves negative windows from delayed
classification. That does not satisfy the `>=500%` promotion tail gate and does
not meet the user's `APR>=100%` continuation rule.

### Updated Stage Decision

The latest push makes the state sharper:

- `Orca SOL-USDC`: reject for now despite capacity.
- `SNDK-USDC`: reject.
- `CARDS-USDC`: short-burst edge exists, but merged regime and stand-down filters
  reject.

The next useful work is not more small parameter tuning. It is either:

1. solve Meteora historical active-bin replay for a genuinely high fee-density
   venue; or
2. build a different CARDS-style entry signal that predicts the burst before the
   adverse platform migration, not after it.

Until then, there is still **no deployable AILP strategy**.
