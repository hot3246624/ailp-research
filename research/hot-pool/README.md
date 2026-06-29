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
  --limit 8 \
  --signature-scan-limit 30 \
  --output data/solana/swaps/raydium-cards-usdc-sample.json \
  --normalized-output data/solana/hot-pool/swaps/raydium-cards-usdc/swaps.jsonl
```

For Raydium CLMM this also decodes `SwapEvent` into a normalized swap preview
containing signed amounts, `sqrt_price_x96`, active liquidity, and tick.

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
