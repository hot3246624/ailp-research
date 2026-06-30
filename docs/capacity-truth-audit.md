# Capacity Truth Audit

Snapshot: 2026-06-30 CST. This note supersedes the overly broad reading that
"capacity is only $1k-$3k." That statement was true only for the specific
high-fee WETH-USDC 200 bps pool under a same-pool rebalance-impact gate. It is
not a valid claim about mainstream pools or the whole autoLP strategy.

## Correct Capacity Model

Separate three different capacities:

1. **LP mint capacity**: adding a concentrated position does not itself move price.
2. **Active-liquidity share**: how large our range liquidity is versus current
   in-range liquidity. If this is high, fee estimates stop scaling linearly and
   exit/rebalance risk rises.
3. **Rebalance execution capacity**: inventory conversion needed after a range
   exit. This can be done in the same pool, through a router, or through a hedge
   venue. Same-pool swap impact is a conservative local check, not a global proof
   of deployable capital.

The earlier no-go document collapsed #2 and #3 into one number. That made the
conclusion too pessimistic for deep mainstream pools.

## Tool Added

`capacity-truth-audit` reads live Slipstream pool state and reports, per capital
size:

- current active liquidity converted into the same target range (`act_cap`);
- our target position size versus active liquidity (`pos/act%`);
- one-sided token0 and token1 inventory conversion impact through the same pool;
- whether the same-pool rebalance impact passes the configured gate.

Example:

```bash
BASE_RPC_URL=... cargo run -q -p autopool-cli -- capacity-truth-audit \
  --half-width-ticks 600 \
  --capital-usd 10000 \
  --capital-usd 50000 \
  --capital-usd 100000 \
  --pool WETH-USDC-200bps:0x56aeaf4af2df4bdfd9d865830fefdd278b25e7ef \
  --pool WETH-USDC-4bps:0xb2cc224c1c9fee385f8ad6a55b4d94e92359dc59 \
  --pool WETH-USDC-2.3bps:0x3fe04a59ebd38cf06080a6f60a98d124eb59392a \
  --pool WETH-USDC-0.8bps:0x4e392fbfe4d0557c82d2f97f02ec39daa31516dd \
  --pool WETH-AERO-20.75bps:0x4e506648d493c8870f55e870480f92f2f33ece51
```

## Fresh Base Read

Half-width: `±600` ticks. Same-pool impact gate: `30 bps`. WETH/USD was inferred
from the live deep WETH-USDC reference pool. Fees are read live from each pool and
may differ from old labels.

| pool | live fee bps | capital | act cap | pos/act | token0 impact | token1 impact | gate |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| WETH-USDC high-fee | 200.00 | $10k | $102k | 9.82% | 48.77 bps | 47.26 bps | fail |
| WETH-USDC high-fee | 200.00 | $50k | $102k | 49.12% | 241.43 bps | 238.53 bps | fail |
| WETH-USDC high-fee | 200.00 | $100k | $102k | 98.24% | 476.92 bps | 482.75 bps | fail |
| WETH-USDC deep | 4.52 | $10k | $5.65m | 0.18% | 1.00 bps | 1.09 bps | pass |
| WETH-USDC deep | 4.52 | $50k | $5.65m | 0.88% | 5.00 bps | 5.45 bps | pass |
| WETH-USDC deep | 4.52 | $100k | $5.65m | 1.77% | 9.99 bps | 10.91 bps | pass |
| WETH-USDC very deep | 2.33 | $10k | $32.24m | 0.03% | 0.19 bps | 0.18 bps | pass |
| WETH-USDC very deep | 2.33 | $50k | $32.24m | 0.16% | 0.95 bps | 0.88 bps | pass |
| WETH-USDC very deep | 2.33 | $100k | $32.24m | 0.31% | 1.91 bps | 1.76 bps | pass |
| WETH-USDC thin low-fee | 0.80 | $10k | $79k | 12.71% | 74.95 bps | 75.32 bps | fail |
| WETH-AERO | 20.75 | $10k | $742k | 1.35% | 9.03 bps | 6.88 bps | pass |
| WETH-AERO | 20.75 | $50k | $742k | 6.74% | 45.07 bps | 34.46 bps | fail |
| WETH-AERO | 20.75 | $100k | $742k | 13.48% | 89.93 bps | 69.05 bps | fail |

## Interpretation

The user's objection is correct: mainstream pools can support far more than
`$1k-$3k`. The deep WETH-USDC pools pass the same-pool impact gate even at
`$100k`, and one variant shows active range capacity above `$30m`.

The real tradeoff is different:

- high-fee WETH-USDC has strong fee density but only about `$100k` active range
  capacity at this width, so even `$10k` one-sided same-pool rebalancing fails;
- deep WETH-USDC has excellent capacity but low fee bps, so it still needs a fresh
  fee/vol replay to prove net edge;
- WETH-AERO is in the middle: `$10k` is clean, `$50k+` needs external routing,
  wider ranges, slower turnover, or hedge inventory to avoid same-pool impact.

Therefore the current state is not "autoLP is dead." The corrected state is:

**thin high-APR pool hunting is not enough; the next strategy version must be
capacity-first on mainstream pools, then prove fee/vol edge after LVR, funding,
and routing costs.**

## APR Kill Gate

New business threshold: **do not advance a pool when credible current APR is below
`100%`**. Capacity is necessary but not sufficient. A deep pool with `10 bps` swap
impact and `10%-50%` APR is not worth the strategy stack.

Use the gate this way:

- reported or formula-implied organic fee APR must be at least `100%`;
- reward-heavy APR does not pass unless reward liquidation and emissions durability
  are modeled;
- replay/shadow promotion must still prove positive fee-minus-LVR after rebalance,
  hedge funding, routing, and operational costs;
- pools below `100%` may be kept only as control datasets, not strategy candidates.

Fresh Base/Aerodrome scan on 2026-06-30 with `tvl >= $100k` and `volume_1d >= $100k`
found **no strategy candidate above `100%` APR**. The highest listed row was
`EURC-USDC` at about `53.7%`; `WETH-AERO` was about `13.6%`; the WETH-USDC high-fee
row was `0%` APY and below the 1d volume threshold. Therefore the Base mainstream
capacity matrix should pause until a `>=100%` candidate appears.

## Next Research Direction

Stop optimizing the memecoin/DLMM strict-window scanner until historical active
liquidity is available. Move only `>=100%` APR candidates into a capacity-first
matrix:

1. Base/Aerodrome: WETH-USDC deep, WETH-AERO, AERO-USDC, cbBTC/USDC if available.
2. Solana: SOL-USDC and major quote-asset pools, but capacity must come from real
   active liquidity plus Jupiter route impact, not snapshot-only proxy APR.
3. For each pool, evaluate `$10k/$50k/$100k` at multiple widths (`±300`,
   `±600`, `±1200`, volatility-scaled).
4. Promotion gate should require positive rolling fee-minus-LVR, acceptable
   active-liquidity share, and routed rebalance impact below threshold.

The new go/no-go question is no longer "can we find a 2000% APR pool?" It is:

**Can a `>=100%` APR mainstream pool produce enough fee density at `$50k-$100k`
capacity after LVR, funding, and routed rebalance cost?**
