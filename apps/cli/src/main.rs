use anyhow::Result;
use autopool_aerodrome::{
    BASE_CHAIN_ID, BASE_SLIPSTREAM_GAUGES_V3, base_slipstream_factories_latest_first,
    build_pilot_universe,
};
use autopool_defillama::{DefiLlamaClient, PoolFilter};
use autopool_evm::{BURN_TOPIC, COLLECT_TOPIC, JsonRpcClient, MINT_TOPIC, SWAP_TOPIC};
use autopool_strategy::WeightedRiskModel;
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Table,
    Json,
}

#[derive(Debug, Parser)]
#[command(name = "autopool")]
#[command(about = "AILP research CLI for autonomous DEX LP range management")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Architecture,
    ScanYields {
        #[arg(long = "chain")]
        chains: Vec<String>,
        #[arg(long = "project")]
        projects: Vec<String>,
        #[arg(long, default_value_t = 1_000_000.0)]
        min_tvl_usd: f64,
        #[arg(long, default_value_t = 100_000.0)]
        min_volume_usd_1d: f64,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long, default_value_t = false)]
        lp_only: bool,
    },
    PilotUniverse {
        #[arg(long, default_value_t = 100_000.0)]
        min_tvl_usd: f64,
        #[arg(long, default_value_t = 100_000.0)]
        min_volume_usd_1d: f64,
        #[arg(long, default_value_t = 0.5)]
        max_reward_share: f64,
        #[arg(long, default_value_t = 12)]
        limit: usize,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    SampleBaseNetwork {
        #[arg(long, env = "BASE_RPC_URL")]
        rpc_url: String,
        #[arg(long, default_value_t = 900_000)]
        rebalance_gas_units: u64,
        #[arg(long)]
        eth_usd: Option<f64>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    ResolveSlipstreamPools {
        #[arg(long, env = "BASE_RPC_URL")]
        rpc_url: String,
        #[arg(long, default_value_t = 100_000.0)]
        min_tvl_usd: f64,
        #[arg(long, default_value_t = 100_000.0)]
        min_volume_usd_1d: f64,
        #[arg(long, default_value_t = 0.5)]
        max_reward_share: f64,
        #[arg(long, default_value_t = 8)]
        limit: usize,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    SampleSlipstreamEvents {
        #[arg(long, env = "BASE_RPC_URL")]
        rpc_url: String,
        #[arg(long, default_value_t = 10)]
        lookback_blocks: u64,
        #[arg(long, default_value_t = 10)]
        log_chunk_blocks: u64,
        #[arg(long, default_value_t = 100_000.0)]
        min_tvl_usd: f64,
        #[arg(long, default_value_t = 100_000.0)]
        min_volume_usd_1d: f64,
        #[arg(long, default_value_t = 0.5)]
        max_reward_share: f64,
        #[arg(long, default_value_t = 4)]
        limit: usize,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    BackfillSlipstreamEvents {
        #[arg(long, env = "BASE_RPC_URL")]
        rpc_url: String,
        #[arg(long, default_value = "data/base/aerodrome")]
        data_dir: PathBuf,
        #[arg(long, default_value_t = 7_200)]
        lookback_blocks: u64,
        #[arg(long, default_value_t = 100)]
        max_blocks_per_run: u64,
        #[arg(long, default_value_t = 10)]
        log_chunk_blocks: u64,
        #[arg(long, default_value_t = 250)]
        sleep_ms: u64,
        #[arg(long, default_value_t = 30)]
        poll_seconds: u64,
        #[arg(long, default_value_t = 1)]
        iterations: u64,
        #[arg(long, default_value_t = 100_000.0)]
        min_tvl_usd: f64,
        #[arg(long, default_value_t = 100_000.0)]
        min_volume_usd_1d: f64,
        #[arg(long, default_value_t = 0.5)]
        max_reward_share: f64,
        #[arg(long, default_value_t = 4)]
        limit: usize,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Architecture => print_architecture(),
        Command::ScanYields {
            chains,
            projects,
            min_tvl_usd,
            min_volume_usd_1d,
            limit,
            lp_only,
        } => {
            scan_yields(
                chains,
                projects,
                min_tvl_usd,
                min_volume_usd_1d,
                limit,
                lp_only,
            )
            .await?
        }
        Command::PilotUniverse {
            min_tvl_usd,
            min_volume_usd_1d,
            max_reward_share,
            limit,
            format,
        } => {
            pilot_universe(
                min_tvl_usd,
                min_volume_usd_1d,
                max_reward_share,
                limit,
                format,
            )
            .await?
        }
        Command::SampleBaseNetwork {
            rpc_url,
            rebalance_gas_units,
            eth_usd,
            format,
        } => sample_base_network(rpc_url, rebalance_gas_units, eth_usd, format).await?,
        Command::ResolveSlipstreamPools {
            rpc_url,
            min_tvl_usd,
            min_volume_usd_1d,
            max_reward_share,
            limit,
            format,
        } => {
            resolve_slipstream_pools(
                rpc_url,
                min_tvl_usd,
                min_volume_usd_1d,
                max_reward_share,
                limit,
                format,
            )
            .await?
        }
        Command::SampleSlipstreamEvents {
            rpc_url,
            lookback_blocks,
            log_chunk_blocks,
            min_tvl_usd,
            min_volume_usd_1d,
            max_reward_share,
            limit,
            format,
        } => {
            sample_slipstream_events(
                rpc_url,
                lookback_blocks,
                log_chunk_blocks,
                min_tvl_usd,
                min_volume_usd_1d,
                max_reward_share,
                limit,
                format,
            )
            .await?
        }
        Command::BackfillSlipstreamEvents {
            rpc_url,
            data_dir,
            lookback_blocks,
            max_blocks_per_run,
            log_chunk_blocks,
            sleep_ms,
            poll_seconds,
            iterations,
            min_tvl_usd,
            min_volume_usd_1d,
            max_reward_share,
            limit,
        } => {
            backfill_slipstream_events(BackfillConfig {
                rpc_url,
                data_dir,
                lookback_blocks,
                max_blocks_per_run,
                log_chunk_blocks,
                sleep_ms,
                poll_seconds,
                iterations,
                min_tvl_usd,
                min_volume_usd_1d,
                max_reward_share,
                limit,
            })
            .await?
        }
    }

    Ok(())
}

fn print_architecture() {
    println!("AILP architecture");
    println!("  product    : AI-assisted autonomous liquidity provision");
    println!("  discovery  : DeFiLlama and other public yield sources");
    println!("  evm state  : pool ticks, liquidity, fee growth, wallet positions");
    println!("  strategy   : candidate scoring, range proposal, rebalance decision");
    println!("  risk       : hard gates before any execution");
    println!("  backtest   : replay tick paths and attribute PnL");
    println!("  execution  : simulate, sign, submit, and ledger outcomes");
    println!("  pilot      : Base / Aerodrome Slipstream first");
}

async fn scan_yields(
    chains: Vec<String>,
    projects: Vec<String>,
    min_tvl_usd: f64,
    min_volume_usd_1d: f64,
    limit: usize,
    lp_only: bool,
) -> Result<()> {
    let client = DefiLlamaClient::default();
    let filter = PoolFilter {
        chains,
        projects,
        min_tvl_usd: Some(min_tvl_usd),
        lp_only,
        dex_only: lp_only,
        exclude_outliers: true,
    };
    let mut risk_model = WeightedRiskModel::default();
    risk_model.limits.min_tvl_usd = min_tvl_usd;
    risk_model.limits.min_volume_usd_1d = min_volume_usd_1d;
    let mut rows = client
        .fetch_snapshots(&filter)
        .await?
        .into_iter()
        .map(|snapshot| {
            let score = risk_model.score_yield(&snapshot);
            (score.score, score.accepted, score.reasons, snapshot)
        })
        .collect::<Vec<_>>();

    rows.sort_by(|left, right| right.0.total_cmp(&left.0));

    println!(
        "{:<12} {:<18} {:<18} {:>7} {:>10} {:>9} {:>9} {:>9}  reason",
        "chain", "protocol", "symbol", "fee_bps", "tvl_usd", "apy", "base", "score"
    );

    for (score, accepted, reasons, snapshot) in rows.into_iter().take(limit) {
        println!(
            "{:<12} {:<18} {:<18} {:>7} {:>10.0} {:>8.2}% {:>8.2}% {:>9.2}  {}{}",
            snapshot.pool.chain.name(),
            format!("{:?}", snapshot.pool.protocol),
            snapshot.pool.symbol,
            snapshot
                .pool
                .fee_tier_bps
                .map(|value| format!("{value:.2}"))
                .unwrap_or_else(|| "-".to_string()),
            snapshot.tvl_usd.unwrap_or(0.0),
            snapshot.apy.unwrap_or(0.0),
            snapshot.apy_base.unwrap_or(0.0),
            score,
            if accepted { "" } else { "REJECTED: " },
            reasons.join("; ")
        );
    }

    Ok(())
}

async fn pilot_universe(
    min_tvl_usd: f64,
    min_volume_usd_1d: f64,
    max_reward_share: f64,
    limit: usize,
    format: OutputFormat,
) -> Result<()> {
    let client = DefiLlamaClient::default();
    let snapshots = client
        .fetch_snapshots(&PoolFilter {
            chains: vec!["Base".to_string()],
            projects: vec!["aerodrome-slipstream".to_string()],
            min_tvl_usd: Some(min_tvl_usd),
            lp_only: true,
            dex_only: true,
            exclude_outliers: true,
        })
        .await?;
    let universe =
        build_pilot_universe(&snapshots, min_tvl_usd, min_volume_usd_1d, max_reward_share);

    if format == OutputFormat::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&universe.into_iter().take(limit).collect::<Vec<_>>())?
        );
        return Ok(());
    }

    println!("base/aerodrome slipstream pilot universe");
    println!(
        "contracts: pool_factory={} position_manager={} swap_router={}",
        BASE_SLIPSTREAM_GAUGES_V3.pool_factory,
        BASE_SLIPSTREAM_GAUGES_V3.nonfungible_position_manager,
        BASE_SLIPSTREAM_GAUGES_V3.swap_router
    );
    println!(
        "{:<18} {:<20} {:>8} {:>10} {:>12} {:>8} {:>8} {:>8}",
        "bucket", "symbol", "fee_bps", "tvl_usd", "vol_1d", "base", "reward", "r_share"
    );

    for candidate in universe.into_iter().take(limit) {
        println!(
            "{:<18} {:<20} {:>8} {:>10.0} {:>12.0} {:>7.2}% {:>7.2}% {:>7.2}",
            candidate.pilot_bucket,
            candidate.symbol,
            candidate
                .fee_tier_bps
                .map(|value| format!("{value:.2}"))
                .unwrap_or_else(|| "-".to_string()),
            candidate.tvl_usd,
            candidate.volume_usd_1d,
            candidate.apy_base,
            candidate.apy_reward,
            candidate.reward_share,
        );
    }

    Ok(())
}

async fn sample_base_network(
    rpc_url: String,
    rebalance_gas_units: u64,
    eth_usd: Option<f64>,
    format: OutputFormat,
) -> Result<()> {
    let sample = JsonRpcClient::new(rpc_url)
        .sample_network(BASE_CHAIN_ID, rebalance_gas_units, eth_usd)
        .await?;

    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&sample)?);
        return Ok(());
    }

    println!("base network sample");
    println!("  chain_id: {}", sample.chain_id);
    println!("  block_number: {}", sample.block_number);
    println!("  gas_price_gwei: {:.6}", sample.gas_price_gwei);
    println!("  rebalance_gas_units: {}", sample.rebalance_gas_units);
    println!(
        "  estimated_rebalance_gas_eth: {:.8}",
        sample.estimated_rebalance_gas_eth
    );
    if let Some(value) = sample.estimated_rebalance_gas_usd {
        println!("  estimated_rebalance_gas_usd: {:.4}", value);
    }

    Ok(())
}

async fn resolve_slipstream_pools(
    rpc_url: String,
    min_tvl_usd: f64,
    min_volume_usd_1d: f64,
    max_reward_share: f64,
    limit: usize,
    format: OutputFormat,
) -> Result<()> {
    let client = DefiLlamaClient::default();
    let snapshots = client
        .fetch_snapshots(&PoolFilter {
            chains: vec!["Base".to_string()],
            projects: vec!["aerodrome-slipstream".to_string()],
            min_tvl_usd: Some(min_tvl_usd),
            lp_only: true,
            dex_only: true,
            exclude_outliers: true,
        })
        .await?;
    let universe =
        build_pilot_universe(&snapshots, min_tvl_usd, min_volume_usd_1d, max_reward_share);
    let rpc = JsonRpcClient::new(rpc_url);
    let factories = base_slipstream_factories_latest_first();
    let mut resolved = Vec::new();

    for candidate in universe.into_iter().take(limit) {
        let Some(tick_spacing) = candidate.tick_spacing else {
            continue;
        };
        if candidate.underlying_tokens.len() != 2 {
            continue;
        }

        let token0 = &candidate.underlying_tokens[0];
        let token1 = &candidate.underlying_tokens[1];

        for factory in factories {
            if let Some(pool_address) = rpc
                .get_cl_pool(factory.pool_factory, token0, token1, tick_spacing)
                .await?
            {
                let state = rpc.read_cl_pool_state(&pool_address).await?;
                resolved.push((candidate.clone(), factory.deployment, state));
                break;
            }
        }
    }

    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&resolved)?);
        return Ok(());
    }

    println!(
        "{:<18} {:<20} {:<10} {:<42} {:>8} {:>8} {:>16}",
        "bucket", "symbol", "deploy", "pool", "tick", "spacing", "liquidity"
    );
    for (candidate, deployment, state) in resolved {
        println!(
            "{:<18} {:<20} {:<10?} {:<42} {:>8} {:>8} {:>16}",
            candidate.pilot_bucket,
            candidate.symbol,
            deployment,
            state.pool_address,
            state.current_tick,
            state.tick_spacing,
            state.liquidity,
        );
    }

    Ok(())
}

async fn sample_slipstream_events(
    rpc_url: String,
    lookback_blocks: u64,
    log_chunk_blocks: u64,
    min_tvl_usd: f64,
    min_volume_usd_1d: f64,
    max_reward_share: f64,
    limit: usize,
    format: OutputFormat,
) -> Result<()> {
    let client = DefiLlamaClient::default();
    let snapshots = client
        .fetch_snapshots(&PoolFilter {
            chains: vec!["Base".to_string()],
            projects: vec!["aerodrome-slipstream".to_string()],
            min_tvl_usd: Some(min_tvl_usd),
            lp_only: true,
            dex_only: true,
            exclude_outliers: true,
        })
        .await?;
    let universe =
        build_pilot_universe(&snapshots, min_tvl_usd, min_volume_usd_1d, max_reward_share);
    let rpc = JsonRpcClient::new(rpc_url);
    let to_block = rpc.latest_block_number().await?;
    let from_block = to_block.saturating_sub(lookback_blocks.saturating_sub(1));
    let factories = base_slipstream_factories_latest_first();
    let mut summaries = Vec::new();

    for candidate in universe.into_iter().take(limit) {
        let Some(tick_spacing) = candidate.tick_spacing else {
            continue;
        };
        if candidate.underlying_tokens.len() != 2 {
            continue;
        }

        let token0 = &candidate.underlying_tokens[0];
        let token1 = &candidate.underlying_tokens[1];

        for factory in factories {
            if let Some(pool_address) = rpc
                .get_cl_pool(factory.pool_factory, token0, token1, tick_spacing)
                .await?
            {
                let summary = rpc
                    .pool_event_summary_chunked(
                        &pool_address,
                        from_block,
                        to_block,
                        log_chunk_blocks,
                    )
                    .await?;
                summaries.push((candidate.clone(), factory.deployment, summary));
                break;
            }
        }
    }

    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&summaries)?);
        return Ok(());
    }

    println!("event window: {from_block}..{to_block}");
    println!(
        "{:<18} {:<20} {:<10} {:>7} {:>7} {:>7} {:>8} {:>12}",
        "bucket", "symbol", "deploy", "swaps", "mints", "burns", "collects", "last_tick"
    );
    for (candidate, deployment, summary) in summaries {
        println!(
            "{:<18} {:<20} {:<10?} {:>7} {:>7} {:>7} {:>8} {:>12}",
            candidate.pilot_bucket,
            candidate.symbol,
            deployment,
            summary.swap_count,
            summary.mint_count,
            summary.burn_count,
            summary.collect_count,
            summary
                .latest_swap_tick
                .map(|tick| tick.to_string())
                .unwrap_or_else(|| "-".to_string()),
        );
    }

    Ok(())
}

#[derive(Debug)]
struct BackfillConfig {
    rpc_url: String,
    data_dir: PathBuf,
    lookback_blocks: u64,
    max_blocks_per_run: u64,
    log_chunk_blocks: u64,
    sleep_ms: u64,
    poll_seconds: u64,
    iterations: u64,
    min_tvl_usd: f64,
    min_volume_usd_1d: f64,
    max_reward_share: f64,
    limit: usize,
}

#[derive(Debug, Clone)]
struct ResolvedCandidate {
    candidate: autopool_aerodrome::SlipstreamCandidate,
    deployment: autopool_aerodrome::SlipstreamDeployment,
    pool_address: String,
}

#[derive(Debug, Clone, Copy)]
struct EventTopic {
    name: &'static str,
    topic: &'static str,
}

#[derive(Debug, Serialize)]
struct BackfillProgress<'a> {
    symbol: &'a str,
    pool_address: &'a str,
    from_block: u64,
    to_block: u64,
    events_written: usize,
    next_block: u64,
}

const EVENT_TOPICS: [EventTopic; 4] = [
    EventTopic {
        name: "swap",
        topic: SWAP_TOPIC,
    },
    EventTopic {
        name: "mint",
        topic: MINT_TOPIC,
    },
    EventTopic {
        name: "burn",
        topic: BURN_TOPIC,
    },
    EventTopic {
        name: "collect",
        topic: COLLECT_TOPIC,
    },
];

async fn backfill_slipstream_events(config: BackfillConfig) -> Result<()> {
    let rpc = JsonRpcClient::new(config.rpc_url.clone());
    let resolved = resolve_pilot_candidates(
        &config.rpc_url,
        config.min_tvl_usd,
        config.min_volume_usd_1d,
        config.max_reward_share,
        config.limit,
    )
    .await?;

    fs::create_dir_all(config.data_dir.join("events"))?;
    fs::create_dir_all(config.data_dir.join("checkpoints"))?;

    let mut iteration = 0_u64;
    loop {
        iteration += 1;
        let latest = rpc.latest_block_number().await?;
        let mut total_written = 0_usize;

        for item in &resolved {
            let checkpoint_path = checkpoint_path(&config.data_dir, &item.pool_address);
            let default_start = latest.saturating_sub(config.lookback_blocks.saturating_sub(1));
            let start = read_next_block(&checkpoint_path)?.unwrap_or(default_start);

            if start > latest {
                continue;
            }

            let to_block = start
                .saturating_add(config.max_blocks_per_run.saturating_sub(1))
                .min(latest);
            let written = backfill_candidate_window(&rpc, &config, item, start, to_block).await?;
            write_checkpoint(&checkpoint_path, item, to_block + 1)?;
            total_written += written;

            println!(
                "{}",
                serde_json::to_string(&BackfillProgress {
                    symbol: &item.candidate.symbol,
                    pool_address: &item.pool_address,
                    from_block: start,
                    to_block,
                    events_written: written,
                    next_block: to_block + 1,
                })?
            );
        }

        eprintln!("iteration={iteration} latest={latest} total_events_written={total_written}");

        if config.iterations != 0 && iteration >= config.iterations {
            break;
        }

        tokio::time::sleep(Duration::from_secs(config.poll_seconds)).await;
    }

    Ok(())
}

async fn resolve_pilot_candidates(
    rpc_url: &str,
    min_tvl_usd: f64,
    min_volume_usd_1d: f64,
    max_reward_share: f64,
    limit: usize,
) -> Result<Vec<ResolvedCandidate>> {
    let client = DefiLlamaClient::default();
    let snapshots = client
        .fetch_snapshots(&PoolFilter {
            chains: vec!["Base".to_string()],
            projects: vec!["aerodrome-slipstream".to_string()],
            min_tvl_usd: Some(min_tvl_usd),
            lp_only: true,
            dex_only: true,
            exclude_outliers: true,
        })
        .await?;
    let universe =
        build_pilot_universe(&snapshots, min_tvl_usd, min_volume_usd_1d, max_reward_share);
    let rpc = JsonRpcClient::new(rpc_url.to_string());
    let factories = base_slipstream_factories_latest_first();
    let mut resolved = Vec::new();

    for candidate in universe.into_iter().take(limit) {
        let Some(tick_spacing) = candidate.tick_spacing else {
            continue;
        };
        if candidate.underlying_tokens.len() != 2 {
            continue;
        }

        let token0 = &candidate.underlying_tokens[0];
        let token1 = &candidate.underlying_tokens[1];

        for factory in factories {
            if let Some(pool_address) = rpc
                .get_cl_pool(factory.pool_factory, token0, token1, tick_spacing)
                .await?
            {
                resolved.push(ResolvedCandidate {
                    candidate: candidate.clone(),
                    deployment: factory.deployment,
                    pool_address,
                });
                break;
            }
        }
    }

    Ok(resolved)
}

async fn backfill_candidate_window(
    rpc: &JsonRpcClient,
    config: &BackfillConfig,
    item: &ResolvedCandidate,
    from_block: u64,
    to_block: u64,
) -> Result<usize> {
    let mut written = 0_usize;
    let mut cursor = from_block;

    while cursor <= to_block {
        let chunk_end = cursor
            .saturating_add(config.log_chunk_blocks.saturating_sub(1))
            .min(to_block);

        for event_topic in EVENT_TOPICS {
            let logs = rpc
                .get_logs(&item.pool_address, cursor, chunk_end, event_topic.topic)
                .await?;
            if !logs.is_empty() {
                let event_path = event_path(&config.data_dir, &item.pool_address)?;
                append_event_logs(&event_path, item, event_topic.name, &logs)?;
                written += logs.len();
            }

            if config.sleep_ms > 0 {
                tokio::time::sleep(Duration::from_millis(config.sleep_ms)).await;
            }
        }

        if chunk_end == u64::MAX {
            break;
        }
        cursor = chunk_end + 1;
    }

    Ok(written)
}

fn event_path(data_dir: &Path, pool_address: &str) -> Result<PathBuf> {
    let dir = data_dir
        .join("events")
        .join(pool_address.to_ascii_lowercase());
    fs::create_dir_all(&dir)?;
    Ok(dir.join("events.jsonl"))
}

fn checkpoint_path(data_dir: &Path, pool_address: &str) -> PathBuf {
    data_dir
        .join("checkpoints")
        .join(format!("{}.json", pool_address.to_ascii_lowercase()))
}

fn read_next_block(path: &Path) -> Result<Option<u64>> {
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(path)?;
    let value = serde_json::from_str::<serde_json::Value>(&raw)?;
    Ok(value.get("next_block").and_then(|value| value.as_u64()))
}

fn write_checkpoint(path: &Path, item: &ResolvedCandidate, next_block: u64) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(
        path,
        serde_json::to_string_pretty(&json!({
            "symbol": item.candidate.symbol,
            "pool_address": item.pool_address,
            "deployment": format!("{:?}", item.deployment),
            "next_block": next_block,
            "updated_unix": current_unix_timestamp(),
        }))?,
    )?;

    Ok(())
}

fn append_event_logs(
    path: &Path,
    item: &ResolvedCandidate,
    event_type: &str,
    logs: &[autopool_evm::EthLog],
) -> Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;

    for log in logs {
        writeln!(
            file,
            "{}",
            serde_json::to_string(&json!({
                "chain": "Base",
                "project": "aerodrome-slipstream",
                "symbol": item.candidate.symbol,
                "pool_address": item.pool_address,
                "deployment": format!("{:?}", item.deployment),
                "event_type": event_type,
                "log": log,
            }))?
        )?;
    }

    Ok(())
}

fn current_unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
