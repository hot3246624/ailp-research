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

Do not put private keys or seed phrases on the server for the current research phase. Only read-only RPC access is required.
