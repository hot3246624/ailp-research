# Live Readiness: Strategy Score and Launch Gap

Snapshot: 2026-06-28 15:25 CST.

Revision note, 2026-06-30 CST: this document's `$1k-$3k` pilot recommendation was
based on the high-fee WETH-USDC 200 bps pool's same-pool rebalance impact. It should
not be read as a global mainstream-pool capacity ceiling. See
[`capacity-truth-audit.md`](capacity-truth-audit.md): deep WETH-USDC capacity is much
larger, while fee density is lower. The launch recommendation remains "no autonomous
live trading," but the next shadow milestone should be a `$10k/$50k/$100k`
capacity-first matrix across mainstream pools.

## Current Score

The strategy has **evidence of edge**, but it is not the naive strategy we started
with.

What failed:

- **Stable-stable low-yield LP** is not the target; expected return is too low.
- **Narrow, mechanical rebalancing** loses. Across WETH-AERO and CTR-USDC tests,
  fee-alpha exists, but frequent recentering turns volatility into churn cost.
- **Naked volatile LP** is regime-dependent. It can win in calm markets and lose hard
  versus hold in trends or crashes.

What works:

- **High fee / low volatility pools** are the best target. The key metric is
  `fee_bps / realized_tick_vol`, not headline APR.
- **WETH-USDC 200 bps** is the strongest researched candidate so far:
  - delta-hedged narrow band: about **+$88 to +$91 per $10k** in the tested window;
  - std about **$20**;
  - drawdown about **$1**;
  - clear fee-LVR edge around **+$113**.
- **CTR-USDC 100 bps** validates the low-churn thesis:
  - passive-wide mean about **+$116 per $10k** in demeaned multi-path;
  - beats hold on about **83%** of paths;
  - hedged-wide is almost flat in the raw down-crash path, about **-$15 vs hold -$537**.
- **WETH-AERO** has real fee-alpha, but it is not currently the best deployment:
  - calm window narrow-static beat hold by about **+$373**;
  - trend window LP policies lost to hold;
  - current DeFiLlama candidate read shows about **19.8% APY**, reward-heavy, not a
    >100% fee opportunity.

## Current On-Chain Gate Read

Latest lightweight reads:

| pool | capital | current execution result |
| --- | ---: | --- |
| WETH-AERO 21.75 bps | $10k | dry-run gates pass; swap impact about **0.8 bps** |
| WETH-USDC 200 bps | $10k | rejected; swap impact about **53.3 bps** > 30 bps |
| WETH-USDC 200 bps | $5k | passes; swap impact about **26.7 bps** |
| WETH-USDC 200 bps | $3k | passes; swap impact about **16.0 bps** |
| WETH-USDC 200 bps | $1k | passes; swap impact about **5.3 bps** |

Base gas is not the blocker: 500k gas was about **$0.009** at the sampled gas price.
The blocker is pool depth, flow continuity, hedge execution, and operational safety.

## Distance to Real Money

Status: **not ready for unattended live trading**, but close to a small guarded pilot.

Ready:

- Pool discovery and scoring loop exists.
- Strategy replay / stress / multi-path framework exists.
- Real Slipstream calldata exists.
- Minimal read-only position inspection exists:
  `inspect-position --token-id ...` reads owner, pool, gauge, range, current tick,
  in-range status, liquidity, and owed tokens.
- Funded fork simulation passes for fresh mint and rebalance:
  `swap -> mint`, then `collect -> decreaseLiquidity -> collect -> mint`.
- Receipt-status checking and real gas readings exist.
- Main execution bug classes found and fixed:
  - second collect is required after decreaseLiquidity;
  - raw NPM `u128` liquidity must not be rounded through `f64`.
  - risk-token inventory gates are side-aware via `--risk-token-side`.

Not ready:

- Minimal read-only position monitoring now exists:
  `monitor-position --token-id ... --output logs/base/positions.jsonl` persists
  current tick, in-range status, liquidity, owed tokens, owner/gauge state, token
  amounts, USD exposure (when `--token0-usd` is supplied), risk-token share, alert
  labels, and kill-switch reasons as JSONL. It still does not compute full shadow
  PnL, hedge PnL, reward liquidation, or wallet balances.
- No **delta hedge adapter** yet. The best risk-adjusted strategy depends on a perp
  hedge, likely Hyperliquid or another venue.
- No **post-trade accounting ledger** yet: fees, rewards, hedge PnL, gas, slippage,
  realized vs expected edge.
- Kill-switch coverage is still partial: monitor can flag out-of-range and one-sided
  risk exposure, but max drawdown, stale RPC, sequencer/RPC health, and failed
  simulation count are not wired into an operator halt state yet.
- RPC is not production-grade. Public/free endpoints are fine for research but too
  slow and flaky for automated execution.
- Reward APR and reward liquidation are still approximated; reward-heavy pools should
  not be trusted as primary edge until liquidation modeling is done.

## Launch Recommendation

Do **not** launch an autonomous strategy yet.

The next deployable milestone is a **shadow/live pilot with tiny capital**:

1. WETH-USDC 200 bps, **$1k-$3k** max, because it passes current impact gates.
2. Start with dry-run + shadow accounting only: produce plans every N minutes, do not
   broadcast.
3. Add hedge simulation side-by-side; only then connect a real hedge adapter.
4. Run 3-7 days of shadow results:
   - expected vs realized range occupancy;
   - estimated fee accrual;
   - dry-run rejection rate;
   - quoted impact drift;
   - RPC latency / failure rate.
5. Only then allow guarded execution with manual approval per transaction.

## Next Build Order

1. **Position monitor**: read active NFT, range, liquidity, balances, fees, stake
   state, and current exposure. The one-shot NFT inspector is done; the next step is
   persistence, wallet balances, and alerting.
2. **Proposal daemon**: periodically produce a signed-off JSON plan with gates and
   expected edge; store every plan.
3. **Shadow PnL ledger**: mark plan outcomes without trading.
4. **Hedge adapter model**: track delta target and hypothetical perp PnL/funding.
5. **Manual guarded execution**: one-click or CLI-confirmed execution, never
   autonomous signing.
6. **Small-cap live pilot**: only after shadow PnL and operational metrics are clean.

Bottom line: the research edge is real but narrow. We are roughly **70% through the
research/execution prototype**, and about **3-5 engineering milestones away from a
responsible tiny live pilot**.
