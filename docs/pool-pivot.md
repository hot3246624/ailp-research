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

## Conclusion

For Base / Aerodrome Slipstream, active concentrated LP is structurally thin-margin:
its active pools are all ≤ ~21 bps, where alpha is either negative (5 bps) or
churn-eaten (21 bps). The single high-fee/volatile exception is **CTR-USDC (100 bps)**
— the one place net alpha might survive, and the clear next test. If CTR-USDC also
fails, the rigorous conclusion is that this venue does not support active LP and the
search should move to higher-fee-density venues/chains (or settle for passive-wide,
which at least beat hold on ~72–86% of paths).
