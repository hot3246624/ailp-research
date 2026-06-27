# Pool Discovery: Finding Liquid AND Volatile Pairs

All the real data collected so far (WETH-AERO) is low-volatility, so the tail-aware
policies (hard-exit, hedge, adaptive) have not been exercised on a real move. To
close that gap we need pools that are both **liquid** (executable) and **volatile**
(price actually moves), which is where active range management has edge.

DeFiLlama is not enough: its Slipstream coverage is thin, its `volumeUsd1d` is often
`0` for these pools, and its `sigma` is APY volatility, not price volatility. The
real signal is on-chain.

## `scan-pool-activity`

Resolves the candidate universe on-chain and measures **realized tick volatility**
(stdev of consecutive swap-to-swap tick changes) plus swap density over a recent
window, then ranks by `activity_score = tick_vol · √(swaps_per_kblock)`.

```bash
BASE_RPC_URL=... cargo run -p autopool-cli -- scan-pool-activity \
  --min-tvl-usd 300000 --limit 8 --lookback-blocks 1000 --sleep-ms 250
```

It fetches swaps one chunk at a time with an inter-chunk sleep, and the RPC client
retries 429/5xx with exponential backoff, so the scan coexists with the running
backfills on a single free RPC endpoint.

## First scan (400-block window)

| symbol | fee_bps | tvl_usd | swaps | swp/kblk | tick_span | tick_vol | score |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| **USDC-AERO** | 5.00 | 546,180 | 9 | 22.5 | 10 | **2.74** | **12.98** |
| WETH-AERO | 20.75 | 2,489,475 | 7 | 17.5 | 3 | 0.76 | 3.20 |
| MSUSD-MSETH | 25.00 | 1,191,830 | 1 | 2.5 | 0 | 0.00 | 0.00 |
| USDC-CBBTC | 15.00 | 942,327 | 0 | — | — | — | — |
| WETH-SERV | 150.00 | 862,984 | 0 | — | — | — | — |
| WETH-LCAP | 30.00 | 359,048 | 0 | — | — | — | — |

(400 blocks ≈ 13 min — too short for robust vol; rankings are directional.)

### Reading

- **USDC-AERO is the most active liquid pool** — highest realized vol *and* highest
  swap density of the liquid set, ~1.9× daily turnover. And it is a **clean
  USD-numeraire pool** (token0 = USDC), so the replay needs no WETH/USD anchor guess.
  It is a better primary research target than WETH-AERO.
- High-fee long-tail pairs exist (**WETH-SERV at 150 bps**, WETH-LCAP at 30 bps) —
  the market pricing high volatility/toxicity — but they are quiet in short windows
  and need a longer sample (or a volatile episode) to evaluate.
- The broad lesson stands: the AERO ecosystem is in a **calm regime** right now;
  even the "volatile" pairs show small tick spans. A real test of the down-tail
  machinery requires either waiting for a move or backfilling a historical crash.

## Action taken

Started a backfill of **USDC-AERO `0xa4fdd479eda160671636e2ecf8f993cbf86258a8`**
(Initial / spacing-100 / 5 bps; token0 = USDC dec 6, token1 = AERO dec 18) into
`data/base/aerodrome-opportunistic`. Replay it with `--token0-usd 1 --decimals0 6
--decimals1 18 --fee-bps 5` — no price-anchor assumption needed.

## Finding a volatile window in history

`scan-pool-activity` measures *current* activity. To find a past volatile window we
coarsely sample the pool's tick back through history (one swap per sample point) and
look for the largest tick moves. Probing WETH-AERO over the last 400k blocks
(~9 days):

```
block~47477149 tick=82811     ... 47527149 tick=82725 ... 47877149 tick=80953
MOST VOLATILE SEGMENT: 47527149..47552149  dtick=-1224 (AERO +13.0% in ~14h)
sustained: 47527149..47627159  tick 82725 -> 80649  (AERO +~21% over ~2.3 days)
```

So AERO has been **trending up with chop** — no dramatic crash in recent history,
but a real ~13–21% trending/volatile regime, far more than the calm slice we had.

## Collecting a historical window

`backfill-slipstream-events` now takes `--from-block/--to-block` (fixed window,
terminates when done) and `--swaps-only` (Swap events only, 4x cheaper — replay
only needs swaps). Use a separate `--data-dir` so checkpoints don't collide with
live backfills:

```bash
BASE_RPC_URL=... cargo run -p autopool-cli -- backfill-slipstream-events \
  --data-dir data/base/aerodrome-history \
  --pool WETH-AERO:0x4e506648d493c8870f55e870480f92f2f33ece51 \
  --swaps-only --from-block 47530000 --to-block 47550000 \
  --max-blocks-per-run 200 --log-chunk-blocks 10 --sleep-ms 300 --iterations 0
```

A backfill of the steepest segment (47530000..47550000, AERO ~+13%) is running into
`data/base/aerodrome-history`; replay it to test the adaptive policy's trend
detection and the hard-exit/hedge tail behaviour on real (not synthetic) movement.

## Next

- Replay the collected historical window; compare adaptive / hard-exit / hedged on
  real trending data, and re-run walk-forward across the calm→trend boundary.
- Re-run `scan-pool-activity` over a longer window for a robust vol ranking once RPC
  budget allows, or on a paid/archive endpoint.
- Widen discovery beyond Aerodrome Slipstream (Base majors: WETH-BRETT, WETH-DEGEN,
  WETH-VIRTUAL — see https://defillama.com/yields?chain=Base) once a non-Slipstream
  pool reader exists.
