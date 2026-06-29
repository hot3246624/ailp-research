# Execution Dry-Run (Milestone 5) — and why protocol version is decisive

The strategy/replay engine is a **Uniswap-v3 concentrated-liquidity *model*** (ticks,
ranges, `sqrtPriceX96`, per-position liquidity, fee growth). That is correct for the
pilot because **Aerodrome Slipstream is a v3 fork** (uses `tickSpacing` instead of
fixed fee tiers). All the research conclusions (fee−LVR, churn, delta hedge,
WETH-USDC-200bps win) are v3 math and venue-correct.

**Execution is where the version is decisive — it is protocol-specific calldata, not
a generic "LP".**

| | v2 (Aerodrome V1) | **v3 (Slipstream — our pilot)** | v4 (Uniswap v4) |
| --- | --- | --- | --- |
| liquidity | full-range `x*y=k` | **concentrated, ticks/ranges** | concentrated + hooks |
| position | fungible ERC-20 LP | **ERC-721 NFT via NonfungiblePositionManager** | singleton PoolManager, ERC-6909 |
| fees | auto-compound | **accrue separately, `collect()`** | hook-defined, can be dynamic |
| rebalance | add/remove only | **collect→decreaseLiquidity→swap→mint→(un/stake)** | flash accounting: unlock→modifyLiquidity/swap→settle/take |
| our engine | no (no ranges) | **yes** | model yes, calldata totally different |

Consequences baked into the dry-run:

- Target is **Slipstream's v3 NPM + SwapRouter + Gauge** (addresses in
  `autopool-aerodrome`). A rebalance is a **multi-step plan**, and a Slipstream
  position that earns emissions must be **staked**, so rebalancing also requires
  **unstake → … → stake** — extra gas that feeds straight into the churn cost the
  research measured.
- **v2** (Aerodrome V1) has no range concept and isn't the strategy we found edge in;
  it would need a separate, simpler adapter.
- **v4** changes the *architecture* (one singleton, flash accounting, hooks that can
  make fees dynamic — breaking the static `fee-bps` assumption). Relevant only if we
  add Uni-v4 Base pools; needs its own adapter.

The `EvmPoolKind { UniswapV3, UniswapV4, AerodromeSlipstream }` type is the intended
adapter boundary. The dry-run is **Slipstream/v3 only** and says so.

## `dry-run-rebalance`

```bash
BASE_RPC_URL=... cargo run -p autopool-cli -- dry-run-rebalance \
  --pool-address 0x56aeaf4af2df4bdfd9d865830fefdd278b25e7ef \
  --capital-usd 10000 --token0-usd 1573 --decimals0 18 --decimals1 6 \
  --half-width-ticks 600 --expected-edge-usd 50
```

It:
1. reads pool state (tick, `sqrtPriceX96`, tick spacing, `fee()`) and gas price;
2. snaps the target band `current_tick ± half_width` to tick spacing;
3. computes target inventory with the **same v3 math as the backtest**
   (`autopool_backtest::cl_mint_amounts`) so the plan and the sim agree;
4. emits the action sequence with **slippage-protected min amounts**
   (`amount0Min/amount1Min`, `amountOutMin`) and an estimated gas cost;
5. runs **hard risk gates**: gas-vs-expected-edge, one-sided inventory, slippage bound;
6. prints the plan marked `requires_signature=true, broadcast=false` and **never
   signs**. JSON via `--format json`.

Example (WETH-USDC 200 bps, fresh mint): plan = `swap(token0→token1) → mint → stake`,
gas ≈ $0.016, all three gates PASS.

## Real swap simulation makes the capacity constraint *enforced*

The dry-run now simulates the rebalance swap against the pool's **real in-range
liquidity** (`simulate_v3_swap`, v3 closed form) to get a true `expected_out` and
**price impact**, and gates on it. This surfaces the binding real-world limit:

| pool | fee | $10k rebalance swap impact | gate |
| --- | ---: | ---: | --- |
| WETH-USDC **0x56ae** (the fee-alpha winner) | 200 bps | **39.4 bps** | **REJECTED** (> 30 bps) |
| WETH-USDC 0xb2cc (deep) | 0.5 bps | 1.4 bps | PASS (but no alpha) |

**The execution gate rejects the strategy on the exact pool where the research found
edge.** The 200 bps pool is the only one with positive fee-alpha, but it is *thin*, so
a $10k position cannot be rebalanced without 39 bps of swap slippage — which would eat
the edge. This turns the research's soft "modest capacity" caveat into a hard,
quantified limit: **the fee-alpha pool's capacity is only a few $k** before impact
dominates. The deep pool that *can* take size has no alpha. That tension — alpha lives
in thin pools, depth lives in zero-fee pools — is the real ceiling on this venue, and
the execution layer now enforces it before any signature.

## Real calldata (done) + on-chain quote (partial)

The planner now emits **real, signable calldata bytes** for the swap, collect,
decreaseLiquidity, and mint path.
Selectors are computed with **keccak** from the exact Slipstream signatures (which use
`int24 tickSpacing`, not `uint24 fee`), and verified against known selectors
(`getPool(address,address,int24)` -> `28af8d0b`, `transfer` -> `a9059cbb`,
`burn(uint256)` -> `42966c68`). Ticks are sign-extended int24; example swap calldata
for WETH->AERO decodes correctly
(selector `a026383e`, tokens, `tickSpacing=0xc8`=200, recipient, deadline, amountIn).

The **swap is simulated against the pool's real in-range liquidity**
(`simulate_v3_swap`): e.g. 1.77 WETH → 5,974 AERO, 11.1 bps impact, which sets
`amountOutMin` and the price-impact gate.

The **on-chain Quoter cross-check is best-effort and currently falls back**: Slipstream's
MixedQuoter is the Uniswap-**v1 Quoter pattern** (it *reverts* with the result encoded
in the revert data rather than returning it), so a plain `eth_call` shows `execution
reverted`. Completing it needs either (a) parsing the revert `data` bytes, or (b) the
deployment-matched **QuoterV2** address that returns normally. The offline real-state
sim is the working swap simulation in the meantime.

## Multicall (done)

The NPM-side actions are bundled into one real `multicall(bytes[])` calldata
(`abi::encode_multicall`, selector `ac9650d8` verified):

- Fresh position: `mint`.
- Rebalance with known `--token-id`: `collect -> decreaseLiquidity -> collect -> mint`.

The second `collect` is required. Slipstream's `decreaseLiquidity` records withdrawn
principal as owed tokens; it does not transfer the tokens to the wallet. Without the
second `collect`, the following `mint` reverts in the callback with `STF` because the
wallet cannot satisfy `transferFrom`.

`burn(tokenId)` is intentionally **not** part of the executable rebalance multicall.
It is a separate cleanup step for the empty old NFT; the funded fork showed Slipstream
can reject `burn` in the same path with `NC` even after principal is collected. The
strategy does not require burn to complete the capital rotation.

The swap (SwapRouter) and stake (gauge) are different contracts, so they stay separate
txs. A staked position still needs `unstake` before modification and `stake` after the
new NFT is minted.

## Funded fork simulation (done)

`scripts/fork-sim-rebalance.sh` starts a local Base fork, funds the standard anvil test
account, deposits WETH, approves the router/NPM, executes the dry-run's real calldata,
snapshots the minted NFT with `monitor-position`, and checks receipt status. It never
signs or broadcasts on mainnet.

Validated on WETH-AERO GaugesV3 (`0x4e5066...e51`) using a local Base fork:

| step | calls | gas |
| --- | ---: | ---: |
| fresh SwapRouter swap | 1 | 234,346 |
| fresh NPM mint multicall | 1 | 418,539 |
| monitor minted position | read-only | n/a |
| rebalance NPM multicall | 4 | 497,166 |

Two fork findings are now baked into the CLI:

- The funded recipient must be configurable (`--recipient`) so fork calldata pulls from
  the funded test account rather than the dead default address.
- NPM position liquidity is read as exact `u128` and passed unchanged to
  `decreaseLiquidity`; converting through `f64` can round above the real liquidity and
  cause an immediate NPM revert.
- The one-sided inventory gate now takes an explicit `--risk-token-side token0|token1`
  plus `--max-risk-token-share`; this prevents WETH-USDC-style pools from being checked
  as if the stablecoin side were the risk asset.
- `monitor-position` now enriches snapshots with CL token amounts, USD exposure when
  `--token0-usd` is supplied, risk-token share, alert labels, and kill-switch reasons.
  The funded fork validates that the snapshot path remains read-only
  (`requires_signature=false`, `broadcast=false`).

## Remaining gaps

- **On-chain Quoter result.** The free Alchemy tier returns `{"code":3,"message":
  "execution reverted"}` with no revert `data`, so the v1 quoter payload still cannot
  be decoded reliably. The dry-run keeps using the offline real-state
  `simulate_v3_swap` unless `--skip-quoter=false` succeeds on a better RPC/fork.
- **The delta-hedge leg.** The deployable strategy is delta-hedged, but the short is a
  perp on a different venue (e.g. Hyperliquid), so it is a separate plan/adapter, not a
  Slipstream action.

## Next

1. Add the perp-hedge plan as a second adapter and a combined risk view.
2. Wire the plan to the actual strategy output (delta-hedged narrow band on the
   highest-`fee/vol` pool) so proposals are generated from live state.
3. Only after stable dry-run proposals: guarded execution behind config gates
   (Milestone 6), still never signing without explicit opt-in.
