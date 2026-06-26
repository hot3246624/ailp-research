# WETH-AERO Replay: First Range-Policy Results

First run of the concentrated-liquidity replay engine (`crates/backtest/replay.rs`,
`autopool-cli replay-events`) against real Base / Aerodrome Slipstream swap data.

This is the first time the project turns collected swaps into LP profit-and-loss
instead of just counting events. It implements architecture Milestone 3 (Strategy
Research Environment) and starts answering the pilot's target question:

> For high-APR volatile pools, what fixed or adaptive tick width beats passive LP
> after gas, inventory drift, and out-of-range opportunity cost?

## Dataset

- Pool: `WETH-AERO` `0x4e506648d493c8870f55e870480f92f2f33ece51` (Slipstream GaugesV3)
- Fee tier: `fee() = 2125` → **21.25 bps**; tick spacing **200**
- Swaps replayed: **3,817**
- Block window: `47815437..47851485` (~36k blocks ≈ 20h on Base)
- Tick path: `80983 → 80889`, and the position never left a `±600` band of entry,
  i.e. **max price excursion < ~6%**. This is a **calm, mean-reverting window.**

WETH/USD anchor: the active WETH-USDC pools all sit at tick ≈ `-202700`, which with
WETH(18)/USDC(6) decimals implies **WETH ≈ $1,574** in this environment. USD figures
below use that anchor. (The USD anchor only linearly scales USD outputs; policy
*rankings* are anchor-invariant.)

## Method

Per swap, a position earns
`fee_fraction * gross_input * L / (L_active + L)` when the pre-swap price is inside
its range. `L_active` is the pool liquidity reported in the swap event; the
position's `L` is solved from capital using the standard v3 closed form. Inventory,
IL-vs-hold, and final value are closed-form functions of `L`, the range, and the
final price. Token0 (WETH) is the numeraire, so AERO is marked at the pool's own
price — which is exactly where adverse inventory drift shows up.

Baselines: `hold_50_50`, `passive_wide` (±6000), `narrow_static` (no rebalance),
`narrow_rebalance` (recenter on exit), `vol_scaled_rebalance` (half-width from
realized tick vol). Capital $10k, gas $0.05/rebalance, 5 bps rebalance slippage on
half the book.

## Result: tightest patient band wins; rebalancing is a tax

Width sweep, `narrow_half_width` ∈ {100, 200, 400, …}, WETH=$1,574:

| half-width | policy | net PnL | vs hold | fees | IL | gas+slip | in-range | rebals |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 100 | hold_50_50 | 38.64 | 0.00 | 0.00 | 0.00 | 0.00 | 0% | 0 |
| 100 | passive_wide | 48.64 | 9.99 | 10.28 | 0.29 | 0.00 | 100% | 0 |
| 100 | **narrow_static** | **338.33** | **299.69** | 314.46 | 14.91 | 0.00 | 67.5% | 0 |
| 100 | narrow_rebalance | 87.78 | 49.14 | 470.60 | 16.63 | 30.04 | 100% | 12 |
| 100 | vol_scaled_rebalance | **-8.96** | -47.60 | 835.38 | 5.08 | 98.09 | 100% | 40 |
| 200 | narrow_static | 254.81 | 216.17 | 223.57 | 7.47 | 0.00 | 91.4% | 0 |
| 200 | narrow_rebalance | 73.25 | 34.60 | 252.01 | 33.17 | 7.57 | 100% | 3 |
| 400 | narrow_static | 165.80 | 127.16 | 130.88 | 3.76 | 0.00 | 100% | 0 |

(For `static` net PnL falls monotonically as the band widens: `100→338, 200→255,
400→166, 600→132, 1000→99, 2000→73`. Fee density beats width.)

### Reading

1. **Fee density dominates in this regime.** Net PnL rises as the static band
   tightens, even though the tightest band (`±100`) is out of range a third of the
   time. The forgone fees while out of range are smaller than the extra density
   captured while in range.

2. **Rebalancing-on-exit destroys value here.** At `±100`, `narrow_rebalance`
   earns *more* gross fees ($470 vs $314 — it is always in range) yet nets
   **$88 vs $338**. Each of its 12 recenters crystallizes inventory loss and pays
   gas+slippage. The price wanders out and mean-reverts, so chasing it is a pure
   tax. `vol_scaled` (40 recenters) is the extreme: it *loses to simply holding.*

3. **The DeFiLlama-style "headline" view (`passive_wide`) captures almost nothing:**
   +$10 vs hold. The edge in this pool lives entirely in concentration.

## Critical caveat — do not over-claim

This is a **single ~20h calm, mean-reverting window** (net drift ~1%, excursion
< 6%). The "tight static band, never rebalance" conclusion *depends on mean
reversion*. In a trending or volatile regime the same static band gets stranded
out of range holding the adverse asset — precisely the flow-toxicity / inventory
risk the strategy docs warn about. So this is **necessary but not sufficient**
evidence. Before trusting tight static ranges:

- Replay **trending and high-volatility windows** (out-of-sample).
- Add an explicit **out-of-range opportunity cost** and **reward (AERO emissions)
  income**, both currently unmodeled.
- Snap ranges to the pool's tick spacing (200); the engine does not yet.
- Model partial in-range fee capture within a single boundary-crossing swap.

## Side finding — the real active WETH-USDC pool

Enumerating WETH-USDC across all factories × tick spacings (last 300 blocks):

| factory | spacing | pool | fee bps | swaps/300blk |
| --- | ---: | --- | ---: | ---: |
| **Initial** | **100** | **`0xb2cc224c1c9fee385f8ad6a55b4d94e92359dc59`** | **0.50** | **318** |
| GaugesV3 | 1 | `0x4e392fbfe4d0557c82d2f97f02ec39daa31516dd` | 0.80 | 96 |
| GaugesV3 | 50 | `0x3fe04a59ebd38cf06080a6f60a98d124eb59392a` | 12.56 | 87 |
| Initial | 1 | `0xdbc6998296caa1652a810dc8d3baf4a8294330f1` | 0.80 | 62 |

The DeFiLlama-resolved pool the backfill was using (`0x56aeaf4af2df4bdfd9d865830fefdd278b25e7ef`,
GaugeCaps) is **not** the active venue. The real WETH-USDC flow is on the
**Initial / spacing-100 / 0.5 bps** pool `0xb2cc…dc59`. Note the 0.5 bps fee tier:
much thinner fee density than WETH-AERO's 21.25 bps, so WETH-AERO remains the
stronger fee-density research target while WETH-USDC is the major-pair control.

## Next steps

1. Backfill the real WETH-USDC pool `0xb2cc…dc59` (needs an explicit-pool-address
   option on `backfill-slipstream-events`; current backfill resolves via DeFiLlama).
2. Collect/replay a **volatile WETH-AERO window** to test whether tight static
   ranges survive trends.
3. Add reward-emissions income and explicit out-of-range opportunity cost to the
   replay so the baseline comparison is complete.
