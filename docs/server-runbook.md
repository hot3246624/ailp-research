# Server Runbook

Remote working directory:

```bash
~/ailp_research
```

Rust is installed in the `ubuntu` user environment:

```bash
. ~/.cargo/env
```

Build and test:

```bash
cd ~/ailp_research
. ~/.cargo/env
cargo test --workspace
```

Resolve the current Base / Aerodrome Slipstream pilot pools:

```bash
BASE_RPC_URL=https://your-base-rpc.example \
cargo run -q -p autopool-cli -- resolve-slipstream-pools \
  --min-tvl-usd 100000 \
  --min-volume-usd-1d 100000 \
  --max-reward-share 0.5 \
  --limit 4
```

Sample recent pool events with the current Alchemy free-tier limitation:

```bash
BASE_RPC_URL=https://your-base-rpc.example \
cargo run -q -p autopool-cli -- sample-slipstream-events \
  --lookback-blocks 100 \
  --log-chunk-blocks 10 \
  --min-tvl-usd 100000 \
  --min-volume-usd-1d 100000 \
  --max-reward-share 0.5 \
  --limit 4
```

Run a slow checkpointed background backfill:

```bash
cd ~/ailp_research
. ~/.cargo/env
mkdir -p ~/.config/ailp logs
chmod 700 ~/.config/ailp
printf '%s\n' 'https://your-base-rpc.example' > ~/.config/ailp/base_rpc_url
chmod 600 ~/.config/ailp/base_rpc_url

nohup bash -lc 'cd ~/ailp_research && . ~/.cargo/env && BASE_RPC_URL="$(cat ~/.config/ailp/base_rpc_url)" cargo run -q -p autopool-cli -- backfill-slipstream-events --data-dir data/base/aerodrome --lookback-blocks 7200 --max-blocks-per-run 200 --log-chunk-blocks 10 --sleep-ms 300 --poll-seconds 30 --iterations 0 --limit 4' \
  > logs/ailp-backfill.out 2> logs/ailp-backfill.err &
echo $! > logs/ailp-backfill.pid
```

Check progress:

```bash
tail -f logs/ailp-backfill.out
tail -f logs/ailp-backfill.err
find data/base/aerodrome/checkpoints -type f -maxdepth 1 -print -exec cat {} \;
find data/base/aerodrome/events -name events.jsonl -print -exec wc -l {} \;
```

Backfill an explicit pool by address (bypasses DeFiLlama resolution). The active
WETH-USDC venue is the Initial / spacing-100 / 0.5 bps pool, not the GaugeCaps pool
DeFiLlama resolves to:

```bash
nohup bash -lc 'cd ~/ailp_research && . ~/.cargo/env && BASE_RPC_URL="$(cat ~/.config/ailp/base_rpc_url)" cargo run -q -p autopool-cli -- backfill-slipstream-events --data-dir data/base/aerodrome-opportunistic --pool WETH-USDC:0xb2cc224c1c9fee385f8ad6a55b4d94e92359dc59 --lookback-blocks 7200 --max-blocks-per-run 200 --log-chunk-blocks 10 --sleep-ms 500 --poll-seconds 60 --iterations 0' \
  > logs/ailp-backfill-wethusdc.out 2> logs/ailp-backfill-wethusdc.err &
echo $! > logs/ailp-backfill-wethusdc.pid
```

Replay a collected pool through the baseline range policies:

```bash
cargo run -q -p autopool-cli -- replay-events \
  --data-dir data/base/aerodrome-opportunistic \
  --symbol WETH-AERO --fee-bps 21.25 --token0-usd 1574 --narrow-half-width 100
```

## RPC endpoints

The Alchemy free tier hard-caps `eth_getLogs` at a 10-block range, which makes
historical backfills and long scans crawl. For historical/scan-heavy work use a
public endpoint that allows large ranges (verified to accept 10,000-block getLogs):

- `https://mainnet.base.org` (official)
- `https://base.drpc.org`

Example fast historical collection (swaps-only, 2000-block chunks):

```bash
BASE_RPC_URL=https://mainnet.base.org cargo run -q -p autopool-cli -- backfill-slipstream-events \
  --data-dir data/base/aerodrome-trend \
  --pool WETH-AERO:0x4e506648d493c8870f55e870480f92f2f33ece51 \
  --swaps-only --from-block 47527000 --to-block 47627000 \
  --max-blocks-per-run 2000 --log-chunk-blocks 2000 --sleep-ms 120 --iterations 0
```

Keep the Alchemy key for the live indexers; use the public endpoint for bulk reads.

Do not put private keys or seed phrases on the server for the current research phase. Only read-only RPC access is required.
