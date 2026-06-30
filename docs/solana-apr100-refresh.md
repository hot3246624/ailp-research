# Solana APR>=100 Refresh

Snapshot: 2026-06-30 CST. The business gate is now: do not advance pools with
credible current APR below `100%`.

Base/Aerodrome currently has no mainstream candidate above that threshold. Solana
does.

## Commands

```bash
cargo run -q -p autopool-cli -- solana-discover \
  --venue orca --venue raydium --venue meteora \
  --min-tvl-usd 100000 \
  --min-volume-usd-24h 100000 \
  --min-fee-apr 100 \
  --max-fee-apr 5000 \
  --page-size 200 \
  --limit 40

cargo run -q -p autopool-cli -- hot-pool-candidates \
  --venue orca --venue raydium --venue meteora \
  --min-tvl-usd 100000 \
  --min-volume-usd-24h 100000 \
  --min-fee-apr 100 \
  --max-fee-apr 5000 \
  --target-fee-apr 100 \
  --min-volume-tvl-24h 0.5 \
  --page-size 200 \
  --limit 40
```

## P0 Queue

These rows passed the `>=100%` 24h fee APR gate and have protocol data whose fee APR
matches the simple fee * turnover formula. They are replay candidates, not deployment
approvals.

| rank | venue | pool | fee APR 24h | fee APR 7d | TVL | 24h volume | note |
| ---: | --- | --- | ---: | ---: | ---: | ---: | --- |
| 1 | Orca Whirlpool | SOL-USDC | 131.5% | 92.8% | $24.4m | $220.1m | best capacity/blue-chip target; high turnover |
| 2 | Raydium CLMM | CARDS-USDC | 264.9% | 168.2% | $3.33m | $6.04m | 7d APR still >100%; wide 24h price range |
| 3 | Orca Whirlpool | SOL-HYPE | 162.6% | 115.9% | $1.35m | $2.00m | cleaner than most long-tail rows |
| 4 | Orca Whirlpool | SOL-ORCA | 223.8% | 84.3% | $0.77m | $2.95m | high 24h, weaker 7d |
| 5 | Raydium CLMM | SNDK-USDC | 312.5% | 79.4% | $0.40m | $3.38m | strong 24h, but 7d below gate |

## Meteora Read

Meteora DLMM dominates the visible high-APR table:

- SOL-USDC 4 bps: reported 794% APR, $3.34m TVL, $48.5m volume;
- SOL-USDC 10 bps: reported 343.5% APR, $2.79m TVL, $12.4m volume;
- HYPE-USDC 20 bps: reported 217.6% APR, $5.72m TVL, $10.0m volume;
- JUP-USDC 10 bps: reported 580.6% APR, $0.90m TVL, $5.1m volume.

But most Meteora rows carry `meteora_daily_ratio_disagrees_with_apy`, and many have
reported APR far above the fee * turnover formula. They stay P1/P2 until historical
active-bin liquidity and event/account reconstruction are solved. Do not treat them
as ready replay wins.

## Decision

Yes: the next candidate search should be Solana-first.

Update: the first bounded replay pass after this queue is recorded in
[`solana-apr100-stage-result.md`](solana-apr100-stage-result.md). Stage decision:
`SNDK-USDC` rejected, `CARDS-USDC` shows a short-burst edge but fails merged
cross-regime gates, and `Orca SOL-USDC` remains unpromoted because fresh sampling
was too sparse while prior rolling gates rejected.

Do **not** restart broad memecoin scanning. The next useful bounded work is:

1. rerun Orca `SOL-USDC` normalized replay with current data, because it is the only
   large-capacity, blue-chip, `>=100%` candidate;
2. rerun Raydium `CARDS-USDC` normalized replay, because both 24h and 7d fee APR
   remain above `100%`;
3. keep `SOL-HYPE` as a second Orca candidate if `SOL-USDC` again fails;
4. only revisit Meteora after historical active-bin liquidity is available, or with
   a clearly caveated live-shadow experiment.

Promotion still requires rolling fee-minus-LVR, drawdown, capacity, and routed
rebalance-cost gates. The APR gate is only the entry filter.
