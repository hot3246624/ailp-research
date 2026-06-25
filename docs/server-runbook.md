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

Do not put private keys or seed phrases on the server for the current research phase. Only read-only RPC access is required.
