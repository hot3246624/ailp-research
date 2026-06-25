use anyhow::Result;
use autopool_aerodrome::{
    BASE_CHAIN_ID, BASE_SLIPSTREAM_GAUGES_V3, base_slipstream_factories_latest_first,
    build_pilot_universe,
};
use autopool_defillama::{DefiLlamaClient, PoolFilter};
use autopool_evm::JsonRpcClient;
use autopool_strategy::WeightedRiskModel;
use clap::{Parser, Subcommand, ValueEnum};

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
