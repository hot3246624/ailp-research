# Pool Pivot: Fee Density Is the Binding Constraint

The multi-path result showed WETH-AERO (21 bps) has positive gross alpha (fee − LVR)
but churn eats the net. The natural question: is there a Base / Aerodrome Slipstream
pool where fee density is high enough — on an *active* pool — for net alpha to
survive? A robust on-chain scan plus a fee-tier contrast answer it.

## Robust activity scan (4,000 blocks via `mainnet.base.org`)

| pool | fee bps | tvl | swaps/kblk | tick_vol | score |
| --- | ---: | ---: | ---: | ---: | ---: |
| OFC-USDC | 5 | 277k | 42.5 | **7.69** | 50.1 |
| USDC-AERO | 5 | 541k | 56.2 | 2.83 | 21.3 |
| WETH-AERO | 20.5 | 2.04m | 15.5 | 2.28 | 9.0 |
| **CTR-USDC** | **100** | 298k | 2.0 | 4.58 | 6.5 |
| USDC-CBBTC | 15 | 1.05m | 4.5 | 0.97 | 2.1 |
| MSUSD-MSETH | 25 | 1.10m | 0.0 | 0.0 | 0.0 |
| WETH-SERV | 150 | 861k | 1.5 | ~0 | 0.0 |
| WETH-LCAP | 30 | 360k | 0.0 | 0.0 | 0.0 |

**The pattern is the whole story:** the active, volatile pools are *low-fee*
(OFC/USDC-AERO at 5 bps, WETH-AERO at 20 bps); the *high-fee* pools (CTR 100, SERV
150, LCAP 30, MSUSD-MSETH 25) are nearly inactive. No pool offers both high fee
*and* high activity.

## The fee-density threshold (gross alpha = fee − LVR)

Multi-path (demeaned/martingale), per-policy gross alpha:

| pool | fee bps | narrow fee − LVR | verdict |
| --- | ---: | ---: | --- |
| USDC-AERO | 5 | **negative** (−2 to −10) | LVR exceeds fees — *no alpha at all* |
| WETH-AERO | 21 | **positive** (+$266 calm gross) | alpha exists, but churn eats the net |

So there is a clear fee-density threshold:

- **≤ ~5 bps**: fees do not even cover adverse selection (fee − LVR < 0). Providing
  liquidity is a guaranteed bleed regardless of policy. Don't LP.
- **~20 bps**: gross alpha turns positive, but on-chain rebalance churn consumes the
  net (see `docs/multi-path.md`).
- **higher bps**: needed for net alpha to clear churn. Fee scales linearly with the
  tier; LVR scales with volatility². So the fee/LVR ratio improves ~linearly with
  the fee tier at equal volatility.

## The one candidate worth a dedicated test: CTR-USDC (100 bps)

CTR-USDC is the only Slipstream pool with both a **high fee (100 bps, 20× WETH-AERO)**
and **real volatility** (tick_vol 4.58). Its fee/LVR ratio should be roughly 5×
WETH-AERO's — plausibly enough for net alpha to survive churn even after costs. Its
weakness is **low activity** (~2 swaps/kblk), so total fee *throughput* is small;
whether the high per-swap fee compensates is exactly what a replay must decide.

Targeted next experiment:

1. Resolve CTR-USDC and confirm token ordering/decimals. If USDC is token1 (not
   token0) the replay numeraire and the `risk_asset_is_token1` assumption must be
   flipped — handle before trusting the numbers.
2. Collect a large window via the public RPC (`--swaps-only`, 2000-block chunks):
   at ~2 swaps/kblk a few hundred swaps needs ~150–200k blocks.
3. Replay + multi-path: does `fee − LVR` stay large *and* net beat hold after churn?

## CTR-USDC (100 bps) result

Collected 322 swaps over ~200k blocks (token0 = CTR 18dec, token1 = USDC 6dec, so
replayed with `--invert`: USDC becomes the token0 numeraire, CTR the risk token1).

**Gross alpha is large and positive at 100 bps.** Single window (a CTR down-trend):
narrow_rebalance fee − LVR = **+$178** on 322 swaps = **$0.55/swap**, ~6× WETH-AERO's
$0.09/swap. So the fee tier does exactly what the threshold analysis predicted — it
lifts gross alpha well above the LVR bleed.

Demeaned multi-path (200 paths, martingale), net PnL:

| policy | mean net | std | fee − LVR | win% hold |
| --- | ---: | ---: | ---: | ---: |
| **passive_wide** | **+116** | 229 | +23 | **83%** |
| hold_50_50 | +103 | 236 | 0 | — |
| narrow_static | +1 | 220 | +42 | 37% |
| hard_exit_stop | -47 | 115 | +85 | 23% |
| narrow_rebalance | -146 | 211 | **+179** | 1% |
| delta_hedged | -204 | **74** | +179 | 15% |

Two findings:

1. **Higher fee does NOT rescue narrow rebalancing.** Despite the largest gross alpha
   yet (fee − LVR +$179), narrow_rebalance still net-loses (−$146): CTR is *more
   volatile* (tick_vol 4.58), so the ±100 band is crossed even more often and the
   rebalance churn grows with volatility. High-fee pools are high-vol, so churn
   tracks the fee — the rebalancing approach loses at every tier.
2. **Higher fee DOES pay for low-churn LP.** `passive_wide` has positive expectancy
   (+$116) and beats hold on **83%** of paths — up from +$10 / 72% on WETH-AERO
   (21 bps). `narrow_static` rises from −$98 (WETH-AERO) to ~breakeven. Low-churn LP
   improves materially with fee density.

## Final synthesis

Across 5 / 21 / 100 bps, three pools, single windows and bootstrapped distributions:

- **Active narrow *rebalancing* never wins.** On-chain churn cost scales with
  volatility, and so does fee, so churn eats the gross alpha at every fee tier
  (5 bps: no alpha; 21 bps & 100 bps: alpha exists but churn-eaten).
- **The deployable edge is low-churn LP on a high-fee-density active pool.** A wide,
  rarely-rebalanced band (`passive_wide`) is the only consistently
  positive-expectancy policy, and it pays materially better as fee density rises
  (+$10/72% at 21 bps → +$116/83% at 100 bps).
- **Hedging is for variance/tail, not mean.** `delta_hedged` always has the lowest
  variance/drawdown but inherits the churn of whatever range policy it wraps; pair it
  with a *wide* band, not a narrow one, to get low-churn + low-beta + fee harvest.

So the recommended live shape is: **CTR-USDC-style (highest-fee active pool) + a wide
band + minimal rebalancing + optional delta hedge for variance.** The next build is a
*delta-hedged passive-wide* policy and a scan for more 100 bps-class active pools.

## The deployable shape: `hedged_wide` (built)

`RangeMode::HedgedWide` = a wide, never-rebalanced band (passive-wide's low churn and
fee harvest) wrapped in the dynamic delta hedge (kills the inventory beta a wide band
still carries). Multi-path on CTR-USDC:

**Demeaned (martingale)** — isolates variance:

| policy | mean | std | meanDD |
| --- | ---: | ---: | ---: |
| hold_50_50 | +133 | 653 | 702 |
| passive_wide | +157 | 599 | 675 |
| **hedged_wide** | −3 | **69** | **78** |

**Raw (the real CTR down-crash)** — the killer test:

| policy | mean | std | win% hold | meanDD |
| --- | ---: | ---: | ---: | ---: |
| hold_50_50 | −537 | 568 | — | 989 |
| passive_wide | −567 | 675 | 56% | 1077 |
| **hedged_wide** | **−15** | **57** | **85%** | **77** |

`hedged_wide` is **direction-robust**: ~flat and tight whether the asset crashes or
rallies, with ~10× less variance and drawdown than hold/passive-wide. On the real CTR
crash it lost **$15 vs hold's $537** and beat hold on **85%** of paths.

**Honest read:** its *mean* edge is small (in the driftless world fee-alpha barely
covers funding + hedge cost at this pool, ≈ 0). Its value is **risk**: it removes the
±$500 directional gamble that naked LP/hold carry, turning LP into a tight,
direction-neutral, crash-proof position. For a risk-managed LP with no directional
view that is the right shape. To make the *mean* clearly positive you need fee-alpha
above hedge cost — i.e. higher fee density and/or cheaper funding than CTR-USDC's
100 bps / 10 bps-day. That is the remaining lever, and the reason to keep scanning for
even higher-fee active pools.
