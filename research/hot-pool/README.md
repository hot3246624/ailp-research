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
`--max-signature-pages` plus `--min-normalized-swaps` for larger replay windows.

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
