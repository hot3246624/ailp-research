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
        /// Backfill a fixed historical window starting here instead of chasing the
        /// chain head (overrides --lookback-blocks). Use a separate --data-dir to
        /// keep checkpoints isolated from live backfills.
        #[arg(long)]
        from_block: Option<u64>,
        /// Upper bound of the historical window; the run stops once every pool has
        /// passed it (even with --iterations 0). Defaults to the chain head.
        #[arg(long)]
        to_block: Option<u64>,
        /// Collect only Swap events (skip mint/burn/collect) — 4x cheaper on RPC,
        /// sufficient for replay.
        #[arg(long, default_value_t = false)]
        swaps_only: bool,
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
        /// Backfill an explicit pool, bypassing DeFiLlama resolution. Format
        /// `SYMBOL:0xADDRESS` (e.g. `WETH-USDC:0xb2cc224c1c9fee385f8ad6a55b4d94e92359dc59`).
        /// Repeatable; when set, only these pools are backfilled.
        #[arg(long = "pool")]
        pools: Vec<String>,
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
        #[command(flatten)]
        params: ReplayParams,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    /// Stress-test the range policies against a synthetic price scenario
    /// (calm / pump / crash / chop) when collected data lacks that regime.
    ReplayScenario {
        /// Scenario: calm, pump, crash, or chop.
        #[arg(long, default_value = "crash")]
        scenario: String,
        #[arg(long, default_value_t = 80_000)]
        start_tick: i32,
        /// Number of synthetic swaps.
        #[arg(long, default_value_t = 1_500)]
        swaps: usize,
        /// Net tick travel for trending scenarios / amplitude for chop.
        /// +6000 ticks ≈ +82% pool price ≈ risk asset (token1) down ~45%.
        #[arg(long, default_value_t = 6_000)]
        move_ticks: i32,
        /// Per-swap input size in whole token0 units.
        #[arg(long, default_value_t = 1.0)]
        swap_size_token0: f64,
        /// Pool active liquidity (raw units) used for fee-share.
        #[arg(long, default_value_t = 1e24)]
        liquidity: f64,
        #[command(flatten)]
        params: ReplayParams,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    /// Walk-forward calibration of the adaptive policy: roll a train window, pick
    /// (trend-threshold, width) by a risk-adjusted score on past swaps, apply
    /// out-of-sample, and compare to fixed-param / static / hold baselines.
    WalkForward {
        #[arg(long, default_value = "data/base/aerodrome-opportunistic")]
        data_dir: PathBuf,
        #[arg(long)]
        pool_address: Option<String>,
        #[arg(long)]
        symbol: Option<String>,
        /// Training window length, in swaps.
        #[arg(long, default_value_t = 1_000)]
        train_swaps: usize,
        /// Test/step window length, in swaps.
        #[arg(long, default_value_t = 500)]
        test_swaps: usize,
        /// Trend-exit thresholds to search (repeatable).
        #[arg(long = "threshold", default_values_t = [2.0, 4.0, 6.0, 8.0, 10.0])]
        thresholds: Vec<f64>,
        /// Narrow half-widths to search (repeatable).
        #[arg(long = "half-width", default_values_t = [100, 300])]
        half_widths: Vec<i32>,
        /// Drawdown penalty in the train objective `net - penalty * maxDD`.
        #[arg(long, default_value_t = 0.5)]
        drawdown_penalty: f64,
        #[command(flatten)]
        params: ReplayParams,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    /// Discover liquid AND volatile pools: resolve the candidate universe on-chain,
    /// measure realized tick volatility from recent swaps, and rank. Finds pairs
    /// where active range management actually has something to manage.
    ScanPoolActivity {
        #[arg(long, env = "BASE_RPC_URL")]
        rpc_url: String,
        #[arg(long, default_value_t = 300_000.0)]
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
        /// How many resolved pools to probe on-chain.
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Recent block window to sample swaps over.
        #[arg(long, default_value_t = 1_000)]
        lookback_blocks: u64,
        /// eth_getLogs chunk size (free Alchemy caps at 10).
        #[arg(long, default_value_t = 10)]
        log_chunk_blocks: u64,
        #[arg(long, default_value_t = 120)]
        sleep_ms: u64,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    /// Moving-block bootstrap of a collected swap stream: run the policy battery over
    /// many resampled paths and report each policy's net-PnL distribution. Shows that
    /// a delta-hedged LP harvests fee−LVR with low variance while unhedged swings
    /// with whichever direction each path took.
    MultiPath {
        #[arg(long, default_value = "data/base/aerodrome-trend")]
        data_dir: PathBuf,
        #[arg(long)]
        pool_address: Option<String>,
        #[arg(long)]
        symbol: Option<String>,
        /// Number of bootstrapped paths.
        #[arg(long, default_value_t = 200)]
        paths: usize,
        /// Moving-block length, in swaps (preserves local microstructure).
        #[arg(long, default_value_t = 100)]
        block_len: usize,
        #[arg(long, default_value_t = 42)]
        seed: u64,
        /// Remove the source's mean drift so paths are driftless (martingale) — the
        /// regime under which LP net ≈ fee − LVR in expectation; isolates LP economics
        /// from the directional bet inherited from a one-way source window.
        #[arg(long, default_value_t = false)]
        demean: bool,
        #[command(flatten)]
        params: ReplayParams,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    /// Build a DRY-RUN Aerodrome Slipstream (Uniswap-v3 model) rebalance plan for a
    /// pool: reads pool state, proposes the collect→burn→swap→mint(+stake) action
    /// sequence with slippage-protected min amounts, estimates gas, runs hard risk
    /// gates, and NEVER signs or broadcasts. v2 (Aerodrome V1) and v4 need separate
    /// adapters — this planner is Slipstream/v3 only.
    DryRunRebalance {
        #[arg(long, env = "BASE_RPC_URL")]
        rpc_url: String,
        #[arg(long)]
        pool_address: String,
        /// USD capital to deploy (or current position value when rebalancing).
        #[arg(long, default_value_t = 10_000.0)]
        capital_usd: f64,
        /// USD value of one token0 (numeraire).
        #[arg(long, default_value_t = 1.0)]
        token0_usd: f64,
        #[arg(long, default_value_t = 18)]
        decimals0: u8,
        #[arg(long, default_value_t = 18)]
        decimals1: u8,
        /// Target half-width in ticks (band = current_tick ± this, snapped to spacing).
        #[arg(long, default_value_t = 600)]
        half_width_ticks: i32,
        /// Existing position to rebalance from (omit for a fresh mint).
        #[arg(long)]
        current_lower: Option<i32>,
        #[arg(long)]
        current_upper: Option<i32>,
        /// Max slippage on the rebalance swap and mint, in bps.
        #[arg(long, default_value_t = 30.0)]
        slippage_bps: f64,
        /// Gas units for the full rebalance multicall.
        #[arg(long, default_value_t = 900_000)]
        rebalance_gas_units: u64,
        #[arg(long, default_value_t = 3000.0)]
        eth_usd: f64,
        /// Expected fee edge (USD) the rebalance is meant to capture; the gas/edge
        /// gate rejects if gas exceeds `max_gas_to_edge_pct` of it.
        #[arg(long, default_value_t = 0.0)]
        expected_edge_usd: f64,
        #[arg(long, default_value_t = 0.15)]
        max_gas_to_edge_pct: f64,
        /// Position is staked in the gauge (adds unstake/stake steps + gas).
        #[arg(long, default_value_t = true)]
        staked: bool,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
}

/// Shared economic + execution parameters for the replay commands.
#[derive(Debug, clap::Args)]
struct ReplayParams {
    /// Pool fee tier in basis points (30 bps = 0.30%).
    #[arg(long, default_value_t = 30.0)]
    fee_bps: f64,
    /// Decimals of token0 (lower-address token). WETH=18.
    #[arg(long, default_value_t = 18)]
    decimals0: u8,
    /// Decimals of token1 (higher-address token). AERO=18, USDC=6.
    #[arg(long, default_value_t = 18)]
    decimals1: u8,
    /// USD value of one token0 (numeraire anchor). WETH ~ 1574 here.
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
    /// Blocks between a trigger and execution (you cannot rebalance instantly).
    #[arg(long, default_value_t = 0)]
    action_delay_blocks: u64,
    /// Funding cost (bps/day) on the short-hedge notional.
    #[arg(long, default_value_t = 0.0)]
    funding_bps_per_day: f64,
    /// Short-hedge size as a fraction of entry risk-asset exposure.
    #[arg(long, default_value_t = 1.0)]
    hedge_fraction: f64,
    /// Trend strength above which the adaptive policy exits (≈ sigmas of drift).
    /// 6.0 is calibrated on real noisy tick data; ~2.0 over-triggers on real flow.
    #[arg(long, default_value_t = 6.0)]
    trend_exit_threshold: f64,
    /// Annual reward (gauge emission) APR earned while in range, as a fraction
    /// (e.g. 0.2249 for WETH-AERO). 0 disables reward income.
    #[arg(long, default_value_t = 0.0)]
    reward_apr: f64,
    /// Haircut on reward income for liquidation cost (0.1 keeps 90%).
    #[arg(long, default_value_t = 0.0)]
    reward_haircut: f64,
    /// Invert token0/token1 so the stable/numeraire leg (if it is token1, e.g.
    /// USDC in CTR-USDC) becomes token0. Swap --decimals0/--decimals1 accordingly.
    #[arg(long, default_value_t = false)]
    invert: bool,
}

impl ReplayParams {
    /// Load swaps for a pool, applying token0/token1 inversion if requested.
    fn load_swaps(
        &self,
        target: &std::path::Path,
    ) -> Result<(String, Vec<autopool_backtest::SwapObs>)> {
        let (symbol, mut swaps) = load_swaps(target)?;
        if self.invert {
            for swap in &mut swaps {
                *swap = swap.inverted();
            }
        }
        Ok((symbol, swaps))
    }
}

impl ReplayParams {
    fn replay_config(&self) -> autopool_backtest::ReplayConfig {
        autopool_backtest::ReplayConfig {
            decimals0: self.decimals0,
            decimals1: self.decimals1,
            fee_fraction: self.fee_bps / 10_000.0,
            token0_usd: self.token0_usd,
            capital_usd: self.capital_usd,
            rebalance_gas_usd: self.rebalance_gas_usd,
            rebalance_slippage_bps: self.rebalance_slippage_bps,
            rebalance_swap_fraction: 0.5,
            reward_apr: self.reward_apr,
            reward_haircut: self.reward_haircut,
        }
    }

    fn exec_config(&self) -> autopool_backtest::ExecConfig {
        autopool_backtest::ExecConfig {
            action_delay_blocks: self.action_delay_blocks,
            block_seconds: 2.0,
            funding_bps_per_day: self.funding_bps_per_day,
            risk_asset_is_token1: true,
        }
    }
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
            from_block,
            to_block,
            swaps_only,
            min_tvl_usd,
            min_volume_usd_1d,
            max_reward_share,
            min_apy,
            min_fee_bps,
            profile,
            include_symbols,
            pools,
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
                from_block,
                to_block,
                swaps_only,
                min_tvl_usd,
                min_volume_usd_1d,
                max_reward_share,
                min_apy,
                min_fee_bps,
                profile,
                include_symbols,
                pools,
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
            params,
            format,
        } => replay_events(data_dir, pool_address, symbol, &params, format)?,
        Command::ReplayScenario {
            scenario,
            start_tick,
            swaps,
            move_ticks,
            swap_size_token0,
            liquidity,
            params,
            format,
        } => replay_scenario(
            scenario,
            start_tick,
            swaps,
            move_ticks,
            swap_size_token0,
            liquidity,
            &params,
            format,
        )?,
        Command::WalkForward {
            data_dir,
            pool_address,
            symbol,
            train_swaps,
            test_swaps,
            thresholds,
            half_widths,
            drawdown_penalty,
            params,
            format,
        } => walk_forward_cmd(WalkForwardArgs {
            data_dir,
            pool_address,
            symbol,
            train_swaps,
            test_swaps,
            thresholds,
            half_widths,
            drawdown_penalty,
            params,
            format,
        })?,
        Command::ScanPoolActivity {
            rpc_url,
            min_tvl_usd,
            min_volume_usd_1d,
            max_reward_share,
            min_apy,
            min_fee_bps,
            profile,
            include_symbols,
            limit,
            lookback_blocks,
            log_chunk_blocks,
            sleep_ms,
            format,
        } => {
            scan_pool_activity(ScanActivityArgs {
                rpc_url,
                min_tvl_usd,
                min_volume_usd_1d,
                max_reward_share,
                min_apy,
                min_fee_bps,
                profile,
                include_symbols,
                limit,
                lookback_blocks,
                log_chunk_blocks,
                sleep_ms,
                format,
            })
            .await?
        }
        Command::MultiPath {
            data_dir,
            pool_address,
            symbol,
            paths,
            block_len,
            seed,
            demean,
            params,
            format,
        } => multi_path_cmd(
            data_dir,
            pool_address,
            symbol,
            paths,
            block_len,
            seed,
            demean,
            &params,
            format,
        )?,
        Command::DryRunRebalance {
            rpc_url,
            pool_address,
            capital_usd,
            token0_usd,
            decimals0,
            decimals1,
            half_width_ticks,
            current_lower,
            current_upper,
            slippage_bps,
            rebalance_gas_units,
            eth_usd,
            expected_edge_usd,
            max_gas_to_edge_pct,
            staked,
            format,
        } => {
            dry_run_rebalance(DryRunArgs {
                rpc_url,
                pool_address,
                capital_usd,
                token0_usd,
                decimals0,
                decimals1,
                half_width_ticks,
                current_lower,
                current_upper,
                slippage_bps,
                rebalance_gas_units,
                eth_usd,
                expected_edge_usd,
                max_gas_to_edge_pct,
                staked,
                format,
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
    from_block: Option<u64>,
    to_block: Option<u64>,
    swaps_only: bool,
    min_tvl_usd: f64,
    min_volume_usd_1d: f64,
    max_reward_share: f64,
    min_apy: f64,
    min_fee_bps: f64,
    profile: CandidateProfile,
    include_symbols: Vec<String>,
    pools: Vec<String>,
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
    let resolved = if config.pools.is_empty() {
        resolve_pilot_candidates(
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
        .await?
    } else {
        config
            .pools
            .iter()
            .map(|spec| parse_manual_pool(spec))
            .collect::<Result<Vec<_>>>()?
    };

    if resolved.is_empty() {
        anyhow::bail!("no pools resolved to backfill");
    }
    for item in &resolved {
        eprintln!(
            "backfilling {} {} ({:?})",
            item.candidate.symbol, item.pool_address, item.deployment
        );
    }

    fs::create_dir_all(config.data_dir.join("events"))?;
    fs::create_dir_all(config.data_dir.join("checkpoints"))?;

    let historical = config.to_block.is_some() || config.from_block.is_some();
    let mut iteration = 0_u64;
    loop {
        iteration += 1;
        let latest = rpc.latest_block_number().await?;
        // Upper bound to collect to: a fixed window end, or the chain head.
        let target_end = config.to_block.unwrap_or(latest).min(latest);
        let mut total_written = 0_usize;
        let mut all_done = true;

        for item in &resolved {
            let checkpoint_path = checkpoint_path(&config.data_dir, &item.pool_address);
            let default_start = config
                .from_block
                .unwrap_or_else(|| latest.saturating_sub(config.lookback_blocks.saturating_sub(1)));
            let start = read_next_block(&checkpoint_path)?.unwrap_or(default_start);

            if start > target_end {
                continue;
            }
            all_done = false;

            let to_block = start
                .saturating_add(config.max_blocks_per_run.saturating_sub(1))
                .min(target_end);
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

        eprintln!(
            "iteration={iteration} latest={latest} target_end={target_end} total_events_written={total_written}"
        );

        // A bounded historical window finishes once every pool has passed its end.
        if historical && all_done {
            eprintln!("historical window complete up to {target_end}");
            break;
        }
        if config.iterations != 0 && iteration >= config.iterations {
            break;
        }

        tokio::time::sleep(Duration::from_secs(config.poll_seconds)).await;
    }

    Ok(())
}

/// Parse a manual pool spec `SYMBOL:0xADDRESS` into a resolved candidate.
fn parse_manual_pool(spec: &str) -> Result<ResolvedCandidate> {
    let (symbol, address) = spec
        .split_once(':')
        .with_context(|| format!("expected SYMBOL:0xADDRESS, got `{spec}`"))?;
    let address = address.trim();
    let stripped = address.strip_prefix("0x").unwrap_or(address);
    if stripped.len() != 40 || !stripped.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("invalid pool address in `{spec}`");
    }
    Ok(ResolvedCandidate {
        candidate: SlipstreamCandidate::manual(symbol.trim()),
        deployment: autopool_aerodrome::SlipstreamDeployment::Manual,
        pool_address: address.to_ascii_lowercase(),
    })
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
            // Pace factory probes so the burst does not trip public-RPC rate limits.
            tokio::time::sleep(Duration::from_millis(60)).await;
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
    let topics: &[EventTopic] = if config.swaps_only {
        &EVENT_TOPICS[..1]
    } else {
        &EVENT_TOPICS[..]
    };

    while cursor <= to_block {
        let chunk_end = cursor
            .saturating_add(config.log_chunk_blocks.saturating_sub(1))
            .min(to_block);

        for event_topic in topics {
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

#[derive(Debug, Serialize)]
struct ReplayReport {
    label: String,
    swaps: usize,
    block_first: Option<u64>,
    block_last: Option<u64>,
    tick_first: Option<i32>,
    tick_last: Option<i32>,
    config: serde_json::Value,
    policies: Vec<autopool_backtest::PolicyReport>,
}

fn params_json(params: &ReplayParams) -> serde_json::Value {
    json!({
        "fee_bps": params.fee_bps,
        "decimals0": params.decimals0,
        "decimals1": params.decimals1,
        "token0_usd": params.token0_usd,
        "capital_usd": params.capital_usd,
        "rebalance_gas_usd": params.rebalance_gas_usd,
        "rebalance_slippage_bps": params.rebalance_slippage_bps,
        "narrow_half_width": params.narrow_half_width,
        "wide_half_width": params.wide_half_width,
        "vol_k": params.vol_k,
        "action_delay_blocks": params.action_delay_blocks,
        "funding_bps_per_day": params.funding_bps_per_day,
        "hedge_fraction": params.hedge_fraction,
        "trend_exit_threshold": params.trend_exit_threshold,
        "reward_apr": params.reward_apr,
        "reward_haircut": params.reward_haircut,
        "invert": params.invert,
    })
}

fn run_battery(
    params: &ReplayParams,
    swaps: &[autopool_backtest::SwapObs],
) -> Vec<autopool_backtest::PolicyReport> {
    autopool_backtest::run_baseline_battery_with(
        swaps,
        &params.replay_config(),
        &params.exec_config(),
        params.narrow_half_width,
        params.wide_half_width,
        params.vol_k,
        params.hedge_fraction,
        params.trend_exit_threshold,
    )
}

fn emit_replay_report(
    report: ReplayReport,
    params: &ReplayParams,
    format: OutputFormat,
) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!(
        "replay {} swaps={} blocks={}..{} ticks={}..{}",
        report.label,
        report.swaps,
        optional_u64(report.block_first),
        optional_u64(report.block_last),
        optional_i32(report.tick_first),
        optional_i32(report.tick_last),
    );
    println!(
        "config: fee={:.2}bps capital=${:.0} token0=${:.0} gas=${:.3}/reb delay={}blk fund={}bps/d hedge={} narrow=±{} wide=±{}",
        params.fee_bps,
        params.capital_usd,
        params.token0_usd,
        params.rebalance_gas_usd,
        params.action_delay_blocks,
        params.funding_bps_per_day,
        params.hedge_fraction,
        params.narrow_half_width,
        params.wide_half_width,
    );
    println!(
        "{:<22} {:>10} {:>10} {:>9} {:>9} {:>9} {:>10} {:>9} {:>8}",
        "policy", "net_pnl", "vs_hold", "fees", "reward", "LVR", "fee-LVR", "maxDD", "rebals"
    );
    for policy in &report.policies {
        println!(
            "{:<22} {:>10.2} {:>10.2} {:>9.2} {:>9.2} {:>9.2} {:>10.2} {:>9.2} {:>8}",
            policy.policy,
            policy.net_pnl_usd,
            policy.net_vs_hold_usd,
            policy.fee_income_usd,
            policy.reward_income_usd,
            policy.lvr_usd,
            policy.fee_minus_lvr_usd,
            policy.max_drawdown_usd,
            policy.rebalances,
        );
    }
    Ok(())
}

fn replay_events(
    data_dir: PathBuf,
    pool_address: Option<String>,
    symbol: Option<String>,
    params: &ReplayParams,
    format: OutputFormat,
) -> Result<()> {
    let events_dir = data_dir.join("events");
    if !events_dir.exists() {
        anyhow::bail!("event directory does not exist: {}", events_dir.display());
    }

    let target = select_replay_target(&events_dir, pool_address.as_deref(), symbol.as_deref())?;
    let (symbol, swaps) = params.load_swaps(&target)?;
    if swaps.is_empty() {
        anyhow::bail!("no decodable swap events found in {}", target.display());
    }

    let pool_address = target
        .parent()
        .and_then(|parent| parent.file_name())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "-".to_string());

    let policies = run_battery(params, &swaps);
    let report = ReplayReport {
        label: format!("{symbol} ({pool_address})"),
        swaps: swaps.len(),
        block_first: swaps.first().map(|swap| swap.block),
        block_last: swaps.last().map(|swap| swap.block),
        tick_first: swaps.first().map(|swap| swap.tick),
        tick_last: swaps.last().map(|swap| swap.tick),
        config: params_json(params),
        policies,
    };
    emit_replay_report(report, params, format)
}

#[allow(clippy::too_many_arguments)]
fn replay_scenario(
    scenario: String,
    start_tick: i32,
    swaps: usize,
    move_ticks: i32,
    swap_size_token0: f64,
    liquidity: f64,
    params: &ReplayParams,
    format: OutputFormat,
) -> Result<()> {
    let kind = autopool_backtest::Scenario::parse(&scenario)
        .with_context(|| format!("unknown scenario `{scenario}` (calm|pump|crash|chop)"))?;
    let swap_size_raw = swap_size_token0 * 10f64.powi(params.decimals0 as i32);
    let stream = autopool_backtest::scenario_swaps(
        kind,
        start_tick,
        swaps,
        move_ticks,
        swap_size_raw,
        liquidity,
    );
    if stream.is_empty() {
        anyhow::bail!("scenario produced no swaps");
    }

    let policies = run_battery(params, &stream);
    let report = ReplayReport {
        label: format!("scenario:{scenario} move={move_ticks}t"),
        swaps: stream.len(),
        block_first: stream.first().map(|swap| swap.block),
        block_last: stream.last().map(|swap| swap.block),
        tick_first: stream.first().map(|swap| swap.tick),
        tick_last: stream.last().map(|swap| swap.tick),
        config: params_json(params),
        policies,
    };
    emit_replay_report(report, params, format)
}

struct WalkForwardArgs {
    data_dir: PathBuf,
    pool_address: Option<String>,
    symbol: Option<String>,
    train_swaps: usize,
    test_swaps: usize,
    thresholds: Vec<f64>,
    half_widths: Vec<i32>,
    drawdown_penalty: f64,
    params: ReplayParams,
    format: OutputFormat,
}

fn walk_forward_cmd(args: WalkForwardArgs) -> Result<()> {
    let events_dir = args.data_dir.join("events");
    if !events_dir.exists() {
        anyhow::bail!("event directory does not exist: {}", events_dir.display());
    }
    let target = select_replay_target(
        &events_dir,
        args.pool_address.as_deref(),
        args.symbol.as_deref(),
    )?;
    let (symbol, swaps) = args.params.load_swaps(&target)?;
    if swaps.is_empty() {
        anyhow::bail!("no decodable swap events found in {}", target.display());
    }

    let wf = autopool_backtest::WalkForwardConfig {
        train_swaps: args.train_swaps,
        test_swaps: args.test_swaps,
        thresholds: args.thresholds.clone(),
        half_widths: args.half_widths.clone(),
        vol_k: args.params.vol_k,
        cap_ticks: args.params.wide_half_width,
        window: 200,
        drawdown_penalty: args.drawdown_penalty,
    };
    let report = autopool_backtest::walk_forward(
        &swaps,
        &args.params.replay_config(),
        &args.params.exec_config(),
        &wf,
    );

    if args.format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!(
        "walk-forward {symbol} ({}) swaps={} folds={} train={} test={} grid: thr={:?} hw={:?} penalty={}",
        target
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        swaps.len(),
        report.folds.len(),
        args.train_swaps,
        args.test_swaps,
        args.thresholds,
        args.half_widths,
        args.drawdown_penalty,
    );
    println!(
        "{:>4} {:>10} {:>9} {:>7} {:>10} {:>9} {:>7} {:>10}",
        "fold", "test_blk", "chosen_T", "hw", "test_net", "test_DD", "rebals", "tick_span"
    );
    for fold in &report.folds {
        println!(
            "{:>4} {:>10} {:>9.1} {:>7} {:>10.2} {:>9.2} {:>7} {:>10}",
            fold.fold,
            format!("{}..{}", fold.train_end, fold.test_end),
            fold.chosen_threshold,
            fold.chosen_half_width,
            fold.test_net_usd,
            fold.test_max_drawdown_usd,
            fold.test_rebalances,
            fold.test_tick_span,
        );
    }
    println!(
        "OOS net (walk-forward adaptive): {:>10.2}   maxDD: {:.2}   (test swaps {})",
        report.oos_net_usd, report.oos_max_drawdown_usd, report.test_swaps_total
    );
    println!(
        "OOS net baselines  fixed_adaptive: {:>10.2}   static: {:>10.2}   hold: {:>10.2}",
        report.fixed_adaptive_net_usd, report.static_net_usd, report.hold_net_usd
    );

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn multi_path_cmd(
    data_dir: PathBuf,
    pool_address: Option<String>,
    symbol: Option<String>,
    paths: usize,
    block_len: usize,
    seed: u64,
    demean: bool,
    params: &ReplayParams,
    format: OutputFormat,
) -> Result<()> {
    let events_dir = data_dir.join("events");
    if !events_dir.exists() {
        anyhow::bail!("event directory does not exist: {}", events_dir.display());
    }
    let target = select_replay_target(&events_dir, pool_address.as_deref(), symbol.as_deref())?;
    let (symbol, swaps) = params.load_swaps(&target)?;
    if swaps.len() < block_len.max(2) {
        anyhow::bail!("not enough swaps ({}) to bootstrap", swaps.len());
    }

    let report = autopool_backtest::multi_path_eval(
        &swaps,
        &params.replay_config(),
        &params.exec_config(),
        params.narrow_half_width,
        params.wide_half_width,
        params.vol_k,
        params.hedge_fraction,
        params.trend_exit_threshold,
        paths,
        block_len,
        seed,
        demean,
    );

    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!(
        "multi-path {symbol} swaps/path={} paths={} block_len={} (bootstrap of {} real swaps)",
        report.swaps_per_path,
        report.n_paths,
        report.block_len,
        swaps.len()
    );
    println!(
        "{:<22} {:>10} {:>10} {:>10} {:>10} {:>10} {:>9} {:>10}",
        "policy", "mean_net", "std_net", "p05", "p95", "fee-LVR", "win%hold", "meanDD"
    );
    for p in &report.policies {
        println!(
            "{:<22} {:>10.2} {:>10.2} {:>10.2} {:>10.2} {:>10.2} {:>8.0}% {:>10.2}",
            p.policy,
            p.mean_net_usd,
            p.std_net_usd,
            p.p05_net_usd,
            p.p95_net_usd,
            p.mean_fee_minus_lvr_usd,
            p.win_rate_vs_hold * 100.0,
            p.mean_max_drawdown_usd,
        );
    }
    Ok(())
}

struct DryRunArgs {
    rpc_url: String,
    pool_address: String,
    capital_usd: f64,
    token0_usd: f64,
    decimals0: u8,
    decimals1: u8,
    half_width_ticks: i32,
    current_lower: Option<i32>,
    current_upper: Option<i32>,
    slippage_bps: f64,
    rebalance_gas_units: u64,
    eth_usd: f64,
    expected_edge_usd: f64,
    max_gas_to_edge_pct: f64,
    staked: bool,
    format: OutputFormat,
}

fn hex_word_to_f64(hex: &str) -> f64 {
    let stripped = hex.trim_start_matches("0x");
    let mut acc = 0.0_f64;
    for ch in stripped.chars() {
        acc = acc * 16.0 + ch.to_digit(16).unwrap_or(0) as f64;
    }
    acc
}

fn snap_to_spacing(tick: i32, spacing: i32) -> i32 {
    let s = spacing.max(1);
    ((tick as f64 / s as f64).round() as i32) * s
}

/// Build a dry-run Slipstream (v3) rebalance plan. Reads pool state, computes the
/// action sequence + slippage-protected amounts + gas, runs risk gates, prints, and
/// never signs. Aerodrome V1 (v2) and Uniswap v4 are NOT handled here.
async fn dry_run_rebalance(args: DryRunArgs) -> Result<()> {
    let rpc = JsonRpcClient::new(args.rpc_url.clone());
    let state = rpc.read_cl_pool_state(&args.pool_address).await?;
    let fee_bps = match rpc.eth_call(&args.pool_address, "0xddca3f43").await {
        Ok(hex) => parse_hex_u64_lossy(&hex)
            .map(|v| v as f64 / 100.0)
            .unwrap_or(0.0),
        Err(_) => 0.0,
    };
    let net = rpc
        .sample_network(BASE_CHAIN_ID, args.rebalance_gas_units, Some(args.eth_usd))
        .await?;

    let sqrt_x96 = hex_word_to_f64(&state.sqrt_price_x96_hex);
    let spacing = state.tick_spacing.max(1);
    let lower = snap_to_spacing(state.current_tick - args.half_width_ticks, spacing);
    let upper = snap_to_spacing(state.current_tick + args.half_width_ticks, spacing);
    let capital_token0 = args.capital_usd / args.token0_usd;

    // Target inventory for the new band (v3 math, agrees with the backtest).
    let (t0, t1, target_liq) = autopool_backtest::cl_mint_amounts(
        args.decimals0,
        args.decimals1,
        lower,
        upper,
        sqrt_x96,
        capital_token0,
    );
    // Current inventory: from the old band if rebalancing, else all token0.
    let (c0, c1) = match (args.current_lower, args.current_upper) {
        (Some(cl), Some(cu)) => {
            let (a0, a1, _) = autopool_backtest::cl_mint_amounts(
                args.decimals0,
                args.decimals1,
                cl,
                cu,
                sqrt_x96,
                capital_token0,
            );
            (a0, a1)
        }
        _ => (capital_token0, 0.0),
    };

    let price_human = {
        let p_raw = (sqrt_x96 / 79_228_162_514_264_337_593_543_950_336.0).powi(2);
        p_raw * 10f64.powi(args.decimals0 as i32 - args.decimals1 as i32)
    };
    let slip = args.slippage_bps / 10_000.0;

    // Slipstream/v3 deployment contracts (verify the pool's deployment matches).
    let c = BASE_SLIPSTREAM_GAUGES_V3;
    let rebalancing = args.current_lower.is_some() && args.current_upper.is_some();
    let mut actions: Vec<serde_json::Value> = Vec::new();

    if rebalancing && args.staked {
        actions.push(json!({"step":"unstake","contract":c.gauge_factory,"call":"gauge.withdraw(tokenId)","note":"emissions require staking; must unstake before modifying"}));
    }
    if rebalancing {
        actions.push(json!({"step":"collect","contract":c.nonfungible_position_manager,"call":"collect(tokenId, max, max)","expects":"uncollected fees out"}));
        actions.push(json!({"step":"decreaseLiquidity","contract":c.nonfungible_position_manager,"call":"decreaseLiquidity(tokenId, liquidity, amount0Min, amount1Min, deadline)","amount0_out_est":c0,"amount1_out_est":c1,"amount0Min":c0*(1.0-slip),"amount1Min":c1*(1.0-slip)}));
        actions.push(json!({"step":"burn","contract":c.nonfungible_position_manager,"call":"burn(tokenId)"}));
    }

    // Swap to reach the target ratio.
    let d0 = t0 - c0;
    let d1 = t1 - c1;
    if d0 < -1e-12 {
        // excess token0 -> sell for token1
        let amount_in = -d0;
        let expected_out = amount_in * price_human;
        actions.push(json!({"step":"swap","contract":c.swap_router,"call":"exactInputSingle(token0->token1)","amount_in":amount_in,"expected_out":expected_out,"amountOutMin":expected_out*(1.0-slip)}));
    } else if d1 < -1e-12 {
        let amount_in = -d1;
        let expected_out = if price_human > 0.0 { amount_in / price_human } else { 0.0 };
        actions.push(json!({"step":"swap","contract":c.swap_router,"call":"exactInputSingle(token1->token0)","amount_in":amount_in,"expected_out":expected_out,"amountOutMin":expected_out*(1.0-slip)}));
    }

    actions.push(json!({"step":"mint","contract":c.nonfungible_position_manager,"call":"mint(token0,token1,tickSpacing,tickLower,tickUpper,amount0Desired,amount1Desired,amount0Min,amount1Min,recipient,deadline)","tickLower":lower,"tickUpper":upper,"amount0Desired":t0,"amount1Desired":t1,"amount0Min":t0*(1.0-slip),"amount1Min":t1*(1.0-slip)}));
    if args.staked {
        actions.push(json!({"step":"stake","contract":c.gauge_factory,"call":"gauge.deposit(tokenId)","note":"stake the new NFT to earn emissions"}));
    }

    // Risk gates.
    let gas_usd = net.estimated_rebalance_gas_usd.unwrap_or(0.0);
    let risk_token_share = {
        let t1_in_t0 = if price_human > 0.0 { t1 / price_human } else { 0.0 };
        let total = t0 + t1_in_t0;
        if total > 0.0 { t1_in_t0 / total } else { 0.0 }
    };
    let mut gates: Vec<(String, bool, String)> = Vec::new();
    let gas_gate = args.expected_edge_usd <= 0.0
        || gas_usd <= args.max_gas_to_edge_pct * args.expected_edge_usd;
    gates.push((
        "gas_vs_edge".into(),
        gas_gate,
        format!(
            "gas ${:.4} vs {:.0}% of edge ${:.2}",
            gas_usd,
            args.max_gas_to_edge_pct * 100.0,
            args.expected_edge_usd
        ),
    ));
    gates.push((
        "one_sided_inventory".into(),
        risk_token_share <= 0.8,
        format!("risk-token (token1) share {:.0}% <= 80%", risk_token_share * 100.0),
    ));
    gates.push((
        "slippage_bounded".into(),
        args.slippage_bps <= 100.0,
        format!("max slippage {:.0} bps", args.slippage_bps),
    ));
    let all_pass = gates.iter().all(|(_, ok, _)| *ok);

    let plan = json!({
        "DRY_RUN": true,
        "requires_signature": true,
        "broadcast": false,
        "protocol": "aerodrome-slipstream (uniswap-v3 concentrated-liquidity model)",
        "not_handled": ["aerodrome-v1 (v2 x*y=k)", "uniswap-v4 (singleton + hooks + flash accounting)"],
        "pool": args.pool_address,
        "deployment_contracts_assumed": format!("{:?}", c.deployment),
        "fee_bps": fee_bps,
        "current_tick": state.current_tick,
        "tick_spacing": spacing,
        "gas_price_gwei": net.gas_price_gwei,
        "est_gas_usd": gas_usd,
        "target_range": {"lower": lower, "upper": upper, "liquidity_est": target_liq},
        "actions": actions,
        "gates": gates.iter().map(|(n,ok,d)| json!({"gate":n,"pass":ok,"detail":d})).collect::<Vec<_>>(),
        "all_gates_pass": all_pass,
    });

    if args.format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
        return Ok(());
    }

    println!("=== DRY RUN (Slipstream / Uniswap-v3) — NOT SIGNED, NOT BROADCAST ===");
    println!(
        "pool {} fee={:.2}bps tick={} spacing={} gas≈${:.4} (@{:.4} gwei)",
        args.pool_address, fee_bps, state.current_tick, spacing, gas_usd, net.gas_price_gwei
    );
    println!(
        "target band: [{lower}, {upper}]  desired token0={t0:.6} token1={t1:.6}  ({})",
        if rebalancing { "rebalance" } else { "fresh mint" }
    );
    println!("plan:");
    for (i, a) in actions.iter().enumerate() {
        println!(
            "  {}. {:<16} {}",
            i + 1,
            a.get("step").and_then(|v| v.as_str()).unwrap_or("-"),
            a.get("call").and_then(|v| v.as_str()).unwrap_or("-")
        );
    }
    println!("risk gates:");
    for (n, ok, d) in &gates {
        println!("  [{}] {:<22} {}", if *ok { "PASS" } else { "FAIL" }, n, d);
    }
    println!(
        "decision: {} (this is a proposal only; full ABI calldata + on-chain eth_call simulation from a funded sender is the next step)",
        if all_pass { "GATES PASS — would propose" } else { "REJECTED by risk gates" }
    );

    Ok(())
}

struct ScanActivityArgs {
    rpc_url: String,
    min_tvl_usd: f64,
    min_volume_usd_1d: f64,
    max_reward_share: f64,
    min_apy: f64,
    min_fee_bps: f64,
    profile: CandidateProfile,
    include_symbols: Vec<String>,
    limit: usize,
    lookback_blocks: u64,
    log_chunk_blocks: u64,
    sleep_ms: u64,
    format: OutputFormat,
}

#[derive(Debug, Serialize)]
struct PoolActivity {
    symbol: String,
    pool_address: String,
    deployment: String,
    fee_bps: f64,
    tvl_usd: f64,
    volume_usd_1d: f64,
    current_tick: i32,
    liquidity: String,
    swaps: usize,
    swaps_per_kblock: f64,
    tick_span: i32,
    /// Stdev of consecutive swap-to-swap tick changes — realized price volatility.
    tick_vol: f64,
    /// Composite: realized vol scaled by activity. Higher = more for an active LP to do.
    activity_score: f64,
    /// fee_bps / tick_vol — proxy for per-swap fee-vs-LVR. Higher ⇒ fee-alpha more
    /// likely positive (CTR-USDC ~22 had measured fee−LVR > 0; USDC-AERO ~2 had < 0).
    fee_vol_ratio: f64,
}

async fn scan_pool_activity(args: ScanActivityArgs) -> Result<()> {
    let resolved = resolve_pilot_candidates(
        &args.rpc_url,
        args.min_tvl_usd,
        args.min_volume_usd_1d,
        args.max_reward_share,
        args.min_apy,
        args.min_fee_bps,
        args.profile,
        args.include_symbols.clone(),
        args.limit,
    )
    .await?;
    if resolved.is_empty() {
        anyhow::bail!("no candidate pools resolved");
    }

    let rpc = JsonRpcClient::new(args.rpc_url.clone());
    let to_block = rpc.latest_block_number().await?;
    let from_block = to_block.saturating_sub(args.lookback_blocks.saturating_sub(1));
    let mut rows = Vec::new();

    for item in &resolved {
        // Fetch swaps one chunk at a time with a sleep between chunks so the scan
        // coexists with the background indexers on a shared free RPC endpoint.
        let mut ticks: Vec<i32> = Vec::new();
        let mut swap_count = 0usize;
        let mut cursor = from_block;
        while cursor <= to_block {
            let end = cursor
                .saturating_add(args.log_chunk_blocks.saturating_sub(1))
                .min(to_block);
            let logs = rpc
                .get_logs(&item.pool_address, cursor, end, SWAP_TOPIC)
                .await?;
            swap_count += logs.len();
            ticks.extend(
                logs.iter()
                    .filter_map(|log| autopool_backtest::decode_swap_obs(&log.data, 0, 0))
                    .map(|o| o.tick),
            );
            if end == u64::MAX {
                break;
            }
            cursor = end + 1;
            if args.sleep_ms > 0 {
                tokio::time::sleep(Duration::from_millis(args.sleep_ms)).await;
            }
        }
        let (tick_span, tick_vol) = tick_stats(&ticks);

        let state = rpc.read_cl_pool_state(&item.pool_address).await?;
        let fee_bps = match rpc.eth_call(&item.pool_address, "0xddca3f43").await {
            Ok(hex) => parse_hex_u64_lossy(&hex)
                .map(|v| v as f64 / 100.0)
                .unwrap_or(0.0),
            Err(_) => 0.0,
        };
        let span = to_block.saturating_sub(from_block).saturating_add(1);
        let swaps_per_kblock = if span > 0 {
            swap_count as f64 * 1000.0 / span as f64
        } else {
            0.0
        };
        let activity_score = tick_vol * swaps_per_kblock.sqrt();
        let fee_vol_ratio = if tick_vol > 0.1 {
            fee_bps / tick_vol
        } else {
            0.0
        };

        rows.push(PoolActivity {
            symbol: item.candidate.symbol.clone(),
            pool_address: item.pool_address.clone(),
            deployment: format!("{:?}", item.deployment),
            fee_bps,
            tvl_usd: item.candidate.tvl_usd,
            volume_usd_1d: item.candidate.volume_usd_1d,
            current_tick: state.current_tick,
            liquidity: state.liquidity,
            swaps: swap_count,
            swaps_per_kblock: (swaps_per_kblock * 100.0).round() / 100.0,
            tick_span,
            tick_vol: (tick_vol * 100.0).round() / 100.0,
            activity_score: (activity_score * 100.0).round() / 100.0,
            fee_vol_ratio: (fee_vol_ratio * 100.0).round() / 100.0,
        });
    }

    // Rank by fee-alpha potential (fee/vol) among pools that actually trade.
    rows.sort_by(|a, b| {
        let key = |r: &PoolActivity| if r.swaps >= 5 { r.fee_vol_ratio } else { -1.0 };
        key(b).total_cmp(&key(a))
    });

    if args.format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    println!(
        "pool activity scan  window={from_block}..{to_block} ({} blocks)",
        args.lookback_blocks
    );
    println!(
        "{:<16} {:>8} {:>11} {:>7} {:>9} {:>9} {:>10} {:>9}",
        "symbol", "fee_bps", "tvl_usd", "swaps", "swp/kblk", "tick_vol", "fee/vol", "score"
    );
    for row in &rows {
        println!(
            "{:<16} {:>8.2} {:>11.0} {:>7} {:>9.1} {:>9.2} {:>10.2} {:>9.2}",
            row.symbol,
            row.fee_bps,
            row.tvl_usd,
            row.swaps,
            row.swaps_per_kblock,
            row.tick_vol,
            row.fee_vol_ratio,
            row.activity_score,
        );
    }
    println!(
        "(ranked by fee/vol = fee_bps/tick_vol, a fee-alpha proxy; CTR-USDC ~22 had measured fee−LVR > 0)"
    );

    Ok(())
}

/// Tick span (max-min) and realized volatility (stdev of consecutive tick changes).
fn tick_stats(ticks: &[i32]) -> (i32, f64) {
    if ticks.len() < 2 {
        return (0, 0.0);
    }
    let max = *ticks.iter().max().unwrap();
    let min = *ticks.iter().min().unwrap();
    let diffs: Vec<f64> = ticks.windows(2).map(|w| (w[1] - w[0]) as f64).collect();
    let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
    let var = diffs.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / diffs.len() as f64;
    (max - min, var.sqrt())
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
