# First Data Analysis

## Sample Window

Source: checkpointed `Swap/Mint/Burn/Collect` backfill from Base / Aerodrome Slipstream.

- Blocks: `47791861..47806294`
- Time: `2026-06-25 15:37:49..23:38:55` CST
- Duration: about 8 hours
- RPC constraint: the free Alchemy endpoint requires `eth_getLogs` chunks of 10 blocks

Current network-cost snapshot during analysis:

- Block: `47806322`
- `eth_gasPrice`: `0.006` gwei
- 900k gas rough cost at ETH/USD 3400: about `$0.018`

This is only a rough L2 gas snapshot. The strategy cost model still needs transaction-receipt based costs before live execution.

## Pool Event Summary

| Pool | Events | Swaps | LP Events | Swaps / 1k Blocks | Tick Range | Tick p05 / p50 / p95 |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| USDC-USDT | 3,986 | 3,912 | 74 | 271.08 | 20..21 | 20 / 21 / 21 |
| EURC-USDC | 3,124 | 2,018 | 1,106 | 140.03 | 1256..1298 | 1259 / 1280 / 1294 |
| MSUSD-USDC | 2,999 | 2,731 | 268 | 194.54 | -276360..-276315 | -276359 / -276353 / -276332 |
| USDC-USDBC | 119 | 103 | 16 | 7.38 | 1..1 | 1 / 1 / 1 |

## Current Candidate Context

Current discovery and on-chain resolution ranked these stable-primary pools:

| Pool | TVL | 1d Volume | APY | Base APY | Reward Share | Fee | Tick Spacing |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| MSUSD-USDC | `$21.76m` | `$31.44m` | `28.10%` | `25.60%` | `8.90%` | `5.00 bps` | 50 |
| USDC-USDT | `$1.72m` | `$9.35m` | `20.74%` | `18.26%` | `11.93%` | `0.09 bps` | 1 |
| EURC-USDC | `$2.05m` | `$4.92m` | `41.97%` | `31.58%` | `24.75%` | `0.85 bps` | 50 |
| USDC-USDBC | `$0.51m` | `$0.36m` | `2.55%` | `2.34%` | `8.26%` | `1.00 bps` | 1 |

## Interpretation

USDC-USDT is the cleanest first replay target. It has the highest swap density in the sample, very low tick movement, low LP churn, and tick spacing of 1. This makes it a good environment for isolating the range-width and rebalance-threshold problem without immediately mixing in large tick jumps.

EURC-USDC is attractive by headline and base APY, but it is not the cleanest first target. Its tick span is 42 over the sample, close to one 50-tick spacing band, and LP events are unusually high relative to swaps. That makes it useful as a second stress case for liquidity churn, reward durability, and range edge behavior.

MSUSD-USDC has strong volume and swap density, but tick spacing is 50 and the observed tick span is 45. A one-spacing narrow range can spend meaningful time near an edge. It also needs token-specific risk review before being treated like a plain USDC-USDT stable pair.

USDC-USDBC is too quiet for the first strategy loop. It can stay as a low-volatility control, but it is unlikely to prove fee-density edge quickly.

## Strategy Consequences

The first AILP policy should not try to maximize APR yet. It should answer this narrower question:

```text
For USDC-USDT, what fixed or adaptive tick width beats passive LP after gas,
inventory drift, and out-of-range opportunity cost?
```

Required next measurements:

- fee growth deltas per replay window
- tick occupancy by candidate range
- liquidity distribution near current tick
- one-sided inventory after range exits
- actual transaction-cost estimates from Base receipts
- baseline comparisons: hold, passive wide range, fixed narrow range, volatility-scaled range

The first backtest target should use USDC-USDT, with EURC-USDC held back as the first robustness case once fee-growth replay works.
