# Hot-Pool Research Workspace

This directory is the coordination point for high-fee/high-flow pool experiments.

Keep durable protocol and summaries in git. Keep bulky or frequently changing raw
outputs untracked under ignored `data/` or `logs/` paths.

Suggested local results log:

```text
research/hot-pool/results.tsv
```

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
