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

The promotion gate now also supports `--gate-policy lagged-policy-switch`. It chooses
the current window's policy from the prior window's regime, with default mapping
`range=delta_hedged`, `volatile=hedged_wide`, `money_trend=hedged_wide`, and
`risk_trend=hedged_wide`. Override it with `--rule-range-policy`,
`--rule-volatile-policy`, `--rule-trend-money-policy`, and
`--rule-trend-risk-policy`.

It also supports `--gate-policy lagged-policy-blend`, a smoother two-sleeve allocation
between `delta_hedged` and `hedged_wide` using only the prior window's regime. Use the
`--rule-*-wide-fraction` flags to set the capital share allocated to `hedged_wide`.
`--gate-policy delta-trend-stop` is now available for direct testing of a narrow
dynamic-delta LP that exits to the money leg on a short intra-window trend signal.

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

Orca `SOL-ORCA` coverage replay:

```text
sample A: scanned 106 signatures, kept 100 normalized swaps, tx_errors=0
sample B: scanned 103 signatures, kept 100 normalized swaps, tx_errors=0
merged:   200 unique swaps, slot span 429673585..429678365, tick span -28026..-27957
```

`SOL-ORCA` was selected from the refreshed queue as a broader Whirlpool coverage pass:
~201% 24h fee APR, 16bps fee tier, ~$763k TVL, ~$2.6m 24h volume, and volume/TVL
around 3.45. The full 200-row replay looked promising at first glance: `delta_hedged`
was about `+$3.76` net with ~$1.03 max drawdown, while `vol_scaled_rebalance` showed
about `+$8.86` net and ~1462% mechanical window APR. Rolling gates rejected it. The
lagged gate failed 25/40/60/80-swap windows with p05 APR around `-426%`, `-478%`,
`-431%`, and `-248%`, and negative mean edge versus hold. Direct `delta_hedged` and
`hedged_wide` gates also rejected. Treat this as a useful warning: full-window APR can
look investable while rolling left-tail and hold-edge metrics say no.

Orca `SOL-USDC` capacity replay:

```text
sample A: scanned 759 signatures, kept 187 normalized swaps, tx_errors=0
span:     slot 429682709..429683021, about 2.1 minutes, tick 26160..26162 after inversion
```

`SOL-USDC` is the large-capacity benchmark from the refreshed Orca queue: ~$24m TVL,
~$231m 24h volume, volume/TVL near 9.55, 4bps fee tier, and ~139% 24h fee APR. The
full short replay looked good before gating (`delta_hedged` about `+$1.10` net with
~$0.21 max drawdown; `vol_scaled_rebalance` about `+$1.34` net), but promotion
rejected it. The lagged gate p05 APR was about `-2854%`, `-1144%`, `-728%`, and
`130%` across 25/40/60/80-swap windows. Direct `delta_hedged` and `hedged_wide` gates
also rejected. This is not a long-regime verdict, but it is evidence that enormous
volume with a 4bps fee tier is not enough by itself.

Orca `SOL-Fartcoin` risk-pair replay:

```text
sample A: scanned 192 signatures, kept 160 normalized swaps, tx_errors=0
span:     slot 429680775..429687571, about 45.3 minutes, tick roughly -6715..-6567
```

This was the next broader Whirlpool risk sample: ~116% 24h fee APR, 16bps fee tier,
~$527k TVL, ~$1.05m 24h volume, volume/TVL near 2.00. It is the best Whirlpool
risk-pair replay so far but still rejected. Full replay had `delta_hedged` at about
`+$8.00` net, `+$35.84` versus hold, ~$2.60 max drawdown, and about 928% mechanical
window APR. Rolling gates exposed the tail: lagged p05 APR was about `-1865%`,
`-875%`, `-789%`, and `-1892%` across 25/40/60/80-swap windows. Direct `delta_hedged`
also rejected; `hedged_wide` improved left tail to about `-245%`, `-106%`, `-97%`,
and `-263%`, but mean net became small. Treat this as a promising rejection: there is
fee-alpha signal, but no stable 500%+ left-tail strategy yet.

First policy-switch test on `SOL-Fartcoin`: default `lagged-policy-switch`
(`range=delta_hedged`, non-range=`hedged_wide`) still rejected, with p05 APR about
`-1865%`, `-906%`, `-708%`, and `-771%` across 25/40/60/80-swap windows. An
all-`hedged_wide` switch improved the left tail to about `-267%`, `-119%`, `-108%`,
and `-285%`, but still failed the 500% gate and had small mean net. A trend
`narrow_rebalance` beta-participation map worsened the tail. The next strategy step
needs a sharper adverse-trend signal or smoother hedge-width control, not just coarse
prior-window policy switching.

First policy-blend test on `SOL-Fartcoin`: default `lagged-policy-blend`
(`range_wide=0.50`, non-range wide fraction `1.00`) rejected with p05 APR about
`-1066%`, `-507%`, `-408%`, and `-528%`. Raising `range_wide` to `0.75`, `0.90`, and
`0.95` improved the 25-swap p05 to about `-667%`, `-427%`, and `-347%`, but even the
all-wide boundary only reached about `-267%`, `-119%`, `-108%`, and `-285%`.
Conclusion: smoother capital blending helps risk control but still does not create a
deployable 500%+ left-tail strategy.

First intra-window stop test on `SOL-Fartcoin`: `delta_trend_stop` also rejected.
The aggressive threshold run had p05 APR about `-36738%`, `-21422%`, `-15534%`, and
`-9876%` across 25/40/60/80-swap windows. Looser threshold-20 still only improved to
about `-29691%`, `-12417%`, `-5460%`, and `-4683%`. Window decomposition showed the
problem: the stop often fired twice inside windows later classified as `range`,
turning small `delta_hedged` drawdowns into double-digit drawdowns. Current strategy
status is still no deployable Whirlpool strategy.

Meteora DLMM now has a bounded replay skeleton in `autopool_backtest::dlmm`: bin-step
price ratios, active-bin fee share, range occupancy, recenter costs, drawdown, and
APR are modeled for normalized bin observations. This is infrastructure only; real
Meteora evaluation still requires decoded swap events and historical bin-liquidity
snapshots.

First real DLMM active-bin snapshot is now wired through the official Meteora SDK:

```text
pool:      Meteora SOL-USDC 5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6
slot:      429710432
activeBin: -6457
bin step:  4 bps
active-bin liquidity: ~$5,060
30m volume from Meteora Data API: ~$1.83m
```

Commands:

```bash
scripts/meteora-dlmm-snapshot.sh \
  --spec data/solana/hot-pool/specs/meteora-solusdc-5rcf1dm8.json \
  --out data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-bin-snapshots.jsonl \
  --raw-out data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-bin-snapshot.latest.json \
  --raw-jsonl-out data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-bin-snapshots.raw.jsonl \
  --append

cargo run -q -p autopool-cli -- replay-dlmm-bins \
  --spec data/solana/hot-pool/specs/meteora-solusdc-5rcf1dm8.json \
  --bins data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-bin-snapshots.jsonl
```

The one-row replay shows about `+$486` gross fee for `$10k`, but this is a capacity
alarm rather than a strategy result: `$10k` is about `2.0x` active-bin liquidity, and
one observation cannot produce APR or rolling-window evidence. Append mode dedupes
by slot for heartbeat sampling, but Meteora Data API rolling-window volume is still
only a capacity/fee-density probe. Replay APR requires non-overlapping swap flow
matched to active-bin liquidity snapshots.

Append smoke on 2026-06-30 produced two SOL-USDC rows over slots
`429714568 -> 429714639`, active bins `-6468 -> -6465`, active-bin liquidity about
`$10.3k -> $10.0k`, and cap/active about `1.0x` for `$10k`. The Rust replay accepts
the stream, but the rows use overlapping rolling `30m` volume and are not promotion
evidence.

Non-overlapping Meteora swap flow is now wired separately:

```bash
scripts/meteora-dlmm-swap-flow.sh \
  --spec data/solana/hot-pool/specs/meteora-solusdc-5rcf1dm8.json \
  --out data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-swap-flow.jsonl \
  --raw-out data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-swap-flow.latest.json \
  --limit 25 \
  --signature-scan-limit 120 \
  --max-signature-pages 2 \
  --append
```

2026-06-30 probe: scanned 99 signatures, decoded 17 swap txs, 0 tx errors,
about `$40,048` non-overlapping flow notional, and average-execution bin proxy
`-6466..-6463`. This does not yet produce replay-grade DLMM observations because
sampled Meteora swap logs did not expose parseable pool `Swap` / `Swap2Evt` events,
and decoded `swap` instructions only contain `amountIn` / `minAmountOut`. We now
need to join this flow stream to repeated active-bin snapshots or archival bin-array
state.

Account reconstruction probe:

```bash
scripts/meteora-dlmm-account-probe.sh \
  --spec data/solana/hot-pool/specs/meteora-solusdc-5rcf1dm8.json \
  --flow data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-swap-flow.jsonl \
  --limit 5 \
  --raw-out data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-account-probe.latest.json
```

The 2026-06-30 run decoded 5/5 swap instructions and identified 3 touched bin arrays
(`-93`, `-92`, `-91`), while official EventParser produced 0 swap events. Current
`lbPair` / `binArray` decoding works, but it is latest state only; public RPC
`getTransaction` does not include historical account data and `getAccountInfo` is not
slot-historical. Status: `blocked_without_archival_account_state` for replay-grade
historical active liquidity unless we use an archival/indexer source or a same-slot
live snapshot pipeline.

Flow/snapshot proxy join is now wired:

```bash
node scripts/meteora-dlmm-join-flow-snapshots.cjs \
  --flow data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-swap-flow.jsonl \
  --snapshots data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-bin-snapshots.jsonl \
  --out data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-bin-flow-proxy.jsonl \
  --raw-out data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-bin-flow-proxy.latest.json \
  --max-slot-distance 400 \
  --active-bin-source flow-price

cargo run -q -p autopool-cli -- replay-dlmm-bin-windows \
  --spec data/solana/hot-pool/specs/meteora-solusdc-5rcf1dm8.json \
  --bins data/solana/hot-pool/swaps/meteora-sol-usdc/dlmm-bin-flow-proxy.jsonl \
  --window-observations 15 \
  --step-observations 5 \
  --min-windows 5 \
  --half-width-bins 5 \
  --capital-usd 1000
```

2026-06-30 live-shadow smoke: snapshot A at slot `429723287` had active bin `-6465`
and active-liq about `$9,971`; snapshot B at slot `429723404` had active bin `-6462`
and active-liq about `$9,239`. A fresh flow scan decoded 20 swaps from 51 signatures,
and the join admitted 19 rows while skipping 18 stale rows. Joined flow notional was
about `$68,467`; `$1k` capital was about `0.10x` active-bin liquidity. Proxy replay
showed `+$2.10` net and `$0.39` max drawdown over 19 rows, but the printed annualized
APR is not a strategy result because the window is tiny and active liquidity still
comes from nearest snapshots.

Second refresh expanded the proxy stream to 47 joined rows from 64 flow rows and 6
snapshots, with 17 stale rows skipped by a 500-slot gate. Joined notional was about
`$104,159`, active-bin proxy moved `-6465..-6453`, and `$1k` capital was about `0.13x`
average active-bin liquidity. Full proxy replay showed centered rebalance at `+$6.18`
net, `$4.28` fees, `$0.39` max drawdown, and 1 rebalance.

Rolling proxy windows now run through `replay-dlmm-bin-windows` with a promotion-style
proxy gate: p05 mechanical net APR >= `500%`, positive mean edge versus hold, win rate
versus hold >= `60%`, worst drawdown <= `5%` of capital, and mean capital/active-liq
<= `0.25x`. On 15-row/5-step windows (`7` windows), `centered_bin_rebalance` and
`static_bin_range` now print `pass_proxy_gate`; hold prints `reject_proxy` with
`win_rate` and `left_tail_apr` failures. Centered mean net was about `+$2.00`, p05
mechanical APR about `4,976%`, worst drawdown about `$0.39`, and cap/active-liq about
`0.14x`. This remains live-shadow proxy evidence, not a deployable APR.

Live shadow runner:

```bash
scripts/meteora-dlmm-live-shadow.sh \
  --flow-limit 25 \
  --signature-scan-limit 180 \
  --max-signature-pages 2 \
  --max-slot-distance 250 \
  --request-sleep-ms 100
```

The 2026-06-30 strict refresh took snapshots at slots `429741140` and `429741214`
around a 25-swap flow scan. The join admitted 40 rows from 89 total flow rows and
skipped 49 stale rows under a 250-slot max distance. Full proxy replay still showed
centered positive (`+$2.03` net, `$2.83` fees), but rolling gate rejected every policy:
centered failed only `left_tail_apr` with p05 about `-1,250%`; static also failed
`left_tail_apr` with p05 about `-1,134%`. Current SOL-USDC status is therefore
`proxy_candidate_rejected_on_latest_strict_refresh`, not strategy-approved.

Second strict SOL-USDC refresh confirmed the left-tail problem: 93 scanned signatures
produced 25 swaps with 0 tx errors; the 250-slot join admitted 50 rows from 114 total
flow rows and 10 snapshots. Full proxy centered improved to `+$4.29` net and `$4.39`
fees, but rolling 15-row/5-step gate again rejected all policies. Centered failed only
`left_tail_apr` with p05 about `-1,245%`; static failed the same gate with p05 about
`-1,115%`. Two consecutive strict refreshes reject SOL-USDC.

The refreshed Meteora queue made `JUP-USDC` the next USDC-numeraire candidate after
SOL-USDC: about `$916k` TVL, `$5.7m` 24h volume, `717%` fee APR, and volume/TVL near
`6.22`. Initial JUP live-shadow decoded 25 swaps from 67 signatures with 0 tx errors
and active-bin liquidity around `$25k`, but every flow row sat outside the 250-slot
join gate. A second small run also produced 0 joined rows and now exits cleanly with
`replay skipped`; this is near-slot data insufficiency, not a strategy rejection.

Orca `HYPE-USDC` final P1 coverage replay:

```text
sample A: scanned 121 signatures, kept 120 normalized swaps, tx_errors=0
span:     slot 429687986..429691923, about 26.2 minutes, tick 27496..27423 after inversion
```

This was the remaining replayable Orca P1 candidate: ~123% 24h fee APR, 16bps fee
tier, ~$59k TVL, ~$126k 24h volume, volume/TVL near 2.12. It was a trend-risk sample,
not an alpha lead. Full replay had hold at about `+$36.49`, narrow LP at `+$35.56`,
`delta_hedged` at about `-$0.86`, and `hedged_wide` near flat. Promotion rejected all
views: lagged p05 APR was about `-882%`, `-452%`, `-754%`, and `224%` across
25/40/60/80-swap windows, with negative mean edge versus hold; direct `delta_hedged`
and `hedged_wide` gates also rejected. `delta_trend_stop` also rejected, with p05 APR
still deeply negative under threshold scans. This completes the current Orca
Whirlpool P1 coverage pass. Next useful work is historical active-liquidity
reconstruction or real Meteora DLMM bin ingestion.

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
