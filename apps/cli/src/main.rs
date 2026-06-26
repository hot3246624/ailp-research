use anyhow::{Context, Result};
use autopool_aerodrome::{
    BASE_CHAIN_ID, BASE_SLIPSTREAM_GAUGES_V3, PilotProfile, SlipstreamCandidate,
    base_slipstream_factories_latest_first, build_pilot_universe_for_profile,
};
use autopool_core::YieldSnapshot;
use autopool_defillama::{DefiLlamaClient, PoolFilter};
use autopool_evm::{BURN_TOPIC, COLLECT_TOPIC, JsonRpcClient, MINT_TOPIC, SWAP_TOPIC};
use autopool_strategy::WeightedRiskModel;
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Table,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CandidateProfile {
    Control,
    Opportunistic,
}

impl From<CandidateProfile> for PilotProfile {
    fn from(value: CandidateProfile) -> Self {
        match value {
            CandidateProfile::Control => Self::Control,
            CandidateProfile::Opportunistic => Self::Opportunistic,
        }
    }
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
        #[arg(long, default_value_t = 0.0)]
        min_volume_usd_1d: f64,
        #[arg(long, default_value_t = 1.0)]
        max_reward_share: f64,
        #[arg(long, default_value_t = 0.0)]
        min_apy: f64,
        #[arg(long, default_value_t = 0.0)]
        min_fee_bps: f64,
        #[arg(long, value_enum, default_value_t = CandidateProfile::Opportunistic)]
        profile: CandidateProfile,
        #[arg(long = "include-symbol")]
        include_symbols: Vec<String>,
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
        #[arg(long, default_value_t = 0.0)]
        min_volume_usd_1d: f64,
        #[arg(long, default_value_t = 1.0)]
        max_reward_share: f64,
        #[arg(long, default_value_t = 0.0)]
        min_apy: f64,
        #[arg(long, default_value_t = 0.0)]
        min_fee_bps: f64,
        #[arg(long, value_enum, default_value_t = CandidateProfile::Opportunistic)]
        profile: CandidateProfile,
        #[arg(long = "include-symbol")]
        include_symbols: Vec<String>,
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
        #[arg(long, default_value_t = 0.0)]
        min_volume_usd_1d: f64,
        #[arg(long, default_value_t = 1.0)]
        max_reward_share: f64,
        #[arg(long, default_value_t = 0.0)]
        min_apy: f64,
        #[arg(long, default_value_t = 0.0)]
        min_fee_bps: f64,
        #[arg(long, value_enum, default_value_t = CandidateProfile::Opportunistic)]
        profile: CandidateProfile,
        #[arg(long = "include-symbol")]
        include_symbols: Vec<String>,
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
        #[arg(long, default_value_t = 0.0)]
        min_volume_usd_1d: f64,
        #[arg(long, default_value_t = 1.0)]
        max_reward_share: f64,
        #[arg(long, default_value_t = 0.0)]
        min_apy: f64,
        #[arg(long, default_value_t = 0.0)]
        min_fee_bps: f64,
        #[arg(long, value_enum, default_value_t = CandidateProfile::Opportunistic)]
        profile: CandidateProfile,
        #[arg(long = "include-symbol")]
        include_symbols: Vec<String>,
        #[arg(long, default_value_t = 4)]
        limit: usize,
    },
    SummarizeSlipstreamEvents {
        #[arg(long, default_value = "data/base/aerodrome")]
        data_dir: PathBuf,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    /// Replay collected swap events through baseline LP range policies and report
    /// PnL attribution (fees, inventory IL, gas, slippage) versus a hold baseline.
    ReplayEvents {
        #[arg(long, default_value = "data/base/aerodrome-opportunistic")]
        data_dir: PathBuf,
        /// Pool address to replay. If omitted, replays the pool with the most swaps.
        #[arg(long)]
        pool_address: Option<String>,
        /// Match a pool by symbol (e.g. WETH-AERO) instead of address.
        #[arg(long)]
        symbol: Option<String>,
        /// Pool fee tier in basis points (30 bps = 0.30%).
        #[arg(long, default_value_t = 30.0)]
        fee_bps: f64,
        /// Decimals of token0 (lower-address token). WETH=18.
        #[arg(long, default_value_t = 18)]
        decimals0: u8,
        /// Decimals of token1 (higher-address token). AERO=18, USDC=6.
        #[arg(long, default_value_t = 18)]
        decimals1: u8,
        /// USD value of one token0 (numeraire anchor). WETH ~ 3300.
        #[arg(long, default_value_t = 3300.0)]
        token0_usd: f64,
        #[arg(long, default_value_t = 10_000.0)]
        capital_usd: f64,
        /// Gas cost per rebalance in USD (Base L2 is cheap).
        #[arg(long, default_value_t = 0.05)]
        rebalance_gas_usd: f64,
        #[arg(long, default_value_t = 5.0)]
        rebalance_slippage_bps: f64,
        /// Half-width (ticks) for the narrow policies.
        #[arg(long, default_value_t = 600)]
        narrow_half_width: i32,
        /// Half-width (ticks) for the passive-wide policy.
        #[arg(long, default_value_t = 6_000)]
        wide_half_width: i32,
        /// Volatility multiplier for the vol-scaled policy.
        #[arg(long, default_value_t = 1.5)]
        vol_k: f64,
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
            min_apy,
            min_fee_bps,
            profile,
            include_symbols,
            limit,
            format,
        } => {
            pilot_universe(
                min_tvl_usd,
                min_volume_usd_1d,
                max_reward_share,
                min_apy,
                min_fee_bps,
                profile,
                include_symbols,
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
            min_apy,
            min_fee_bps,
            profile,
            include_symbols,
            limit,
            format,
        } => {
            resolve_slipstream_pools(
                rpc_url,
                min_tvl_usd,
                min_volume_usd_1d,
                max_reward_share,
                min_apy,
                min_fee_bps,
                profile,
                include_symbols,
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
            min_apy,
            min_fee_bps,
            profile,
            include_symbols,
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
                min_apy,
                min_fee_bps,
                profile,
                include_symbols,
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
            min_apy,
            min_fee_bps,
            profile,
            include_symbols,
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
                min_apy,
                min_fee_bps,
                profile,
                include_symbols,
                limit,
            })
            .await?
        }
        Command::SummarizeSlipstreamEvents { data_dir, format } => {
            summarize_slipstream_events(data_dir, format)?
        }
        Command::ReplayEvents {
            data_dir,
            pool_address,
            symbol,
            fee_bps,
            decimals0,
            decimals1,
            token0_usd,
            capital_usd,
            rebalance_gas_usd,
            rebalance_slippage_bps,
            narrow_half_width,
            wide_half_width,
            vol_k,
            format,
        } => replay_events(ReplayArgs {
            data_dir,
            pool_address,
            symbol,
            fee_bps,
            decimals0,
            decimals1,
            token0_usd,
            capital_usd,
            rebalance_gas_usd,
            rebalance_slippage_bps,
            narrow_half_width,
            wide_half_width,
            vol_k,
            format,
        })?,
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

#[derive(Debug, Clone)]
struct CandidateSelection {
    min_tvl_usd: f64,
    min_volume_usd_1d: f64,
    max_reward_share: f64,
    min_apy: f64,
    min_fee_bps: f64,
    profile: CandidateProfile,
    include_symbols: Vec<String>,
}

fn select_slipstream_candidates(
    snapshots: &[YieldSnapshot],
    selection: &CandidateSelection,
) -> Vec<SlipstreamCandidate> {
    let selected = build_pilot_universe_for_profile(
        snapshots,
        selection.min_tvl_usd,
        selection.min_volume_usd_1d,
        selection.max_reward_share,
        selection.profile.into(),
    )
    .into_iter()
    .filter(|candidate| candidate.apy >= selection.min_apy)
    .filter(|candidate| candidate.fee_tier_bps.unwrap_or(0.0) >= selection.min_fee_bps)
    .collect::<Vec<_>>();

    if selection.include_symbols.is_empty() {
        return selected;
    }

    let all_candidates = snapshots
        .iter()
        .filter(|snapshot| !snapshot.outlier)
        .filter_map(SlipstreamCandidate::from_yield)
        .filter(|candidate| candidate.tvl_usd >= selection.min_tvl_usd)
        .collect::<Vec<_>>();
    let mut forced = Vec::new();
    let mut forced_keys = BTreeSet::new();

    for symbol in &selection.include_symbols {
        let key = symbol_key(symbol);
        if forced_keys.contains(&key) {
            continue;
        }

        if let Some(candidate) = all_candidates
            .iter()
            .find(|candidate| symbol_key(&candidate.symbol) == key)
        {
            forced.push(candidate.clone());
            forced_keys.insert(key);
        }
    }

    let mut merged = forced;
    merged.extend(
        selected
            .into_iter()
            .filter(|candidate| !forced_keys.contains(&symbol_key(&candidate.symbol))),
    );
    merged
}

fn symbol_key(symbol: &str) -> String {
    symbol
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_uppercase()
}

async fn pilot_universe(
    min_tvl_usd: f64,
    min_volume_usd_1d: f64,
    max_reward_share: f64,
    min_apy: f64,
    min_fee_bps: f64,
    profile: CandidateProfile,
    include_symbols: Vec<String>,
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
    let selection = CandidateSelection {
        min_tvl_usd,
        min_volume_usd_1d,
        max_reward_share,
        min_apy,
        min_fee_bps,
        profile,
        include_symbols,
    };
    let universe = select_slipstream_candidates(&snapshots, &selection);

    if format == OutputFormat::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&universe.into_iter().take(limit).collect::<Vec<_>>())?
        );
        return Ok(());
    }

    println!(
        "base/aerodrome slipstream pilot universe ({:?})",
        selection.profile
    );
    println!(
        "contracts: pool_factory={} position_manager={} swap_router={}",
        BASE_SLIPSTREAM_GAUGES_V3.pool_factory,
        BASE_SLIPSTREAM_GAUGES_V3.nonfungible_position_manager,
        BASE_SLIPSTREAM_GAUGES_V3.swap_router
    );
    println!(
        "{:<24} {:<20} {:>8} {:>10} {:>12} {:>8} {:>8} {:>8}",
        "bucket", "symbol", "fee_bps", "tvl_usd", "vol_1d", "base", "reward", "r_share"
    );

    for candidate in universe.into_iter().take(limit) {
        println!(
            "{:<24} {:<20} {:>8} {:>10.0} {:>12.0} {:>7.2}% {:>7.2}% {:>7.2}",
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
    min_apy: f64,
    min_fee_bps: f64,
    profile: CandidateProfile,
    include_symbols: Vec<String>,
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
    let selection = CandidateSelection {
        min_tvl_usd,
        min_volume_usd_1d,
        max_reward_share,
        min_apy,
        min_fee_bps,
        profile,
        include_symbols,
    };
    let universe = select_slipstream_candidates(&snapshots, &selection);
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
        "{:<24} {:<20} {:<10} {:<42} {:>8} {:>8} {:>16}",
        "bucket", "symbol", "deploy", "pool", "tick", "spacing", "liquidity"
    );
    for (candidate, deployment, state) in resolved {
        println!(
            "{:<24} {:<20} {:<10?} {:<42} {:>8} {:>8} {:>16}",
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
    min_apy: f64,
    min_fee_bps: f64,
    profile: CandidateProfile,
    include_symbols: Vec<String>,
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
    let selection = CandidateSelection {
        min_tvl_usd,
        min_volume_usd_1d,
        max_reward_share,
        min_apy,
        min_fee_bps,
        profile,
        include_symbols,
    };
    let universe = select_slipstream_candidates(&snapshots, &selection);
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
        "{:<24} {:<20} {:<10} {:>7} {:>7} {:>7} {:>8} {:>12}",
        "bucket", "symbol", "deploy", "swaps", "mints", "burns", "collects", "last_tick"
    );
    for (candidate, deployment, summary) in summaries {
        println!(
            "{:<24} {:<20} {:<10?} {:>7} {:>7} {:>7} {:>8} {:>12}",
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
    min_apy: f64,
    min_fee_bps: f64,
    profile: CandidateProfile,
    include_symbols: Vec<String>,
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
        config.min_apy,
        config.min_fee_bps,
        config.profile,
        config.include_symbols.clone(),
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
    min_apy: f64,
    min_fee_bps: f64,
    profile: CandidateProfile,
    include_symbols: Vec<String>,
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
    let selection = CandidateSelection {
        min_tvl_usd,
        min_volume_usd_1d,
        max_reward_share,
        min_apy,
        min_fee_bps,
        profile,
        include_symbols,
    };
    let universe = select_slipstream_candidates(&snapshots, &selection);
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

#[derive(Debug, Deserialize)]
struct StoredPoolEvent {
    symbol: String,
    pool_address: String,
    event_type: String,
    log: autopool_evm::EthLog,
}

#[derive(Debug, Default)]
struct EventSummaryBuilder {
    symbol: Option<String>,
    pool_address: Option<String>,
    events: usize,
    swap_count: usize,
    mint_count: usize,
    burn_count: usize,
    collect_count: usize,
    other_count: usize,
    unique_txs: BTreeSet<String>,
    active_event_blocks: BTreeSet<u64>,
    active_swap_blocks: BTreeSet<u64>,
    block_min: Option<u64>,
    block_max: Option<u64>,
    ticks: Vec<i32>,
    first_swap: Option<(u64, u64, i32)>,
    last_swap: Option<(u64, u64, i32)>,
    token0_in_swaps: usize,
    token1_in_swaps: usize,
    other_swaps: usize,
}

#[derive(Debug, Serialize)]
struct SlipstreamEventFileSummary {
    symbol: String,
    pool_address: String,
    events: usize,
    swap_count: usize,
    mint_count: usize,
    burn_count: usize,
    collect_count: usize,
    other_count: usize,
    unique_txs: usize,
    block_min: Option<u64>,
    block_max: Option<u64>,
    span_blocks: Option<u64>,
    active_event_blocks: usize,
    active_swap_blocks: usize,
    events_per_1k_blocks: Option<f64>,
    swaps_per_1k_blocks: Option<f64>,
    tick_first: Option<i32>,
    tick_last: Option<i32>,
    tick_min: Option<i32>,
    tick_p05: Option<f64>,
    tick_p50: Option<f64>,
    tick_p95: Option<f64>,
    tick_max: Option<i32>,
    tick_span: Option<i32>,
    token0_in_swaps: usize,
    token1_in_swaps: usize,
    other_swaps: usize,
}

impl EventSummaryBuilder {
    fn add(&mut self, event: StoredPoolEvent) {
        self.symbol.get_or_insert(event.symbol);
        self.pool_address.get_or_insert(event.pool_address);
        self.events += 1;

        let block = event
            .log
            .block_number
            .as_deref()
            .and_then(parse_hex_u64_lossy);
        if let Some(block) = block {
            self.active_event_blocks.insert(block);
            self.block_min = Some(self.block_min.map_or(block, |value| value.min(block)));
            self.block_max = Some(self.block_max.map_or(block, |value| value.max(block)));
        }

        if let Some(tx_hash) = event.log.transaction_hash {
            self.unique_txs.insert(tx_hash);
        }

        match event.event_type.to_ascii_lowercase().as_str() {
            "swap" => {
                self.swap_count += 1;
                if let Some(block) = block {
                    self.active_swap_blocks.insert(block);
                    if let Some(tick) = decode_swap_tick_lossy(&event.log.data) {
                        let log_index = event
                            .log
                            .log_index
                            .as_deref()
                            .and_then(parse_hex_u64_lossy)
                            .unwrap_or_default();
                        self.add_tick(block, log_index, tick);
                    }
                }

                match decode_swap_amount_signs_lossy(&event.log.data) {
                    Some((1, -1)) => self.token0_in_swaps += 1,
                    Some((-1, 1)) => self.token1_in_swaps += 1,
                    _ => self.other_swaps += 1,
                }
            }
            "mint" => self.mint_count += 1,
            "burn" => self.burn_count += 1,
            "collect" => self.collect_count += 1,
            _ => self.other_count += 1,
        }
    }

    fn add_tick(&mut self, block: u64, log_index: u64, tick: i32) {
        let key = (block, log_index, tick);
        if self
            .first_swap
            .map_or(true, |current| (block, log_index) < (current.0, current.1))
        {
            self.first_swap = Some(key);
        }
        if self
            .last_swap
            .map_or(true, |current| (block, log_index) > (current.0, current.1))
        {
            self.last_swap = Some(key);
        }
        self.ticks.push(tick);
    }

    fn finish(self) -> SlipstreamEventFileSummary {
        let span_blocks = match (self.block_min, self.block_max) {
            (Some(min), Some(max)) => Some(max.saturating_sub(min).saturating_add(1)),
            _ => None,
        };
        let events_per_1k_blocks = span_blocks.map(|span| per_1k_blocks(self.events, span));
        let swaps_per_1k_blocks = span_blocks.map(|span| per_1k_blocks(self.swap_count, span));
        let tick_min = self.ticks.iter().min().copied();
        let tick_max = self.ticks.iter().max().copied();

        SlipstreamEventFileSummary {
            symbol: self.symbol.unwrap_or_else(|| "-".to_string()),
            pool_address: self.pool_address.unwrap_or_else(|| "-".to_string()),
            events: self.events,
            swap_count: self.swap_count,
            mint_count: self.mint_count,
            burn_count: self.burn_count,
            collect_count: self.collect_count,
            other_count: self.other_count,
            unique_txs: self.unique_txs.len(),
            block_min: self.block_min,
            block_max: self.block_max,
            span_blocks,
            active_event_blocks: self.active_event_blocks.len(),
            active_swap_blocks: self.active_swap_blocks.len(),
            events_per_1k_blocks,
            swaps_per_1k_blocks,
            tick_first: self.first_swap.map(|(_, _, tick)| tick),
            tick_last: self.last_swap.map(|(_, _, tick)| tick),
            tick_min,
            tick_p05: percentile_ticks(&self.ticks, 5.0),
            tick_p50: percentile_ticks(&self.ticks, 50.0),
            tick_p95: percentile_ticks(&self.ticks, 95.0),
            tick_max,
            tick_span: tick_min.zip(tick_max).map(|(min, max)| max - min),
            token0_in_swaps: self.token0_in_swaps,
            token1_in_swaps: self.token1_in_swaps,
            other_swaps: self.other_swaps,
        }
    }
}

fn summarize_slipstream_events(data_dir: PathBuf, format: OutputFormat) -> Result<()> {
    let events_dir = data_dir.join("events");
    if !events_dir.exists() {
        anyhow::bail!("event directory does not exist: {}", events_dir.display());
    }

    let mut summaries = Vec::new();
    for entry in fs::read_dir(&events_dir)? {
        let entry = entry?;
        let path = entry.path().join("events.jsonl");
        if !path.exists() {
            continue;
        }

        let file = File::open(&path)?;
        let reader = BufReader::new(file);
        let mut builder = EventSummaryBuilder::default();
        for (index, line) in reader.lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let event = serde_json::from_str::<StoredPoolEvent>(&line).with_context(|| {
                format!("failed to parse {} line {}", path.display(), index + 1)
            })?;
            builder.add(event);
        }
        summaries.push(builder.finish());
    }

    summaries.sort_by(|left, right| {
        right
            .events
            .cmp(&left.events)
            .then_with(|| left.symbol.cmp(&right.symbol))
    });

    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&summaries)?);
        return Ok(());
    }

    println!("base/aerodrome slipstream event summary");
    println!(
        "{:<18} {:>7} {:>7} {:>7} {:>8} {:>10} {:>10} {:>10} {:>13} {:>13}",
        "symbol",
        "events",
        "swaps",
        "lp_evt",
        "txs",
        "span_blk",
        "swp/kblk",
        "tick_span",
        "tick_p05/p50",
        "tick_p95"
    );
    for summary in summaries {
        let lp_events = summary.mint_count + summary.burn_count + summary.collect_count;
        println!(
            "{:<18} {:>7} {:>7} {:>7} {:>8} {:>10} {:>10} {:>10} {:>13} {:>13}",
            summary.symbol,
            summary.events,
            summary.swap_count,
            lp_events,
            summary.unique_txs,
            optional_u64(summary.span_blocks),
            optional_f64(summary.swaps_per_1k_blocks),
            optional_i32(summary.tick_span),
            format!(
                "{}/{}",
                optional_f64(summary.tick_p05),
                optional_f64(summary.tick_p50)
            ),
            optional_f64(summary.tick_p95),
        );
    }

    Ok(())
}

struct ReplayArgs {
    data_dir: PathBuf,
    pool_address: Option<String>,
    symbol: Option<String>,
    fee_bps: f64,
    decimals0: u8,
    decimals1: u8,
    token0_usd: f64,
    capital_usd: f64,
    rebalance_gas_usd: f64,
    rebalance_slippage_bps: f64,
    narrow_half_width: i32,
    wide_half_width: i32,
    vol_k: f64,
    format: OutputFormat,
}

#[derive(Debug, Serialize)]
struct ReplayReport {
    pool_address: String,
    symbol: String,
    swaps: usize,
    block_first: Option<u64>,
    block_last: Option<u64>,
    tick_first: Option<i32>,
    tick_last: Option<i32>,
    config: serde_json::Value,
    policies: Vec<autopool_backtest::PolicyReport>,
}

fn replay_events(args: ReplayArgs) -> Result<()> {
    let events_dir = args.data_dir.join("events");
    if !events_dir.exists() {
        anyhow::bail!("event directory does not exist: {}", events_dir.display());
    }

    let target = select_replay_target(
        &events_dir,
        args.pool_address.as_deref(),
        args.symbol.as_deref(),
    )?;

    let (symbol, swaps) = load_swaps(&target)?;
    if swaps.is_empty() {
        anyhow::bail!("no decodable swap events found in {}", target.display());
    }

    let pool_address = target
        .parent()
        .and_then(|parent| parent.file_name())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "-".to_string());

    let cfg = autopool_backtest::ReplayConfig {
        decimals0: args.decimals0,
        decimals1: args.decimals1,
        fee_fraction: args.fee_bps / 10_000.0,
        token0_usd: args.token0_usd,
        capital_usd: args.capital_usd,
        rebalance_gas_usd: args.rebalance_gas_usd,
        rebalance_slippage_bps: args.rebalance_slippage_bps,
        rebalance_swap_fraction: 0.5,
    };

    let policies = autopool_backtest::run_baseline_battery_with(
        &swaps,
        &cfg,
        args.narrow_half_width,
        args.wide_half_width,
        args.vol_k,
    );

    let report = ReplayReport {
        pool_address,
        symbol,
        swaps: swaps.len(),
        block_first: swaps.first().map(|swap| swap.block),
        block_last: swaps.last().map(|swap| swap.block),
        tick_first: swaps.first().map(|swap| swap.tick),
        tick_last: swaps.last().map(|swap| swap.tick),
        config: json!({
            "fee_bps": args.fee_bps,
            "decimals0": args.decimals0,
            "decimals1": args.decimals1,
            "token0_usd": args.token0_usd,
            "capital_usd": args.capital_usd,
            "rebalance_gas_usd": args.rebalance_gas_usd,
            "rebalance_slippage_bps": args.rebalance_slippage_bps,
            "narrow_half_width": args.narrow_half_width,
            "wide_half_width": args.wide_half_width,
            "vol_k": args.vol_k,
        }),
        policies,
    };

    if args.format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!(
        "replay {} ({}) swaps={} blocks={}..{} ticks={}..{}",
        report.symbol,
        report.pool_address,
        report.swaps,
        optional_u64(report.block_first),
        optional_u64(report.block_last),
        optional_i32(report.tick_first),
        optional_i32(report.tick_last),
    );
    println!(
        "config: fee={:.2}bps capital=${:.0} token0=${:.0} gas=${:.3}/reb narrow=±{} wide=±{} vol_k={}",
        args.fee_bps,
        args.capital_usd,
        args.token0_usd,
        args.rebalance_gas_usd,
        args.narrow_half_width,
        args.wide_half_width,
        args.vol_k,
    );
    println!(
        "{:<22} {:>10} {:>10} {:>9} {:>9} {:>10} {:>10} {:>8} {:>9}",
        "policy", "net_pnl", "vs_hold", "fees", "il", "gas+slip", "in_range%", "rebals", "avg_w"
    );
    for policy in &report.policies {
        println!(
            "{:<22} {:>10.2} {:>10.2} {:>9.2} {:>9.2} {:>10.2} {:>9.1}% {:>8} {:>9.0}",
            policy.policy,
            policy.net_pnl_usd,
            policy.net_vs_hold_usd,
            policy.fee_income_usd,
            policy.inventory_il_usd,
            policy.gas_cost_usd + policy.slippage_cost_usd,
            policy.time_in_range_pct,
            policy.rebalances,
            policy.avg_half_width_ticks,
        );
    }

    Ok(())
}

/// Find the events.jsonl to replay: by explicit pool address, by symbol match, or
/// fall back to the file with the most swap events.
fn select_replay_target(
    events_dir: &Path,
    pool_address: Option<&str>,
    symbol: Option<&str>,
) -> Result<PathBuf> {
    if let Some(address) = pool_address {
        let path = events_dir
            .join(address.to_ascii_lowercase())
            .join("events.jsonl");
        if !path.exists() {
            anyhow::bail!(
                "no events for pool {address} under {}",
                events_dir.display()
            );
        }
        return Ok(path);
    }

    let mut best: Option<(usize, PathBuf)> = None;
    let symbol_key = symbol.map(self::symbol_key);
    for entry in fs::read_dir(events_dir)? {
        let path = entry?.path().join("events.jsonl");
        if !path.exists() {
            continue;
        }
        let (file_symbol, swap_count) = peek_symbol_and_swap_count(&path)?;
        if let Some(want) = &symbol_key {
            if self::symbol_key(&file_symbol) != *want {
                continue;
            }
        }
        if best.as_ref().map_or(true, |(count, _)| swap_count > *count) {
            best = Some((swap_count, path));
        }
    }

    best.map(|(_, path)| path).ok_or_else(|| {
        anyhow::anyhow!(
            "no matching events.jsonl found under {}",
            events_dir.display()
        )
    })
}

fn peek_symbol_and_swap_count(path: &Path) -> Result<(String, usize)> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut symbol = String::from("-");
    let mut swaps = 0usize;
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<StoredPoolEvent>(&line) {
            if symbol == "-" {
                symbol = event.symbol.clone();
            }
            if event.event_type.eq_ignore_ascii_case("swap") {
                swaps += 1;
            }
        }
    }
    Ok((symbol, swaps))
}

fn load_swaps(path: &Path) -> Result<(String, Vec<autopool_backtest::SwapObs>)> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut symbol = String::from("-");
    let mut swaps = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let event = serde_json::from_str::<StoredPoolEvent>(&line)
            .with_context(|| format!("failed to parse {} line {}", path.display(), index + 1))?;
        if symbol == "-" {
            symbol = event.symbol.clone();
        }
        if !event.event_type.eq_ignore_ascii_case("swap") {
            continue;
        }
        let block = event
            .log
            .block_number
            .as_deref()
            .and_then(parse_hex_u64_lossy)
            .unwrap_or_default();
        let log_index = event
            .log
            .log_index
            .as_deref()
            .and_then(parse_hex_u64_lossy)
            .unwrap_or_default();
        if let Some(obs) = autopool_backtest::decode_swap_obs(&event.log.data, block, log_index) {
            swaps.push(obs);
        }
    }
    swaps.sort_by(|left, right| {
        left.block
            .cmp(&right.block)
            .then_with(|| left.log_index.cmp(&right.log_index))
    });
    Ok((symbol, swaps))
}

fn parse_hex_u64_lossy(value: &str) -> Option<u64> {
    u64::from_str_radix(value.trim_start_matches("0x"), 16).ok()
}

fn per_1k_blocks(count: usize, span_blocks: u64) -> f64 {
    if span_blocks == 0 {
        return 0.0;
    }
    ((count as f64 * 100_000.0 / span_blocks as f64).round()) / 100.0
}

fn percentile_ticks(values: &[i32], percentile: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let index = (sorted.len() - 1) as f64 * percentile / 100.0;
    let lower = index.floor() as usize;
    let upper = index.ceil() as usize;
    if lower == upper {
        Some(sorted[lower] as f64)
    } else {
        let weight_upper = index - lower as f64;
        let weight_lower = 1.0 - weight_upper;
        Some(sorted[lower] as f64 * weight_lower + sorted[upper] as f64 * weight_upper)
    }
    .map(|value| (value * 100.0).round() / 100.0)
}

fn decode_swap_tick_lossy(data: &str) -> Option<i32> {
    let words = abi_words(data)?;
    let word = words.get(4)?;
    let lower = &word[word.len().saturating_sub(6)..];
    let raw = i32::from_str_radix(lower, 16).ok()?;
    Some(if raw & 0x80_0000 != 0 {
        raw - 0x100_0000
    } else {
        raw
    })
}

fn decode_swap_amount_signs_lossy(data: &str) -> Option<(i8, i8)> {
    let words = abi_words(data)?;
    Some((
        abi_word_sign(words.first()?)?,
        abi_word_sign(words.get(1)?)?,
    ))
}

fn abi_words(data: &str) -> Option<Vec<&str>> {
    let stripped = data.strip_prefix("0x")?;
    if stripped.len() % 64 != 0 {
        return None;
    }
    Some(
        (0..stripped.len())
            .step_by(64)
            .map(|start| &stripped[start..start + 64])
            .collect(),
    )
}

fn abi_word_sign(word: &str) -> Option<i8> {
    if word.len() != 64 || !word.chars().all(|character| character.is_ascii_hexdigit()) {
        return None;
    }
    if word.chars().all(|character| character == '0') {
        return Some(0);
    }
    let first = word.as_bytes()[0].to_ascii_lowercase();
    Some(if first >= b'8' { -1 } else { 1 })
}

fn optional_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn optional_i32(value: Option<i32>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn optional_f64(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "-".to_string())
}

fn current_unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
