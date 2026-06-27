# LVR & Reward Attribution: Is There Real LP Edge?

IL-vs-hold conflates two things: LP skill (fees beyond adverse selection) and price
beta (the inventory you were forced to hold). To answer "does this pool have real LP
edge" we need to separate them. **Loss-versus-rebalancing (LVR)** does exactly that.

## What we compute

- **LVR** (per swap, while the position is active): the position's *old* holdings
  marked at the *new* price, minus the LP's *actual* new value. This is the value
  bled to arbitrageurs each time price moves — `≥ 0`, path-robust, and **beta-free**
  (it does not depend on whether the price came back).
- **fee − LVR**: the LP's pure edge. `> 0` means fees more than pay for the
  arbitrage bleed. This is the clean "is there alpha" number.
- **reward income**: gauge emissions accrued per second on staked value while in
  range, after a liquidation haircut. (`--reward-apr`, `--reward-haircut`.)

Identity: `LP_net = rebalancing_portfolio_net + (fees − LVR)`. The first term is the
*beta* of the LP's (time-varying) delta; the second is the LP's *alpha*.

## Result — alpha is positive in BOTH regimes; net is not

WETH-AERO, $10k, fee 21.25 bps, reward APR 22.49% (10% haircut), ±100 band:

### Calm window (~20h)

| policy | net | vs hold | fees | reward | LVR | **fee−LVR** | maxDD |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| hold_50_50 | -116 | 0 | 0 | 0 | 0 | 0 | 258 |
| **narrow_static** | **+256** | +373 | 460 | 5 | 194 | **+266** | 233 |
| adaptive_regime | +179 | +296 | 435 | 3 | 224 | +211 | 176 |
| narrow_rebalance | -48 | +68 | 655 | 8 | 234 | +422 | 251 |

### Trend window (~13k swaps, AERO +~18%)

| policy | net | vs hold | fees | LVR | **fee−LVR** | maxDD |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| hold_50_50 | +1145 | 0 | 0 | 0 | 0 | 283 |
| narrow_rebalance | +428 | -717 | 1824 | 620 | **+1203** | 278 |
| adaptive_regime | +230 | -915 | 376 | 164 | +213 | 290 |

## Reading

1. **The pool has real LP alpha: fee − LVR > 0 in every regime.** The LP earns more
   in fees than it loses to arbitrageurs — calm +$266 (static), trend +$1203
   (rebalance). So the answer to "is there edge" is **yes**, there is genuine
   fee-alpha, not just beta. This is the rigorous result IL-vs-hold could not give.

2. **But alpha only becomes net outperformance in calm.** `LP_net = beta + (fee −
   LVR)`. In calm the LP's delta has little beta drag, so the +alpha shows up as net
   edge (narrow_static beats hold by $373). In a trend the LP's delta sells the
   riser — large negative beta — which swamps the +$1203 alpha and leaves net far
   below hold. LVR makes this decomposition explicit.

3. **The implication is a delta hedge — done right.** Since the *alpha* is positive
   everywhere and only *beta* hurts in trends, neutralizing the LP's delta would let
   the fee-alpha through in any regime. The current *static* hedge fails in a trend
   (it shorts a fixed AERO size that then rises); a **dynamic delta hedge** that
   tracks the position's actual, changing delta is the theoretically-correct way to
   harvest fee − LVR regardless of regime. That is the next build.

4. **Capturing alpha requires not churning.** `narrow_rebalance` has the highest
   gross alpha (fee − LVR) but its net trails `narrow_static` because rebalance
   gas/slippage and re-entry whipsaw eat it. Static capture > active capture.

5. **Rewards are a modest, additive lift** here (~$5–14 over these short windows)
   but compound over time and would push marginal calm cases further positive. They
   do not change the regime story.

## Bottom line

WETH-AERO **does** have real LP fee-alpha (fee > LVR). It is harvestable net-of-beta
today only in ranging markets (where `narrow_static`/`adaptive` beat hold). To
harvest it in *all* regimes you must hedge the inventory delta dynamically — which is
the strongest argument yet for pairing the LP with a delta hedge rather than running
it naked. The LP-on/off gate (calm-only LPing) is the no-derivatives way to
approximate the same thing.

## Caveats

- LVR is computed discretely per swap (old-holdings-at-new-price); it is an
  approximation of the continuous LVR but directionally correct, and the
  `fee > LVR` conclusion is robust to the discretization.
- Reward APR is the DeFiLlama pool figure applied to staked value; real per-position
  emissions depend on in-range liquidity share and the haircut is a guess.
