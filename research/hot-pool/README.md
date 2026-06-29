# Hot-Pool Research Workspace

This directory is the coordination point for high-fee/high-flow pool experiments.

Keep durable protocol and summaries in git. Keep bulky or frequently changing raw
outputs untracked under ignored `data/` or `logs/` paths.

Suggested local results log:

```text
research/hot-pool/results.tsv
```

Candidate queue:

```bash
cargo run -p autopool-cli -- hot-pool-candidates \
  --min-tvl-usd 50000 \
  --min-volume-usd-24h 25000 \
  --min-fee-apr 100 \
  --max-fee-apr 5000 \
  --target-fee-apr 2000 \
  --output data/hot-pool/candidates/latest.json
```

Experiment manifest:

```bash
cargo run -p autopool-cli -- hot-pool-experiment-plan \
  --input data/hot-pool/candidates/latest.json \
  --data-dir data/solana/hot-pool \
  --limit 12 \
  --output data/hot-pool/experiments/latest.json \
  --write-specs
```

Replay normalized CLMM swaps after a Solana decoder writes `SwapObs` JSONL:

```bash
cargo run -p autopool-cli -- replay-normalized-swaps \
  --spec data/solana/hot-pool/specs/<experiment>.json \
  --swaps data/solana/hot-pool/swaps/<pool>/swaps.jsonl
```

Solana proxy replay, used before tick-by-tick replay is available:

```bash
cargo run -p autopool-cli -- solana-proxy-replay \
  --min-tvl-usd 50000 \
  --min-volume-usd-24h 25000 \
  --min-fee-apr 100 \
  --max-fee-apr 5000 \
  --min-volume-tvl-24h 0.5 \
  --capital-usd 10000 \
  --output data/solana/proxy/latest.json
```

Recent real swap landing sample:

```bash
cargo run -p autopool-cli -- sample-solana-pool-swaps \
  --pool-address HnhpJPJgBG2KwniMTNW8cVBHvk1hFog3RC3kjnyc23tD \
  --program-id CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK \
  --token0-mint CARDSccUMFKoPRZxt5vt3ksUbxEFEcnZ3H2pd3dKxYjp \
  --token1-mint EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v \
  --signature-scan-limit 250 \
  --max-signature-pages 4 \
  --min-normalized-swaps 200 \
  --request-sleep-ms 250 \
  --output data/solana/swaps/raydium-cards-usdc-sample.json \
  --normalized-output data/solana/hot-pool/swaps/raydium-cards-usdc/swaps.jsonl
```

For Raydium CLMM this also decodes `SwapEvent` into a normalized swap preview
containing signed amounts, `sqrt_price_x96`, active liquidity, and tick. Use
`--max-signature-pages` plus `--min-normalized-swaps` for larger replay windows. The
sampler prints progress every 50 scanned signatures and ends with a
`next_before_signature` cursor for continuing older pagination.

Merge overlapping normalized samples into a stable replay stream:

```bash
cargo run -p autopool-cli -- merge-normalized-swaps \
  --input data/solana/hot-pool/swaps/raydium-cards-usdc/swaps.jsonl \
  --input data/solana/hot-pool/swaps/raydium-cards-usdc/swaps-refresh.jsonl \
  --input data/solana/hot-pool/swaps/raydium-cards-usdc/swaps-older.jsonl \
  --output data/solana/hot-pool/swaps/raydium-cards-usdc/swaps-merged-3.jsonl
```

Latest `CARDS-USDC` real replay check: after adding a 20 second Solana HTTP timeout
to bound public-RPC scans, 185 signatures -> 77 target swaps -> 77 normalized rows,
wall-clock span ~46.5 minutes. `vol_scaled_rebalance` / `adaptive_regime` produced
about $78.22 net on $10k with $23.08 fee-LVR and $1.82 max drawdown, while
`delta_hedged` stayed positive at about $13.19 net with much lower directional risk.

Rolling window replay:

```bash
cargo run -p autopool-cli -- replay-normalized-windows \
  --spec data/solana/hot-pool/specs/raydium-cardsusdc-hnhpjpjg.json \
  --swaps data/solana/hot-pool/swaps/raydium-cards-usdc/swaps.jsonl \
  --window-swaps 25 \
  --step-swaps 10 \
  --min-windows 4
```

Latest larger-window check used the 77-row file with 40-swap windows and 15-swap
steps, producing 3 windows. `vol_scaled_rebalance` / `adaptive_regime` won vs hold
in 66.7% of windows with positive p05 net APR (~656%), while `delta_hedged` won in
66.7% with much lower worst drawdown (~$3.07). Caveat: this is still one short
directional regime, so the left tail is less bad than before but not yet deployable.

Hedge fraction sweep:

```bash
cargo run -p autopool-cli -- replay-normalized-hedge-grid \
  --spec data/solana/hot-pool/specs/raydium-cardsusdc-hnhpjpjg.json \
  --swaps data/solana/hot-pool/swaps/raydium-cards-usdc/swaps.jsonl \
  --window-swaps 40 \
  --step-swaps 15 \
  --min-windows 3 \
  --grid-hedge-fraction 0 \
  --grid-hedge-fraction 0.25 \
  --grid-hedge-fraction 0.5 \
  --grid-hedge-fraction 0.75 \
  --grid-hedge-fraction 1
```

Latest grid result: `hedged_narrow` at 0.75 fixed hedge had the best score, with
~1074% p05 mechanical net APR and ~$3.48 worst drawdown. Lower hedge fractions had
higher mean net, but the p05 APR and drawdown deteriorated quickly.

The grid report now also prints a `by regime` section. On the same 3-window sample,
range windows favored heavier hedging for left-tail control, while the single
`trend_down_money` window favored keeping more directional beta and punished higher
fixed hedges versus hold. Treat this as a rule-design hint, not a deployable
setting: the next useful step is regime-conditioned hedge sizing over more windows.

The current hedge-grid command also prints a `lagged_regime_rule` row. It uses the
prior window's regime to choose the next window's hedge fraction, avoiding direct
lookahead. The default rule is conservative in range-like states:
`range=1.00`, `volatile=1.00`, `money_trend=0.25`, `risk_trend=1.00`. On the latest
201-row merged CARDS-USDC sample, 25-swap/10-step windows showed 59% win rate vs
hold, mean vs hold about -$2.80, p05 APR about +660%, and worst drawdown about
$9.99. The 40-swap/15-step and 60-swap/20-step views had negative p05 APR, and the
80-swap/25-step view had only 4 lagged windows while still losing to hold on average.
This is still candidate evidence rather than a deployable rule. Caveat: this remains
a small, overlapping-window sample inside one short wall-clock regime.

Promotion gate:

```bash
cargo run -p autopool-cli -- replay-promotion-gate \
  --spec data/solana/hot-pool/specs/raydium-cardsusdc-hnhpjpjg.json \
  --swaps data/solana/hot-pool/swaps/raydium-cards-usdc/swaps-merged-5.jsonl \
  --min-p05-net-apr-pct 500 \
  --min-mean-vs-hold-usd 0 \
  --min-win-rate-vs-hold-pct 60 \
  --max-drawdown-pct 0.05
```

Current CARDS-USDC promotion verdict: `reject_replay`. The 25-swap/10-step view
misses the win-rate and mean-vs-hold gates; 40-swap/15-step misses win-rate,
mean-vs-hold, and left-tail APR; 60-swap/20-step misses win-rate and left-tail APR;
80-swap/25-step has a high p05 APR but still loses to hold on average. This encodes
the current business goal directly: a pool/policy must show about 500%+ left-tail
net APR and a stable edge over hold before shadow monitoring.

The latest scout also says the best near-term opportunities are not Raydium-only.
Current hot candidates are mostly Meteora DLMM and Orca Whirlpool pools. Meteora
proxy APRs are often much higher, but they require a DLMM replay adapter before they
are actionable. Orca `SOL-PUMP` is now replayable through the Whirlpool `Traded`
event adapter, using snapshot active liquidity until historical liquidity snapshots
are added. Raydium infrastructure is useful for EVM/Solana style execution research,
but current Raydium hot candidates are either CARDS rejected by the gate or WSOL-CX
rejected as severe-risk until replay proves otherwise.

Orca `SOL-PUMP` first replay:

```text
sample A: scanned 408 signatures, kept 80 normalized swaps, tx_errors=0
sample B: scanned 303 signatures, kept 80 normalized swaps, tx_errors=0
merged:   160 unique swaps, slot span 429614430..429620972, tick span 39003..39161
```

With `narrow_half_width=100`, `$10k` capital, SOL marked near `$72.28`, and
snapshot active liquidity, the merged replay is risk-control evidence rather than
deployability evidence. `delta_hedged` improved hold by about `$70.31` and cut max
drawdown to about `$12.99`, but still lost about `$6.02` net on the segment.
`hedged_wide` reduced drawdown further while earning little fee alpha. Promotion gate
returned `reject_replay`: 20/40/60-swap windows all beat hold on average, but their
p05 mechanical APR remained deeply negative. Next useful work is broader Whirlpool
coverage or historical active-liquidity reconstruction, not overfitting this short
SOL-PUMP segment.

Orca `SOL-GRASS` follow-up replay:

```text
sample A: scanned 86 signatures, kept 80 normalized swaps, tx_errors=0
sample B: scanned 85 signatures, kept 80 normalized swaps, tx_errors=0
merged:   160 unique swaps, slot span 429623681..429644467, tick span 49488..49876
```

This was the current cleanest Orca P0 candidate: ~314% 24h fee APR, ~30bps fee tier,
~$72k TVL, no discovery warnings, and high sample hit-rate. It still did not promote.
With `narrow_half_width=384`, `$10k` capital, SOL marked near `$72.93`, and snapshot
active liquidity, the full merged replay had `delta_hedged` at about `-$21.39` net
versus hold at about `-$159.08`; `hedged_wide` was about `-$7.61` with ~$12.65 max
drawdown. Rolling 20-swap windows showed `hedged_wide` mean net around `+$0.12` but
p05 APR around `-101%`; the lagged promotion gate returned `reject_replay` with p05
APR around `-1399%`, `-1238%`, and `-1486%` across 20/40/60-swap families.

Current Whirlpool read: the hedge machinery is useful risk control, but it has not
yet shown a stable positive-fee-alpha strategy. Reported hot-pool APR is still a
candidate alarm, not a deployable strategy APR.

Orca `SOL-CARDS` heartbeat replay:

```text
sample A: scanned 770 signatures, kept 65 normalized swaps, tx_errors=0
span:     slot 429658908..429669695, tick -11468..-10842
```

This was the newest clean Orca P0 candidate after refreshing the queue: ~642% 24h
fee APR, 100bps fee tier, ~$62k TVL, no discovery warnings. It is the first Whirlpool
sample where a very defensive `hedged_wide` slice had a positive short-window read:
15-swap windows showed mean net around `+$1.20`, mean mechanical APR around `653%`,
p05 APR around `106%`, and worst drawdown around `$1.88`. That still does not clear
the 500% left-tail gate, and 25-swap windows flipped negative. The promotion gate now
supports `--gate-policy hedged-wide`; it still returned `reject_replay` with p05 APR
about `106%`, `-545%`, and `-1665%` across 15/25/40-swap families. Treat
`SOL-CARDS` as an interesting Whirlpool lead for more data, not a strategy promotion.

Schema:

```text
commit	pool	window	capital_usd	policy	net_usd	fee_minus_lvr_usd	max_dd_usd	rebalances	time_in_range_pct	status	description
```

Statuses:

- `keep`: beats all required baselines after costs and complexity.
- `discard`: worse, unstable, or complexity not justified.
- `crash`: data/logic/runtime failure.
- `needs_validation`: promising but missing pool-state or replay confirmation.

Do not promote a pool or policy from headline APR alone. Promotion requires replay,
capacity, and shadow-monitor evidence.
