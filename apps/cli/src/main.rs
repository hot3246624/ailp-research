#![recursion_limit = "256"]

use anyhow::{Context, Result};
use autopool_aerodrome::{
    BASE_CHAIN_ID, BASE_SLIPSTREAM_GAUGES_V3, PilotProfile, SlipstreamCandidate,
    base_slipstream_factories_latest_first, build_pilot_universe_for_profile,
};
use autopool_core::YieldSnapshot;
use autopool_defillama::{DefiLlamaClient, DefiLlamaPool, PoolFilter};
use autopool_evm::{BURN_TOPIC, COLLECT_TOPIC, JsonRpcClient, MINT_TOPIC, SWAP_TOPIC};
use autopool_solana::{
    DiscoveryOptions, SolanaDiscoveryClient, SolanaPoolCandidate, SolanaToken, SolanaVenue,
};
use autopool_strategy::WeightedRiskModel;
use base64::Engine;
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SOLANA_HTTP_TIMEOUT_SECS: u64 = 20;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum SolanaDiscoverVenue {
    Orca,
    Raydium,
    Meteora,
}

impl From<SolanaDiscoverVenue> for SolanaVenue {
    fn from(value: SolanaDiscoverVenue) -> Self {
        match value {
            SolanaDiscoverVenue::Orca => Self::OrcaWhirlpool,
            SolanaDiscoverVenue::Raydium => Self::RaydiumClmm,
            SolanaDiscoverVenue::Meteora => Self::MeteoraDlmm,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ValueEnum)]
enum RiskTokenSide {
    Token0,
    Token1,
}

impl RiskTokenSide {
    fn label(self) -> &'static str {
        match self {
            Self::Token0 => "token0",
            Self::Token1 => "token1",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ValueEnum)]
enum PromotionGatePolicy {
    LaggedRegimeRule,
    LaggedPolicySwitch,
    HedgedWide,
    DeltaHedged,
}

impl PromotionGatePolicy {
    fn label(self) -> &'static str {
        match self {
            Self::LaggedRegimeRule => "lagged_regime_rule",
            Self::LaggedPolicySwitch => "lagged_policy_switch",
            Self::HedgedWide => "hedged_wide",
            Self::DeltaHedged => "delta_hedged",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ValueEnum)]
enum RegimeSwitchPolicy {
    Hold,
    PassiveWide,
    NarrowRebalance,
    DeltaHedged,
    HedgedWide,
}

impl RegimeSwitchPolicy {
    fn label(self) -> &'static str {
        match self {
            Self::Hold => "hold_50_50",
            Self::PassiveWide => "passive_wide",
            Self::NarrowRebalance => "narrow_rebalance",
            Self::DeltaHedged => "delta_hedged",
            Self::HedgedWide => "hedged_wide",
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
    /// Rank Solana LP candidates from DeFiLlama. This is the first Solana research
    /// pass: organic base APY, TVL, volume/Tvl, reward share, and fee tier when
    /// available. It does not imply the pool is executable yet.
    SolanaUniverse {
        #[arg(long, default_value_t = 50_000.0)]
        min_tvl_usd: f64,
        #[arg(long, default_value_t = 25_000.0)]
        min_volume_usd_1d: f64,
        #[arg(long, default_value_t = 20.0)]
        min_base_apy: f64,
        #[arg(long, default_value_t = 0.25)]
        max_reward_share: f64,
        #[arg(long, default_value_t = 0.0)]
        min_fee_bps: f64,
        #[arg(long, default_value_t = false)]
        include_outliers: bool,
        #[arg(long, default_value_t = false)]
        concentrated_only: bool,
        #[arg(long, default_value_t = false)]
        blue_chip_only: bool,
        #[arg(long = "project", default_values_t = [
            "raydium-amm".to_string(),
            "orca-dex".to_string(),
            "kamino-liquidity".to_string()
        ])]
        projects: Vec<String>,
        #[arg(long, default_value_t = 25)]
        limit: usize,
        /// Optional JSON output path for snapshotting the ranked rows.
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    /// Discover executable Solana LP candidates from protocol-owned APIs.
    /// This is still read-only research: no wallet, signing, or transaction
    /// planning. It normalizes Orca Whirlpool, Raydium CLMM, and Meteora DLMM
    /// pool statistics into one candidate table.
    SolanaDiscover {
        #[arg(long = "venue", value_enum)]
        venues: Vec<SolanaDiscoverVenue>,
        #[arg(long, default_value_t = 100_000.0)]
        min_tvl_usd: f64,
        #[arg(long, default_value_t = 100_000.0)]
        min_volume_usd_24h: f64,
        #[arg(long, default_value_t = 20.0)]
        min_fee_apr: f64,
        #[arg(long, default_value_t = 1_000.0)]
        max_fee_apr: f64,
        #[arg(long, default_value_t = false)]
        verified_only: bool,
        #[arg(long, default_value_t = 100)]
        page_size: usize,
        #[arg(long, default_value_t = 25)]
        limit: usize,
        /// Optional JSON output path for snapshotting protocol API rows.
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    /// Rank hot-pool research candidates using the autoresearch protocol. This treats
    /// very high APR as a signal to validate, not a deployable return promise.
    HotPoolCandidates {
        #[arg(long = "venue", value_enum)]
        venues: Vec<SolanaDiscoverVenue>,
        #[arg(long, default_value_t = 50_000.0)]
        min_tvl_usd: f64,
        #[arg(long, default_value_t = 25_000.0)]
        min_volume_usd_24h: f64,
        #[arg(long, default_value_t = 100.0)]
        min_fee_apr: f64,
        #[arg(long, default_value_t = 10_000.0)]
        max_fee_apr: f64,
        /// Headline APR level treated as an alarm threshold and sanity benchmark.
        #[arg(long, default_value_t = 2_000.0)]
        target_fee_apr: f64,
        #[arg(long, default_value_t = 0.5)]
        min_volume_tvl_24h: f64,
        #[arg(long, default_value_t = 200)]
        page_size: usize,
        #[arg(long, default_value_t = 30)]
        limit: usize,
        /// Optional JSON output path for the ranked hot-pool research queue.
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    /// Convert the hot-pool candidate queue into concrete replay experiments.
    /// This does not invent results: pools without normalized swap/bin data stay
    /// blocked with explicit data requirements.
    HotPoolExperimentPlan {
        /// JSON emitted by `hot-pool-candidates`.
        #[arg(long, default_value = "data/hot-pool/candidates/latest.json")]
        input: PathBuf,
        /// Root where normalized Solana replay streams will live.
        #[arg(long, default_value = "data/solana/hot-pool")]
        data_dir: PathBuf,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Include P2/API-validation rows in the plan. By default only replayable
        /// or freeze-state candidates are planned.
        #[arg(long, default_value_t = false)]
        include_p2: bool,
        #[arg(long, default_value_t = 10_000.0)]
        capital_usd: f64,
        #[arg(long, default_value_t = 300)]
        narrow_half_width: i32,
        #[arg(long, default_value_t = 3_000)]
        wide_half_width: i32,
        #[arg(long, default_value_t = 0.05)]
        max_drawdown_pct: f64,
        #[arg(long, default_value_t = 12)]
        max_rebalances_per_day: u32,
        /// Optional JSON output path for the experiment manifest.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Also write per-pool replay spec sidecars under `data_dir/specs`.
        #[arg(long, default_value_t = false)]
        write_specs: bool,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    /// Run a Solana protocol-API proxy replay. This is the first end-to-end
    /// business-flow estimate: pool stats -> range width -> fee capture -> churn
    /// cost -> risk. It is not a substitute for tick-by-tick replay.
    SolanaProxyReplay {
        #[arg(long = "venue", value_enum)]
        venues: Vec<SolanaDiscoverVenue>,
        #[arg(long, default_value_t = 50_000.0)]
        min_tvl_usd: f64,
        #[arg(long, default_value_t = 25_000.0)]
        min_volume_usd_24h: f64,
        #[arg(long, default_value_t = 100.0)]
        min_fee_apr: f64,
        #[arg(long, default_value_t = 5_000.0)]
        max_fee_apr: f64,
        #[arg(long, default_value_t = 0.5)]
        min_volume_tvl_24h: f64,
        #[arg(long, default_value_t = 120)]
        page_size: usize,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long, default_value_t = 10_000.0)]
        capital_usd: f64,
        /// Range half-width as percent around current price. Repeatable.
        #[arg(long = "half-width-pct", default_values_t = [2.5, 5.0, 10.0, 20.0])]
        half_width_pct: Vec<f64>,
        /// Cap the concentration multiplier because protocol APIs do not expose
        /// local active liquidity distributions.
        #[arg(long, default_value_t = 12.0)]
        max_concentration: f64,
        #[arg(long, default_value_t = 5.0)]
        rebalance_slippage_bps: f64,
        #[arg(long, default_value_t = 0.002)]
        rebalance_tx_cost_usd: f64,
        #[arg(long, default_value_t = 12)]
        max_rebalances_per_day: u32,
        /// Optional JSON output path for the proxy replay rows.
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    /// Sample recent successful Solana CLMM pool swaps from JSON-RPC and extract
    /// pool-owned token balance deltas plus raw program data. This is the data
    /// landing layer before decoding program events into normalized SwapObs.
    SampleSolanaPoolSwaps {
        #[arg(long, default_value = "https://solana-rpc.publicnode.com")]
        rpc_url: String,
        #[arg(long)]
        pool_address: String,
        #[arg(long)]
        program_id: String,
        #[arg(long)]
        token0_mint: String,
        #[arg(long)]
        token1_mint: String,
        /// Active pool liquidity to attach to normalized swaps when the venue event
        /// does not emit per-swap liquidity, e.g. Orca Whirlpool Traded events.
        #[arg(long)]
        active_liquidity: Option<f64>,
        #[arg(long, default_value_t = 25)]
        limit: usize,
        #[arg(long, default_value_t = 100)]
        signature_scan_limit: usize,
        #[arg(long, default_value_t = 1)]
        max_signature_pages: usize,
        /// Optional signature cursor for Solana getSignaturesForAddress pagination.
        #[arg(long)]
        before_signature: Option<String>,
        /// Keep scanning until this many decoded normalized rows are available.
        #[arg(long)]
        min_normalized_swaps: Option<usize>,
        #[arg(long, default_value_t = 150)]
        request_sleep_ms: u64,
        /// Optional JSON output path for sampled swap rows.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Optional JSONL output path for decoded Raydium CLMM SwapObs rows.
        #[arg(long)]
        normalized_output: Option<PathBuf>,
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
    /// Replay a normalized swap stream (`autopool_backtest::SwapObs` JSONL) with a
    /// pool spec sidecar. This is the adapter boundary for Solana CLMM decoders.
    ReplayNormalizedSwaps {
        #[arg(long)]
        spec: PathBuf,
        /// JSONL file of normalized SwapObs rows. Defaults to `swaps.jsonl` next to
        /// the spec file.
        #[arg(long)]
        swaps: Option<PathBuf>,
        #[command(flatten)]
        params: NormalizedReplayParams,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    /// Replay normalized swaps over rolling fixed-size windows and summarize
    /// policy stability, mechanical APR, and drawdown across windows.
    ReplayNormalizedWindows {
        #[arg(long)]
        spec: PathBuf,
        /// JSONL file of normalized SwapObs rows. Defaults to `swaps.jsonl` next to
        /// the spec file.
        #[arg(long)]
        swaps: Option<PathBuf>,
        #[arg(long, default_value_t = 50)]
        window_swaps: usize,
        #[arg(long, default_value_t = 25)]
        step_swaps: usize,
        #[arg(long, default_value_t = 2)]
        min_windows: usize,
        #[command(flatten)]
        params: NormalizedReplayParams,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    /// Sweep hedge fractions over rolling normalized replay windows to find
    /// robust hedge sizing under current hot-pool regimes.
    ReplayNormalizedHedgeGrid {
        #[arg(long)]
        spec: PathBuf,
        /// JSONL file of normalized SwapObs rows. Defaults to `swaps.jsonl` next to
        /// the spec file.
        #[arg(long)]
        swaps: Option<PathBuf>,
        #[arg(long, default_value_t = 40)]
        window_swaps: usize,
        #[arg(long, default_value_t = 15)]
        step_swaps: usize,
        #[arg(long, default_value_t = 2)]
        min_windows: usize,
        /// Hedge fractions to test. Repeat the flag, e.g. --grid-hedge-fraction 0.25.
        #[arg(long = "grid-hedge-fraction", default_values_t = vec![0.0, 0.25, 0.5, 0.75, 1.0])]
        hedge_fractions: Vec<f64>,
        #[command(flatten)]
        regime_rule: RegimeHedgeRuleArgs,
        #[command(flatten)]
        params: NormalizedReplayParams,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    /// Apply strict replay promotion gates across multiple rolling-window sizes.
    /// This is the deployability filter: a hot pool must beat hold with positive
    /// left-tail net APR across short and longer windows before shadow monitoring.
    ReplayPromotionGate {
        #[arg(long)]
        spec: PathBuf,
        /// JSONL file of normalized SwapObs rows. Defaults to `swaps.jsonl` next to
        /// the spec file.
        #[arg(long)]
        swaps: Option<PathBuf>,
        /// Window config as `window_swaps:step_swaps:min_windows`. Repeatable.
        #[arg(long = "window-config")]
        window_configs: Vec<String>,
        /// Policy to gate. The default preserves the no-lookahead lagged regime rule;
        /// defensive policies let us test `hedged_wide` / `delta_hedged` directly.
        #[arg(long = "gate-policy", value_enum, default_value_t = PromotionGatePolicy::LaggedRegimeRule)]
        gate_policy: PromotionGatePolicy,
        /// Hedge fractions to test. Repeat the flag, e.g. --grid-hedge-fraction 0.25.
        #[arg(long = "grid-hedge-fraction", default_values_t = vec![0.0, 0.25, 0.5, 0.75, 1.0])]
        hedge_fractions: Vec<f64>,
        #[command(flatten)]
        regime_rule: RegimeHedgeRuleArgs,
        #[command(flatten)]
        policy_rule: RegimePolicyRuleArgs,
        #[arg(long, default_value_t = 500.0)]
        min_p05_net_apr_pct: f64,
        #[arg(long, default_value_t = 0.0)]
        min_mean_vs_hold_usd: f64,
        #[arg(long, default_value_t = 60.0)]
        min_win_rate_vs_hold_pct: f64,
        /// Max worst drawdown as share of deployed capital.
        #[arg(long, default_value_t = 0.05)]
        max_drawdown_pct: f64,
        #[command(flatten)]
        params: NormalizedReplayParams,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    /// Merge normalized SwapObs JSONL files, dedupe repeated sampled swaps, sort
    /// by block, and write a stable combined replay stream.
    MergeNormalizedSwaps {
        /// Input normalized SwapObs JSONL. Repeat for multiple samples.
        #[arg(long = "input", required = true)]
        inputs: Vec<PathBuf>,
        /// Output normalized SwapObs JSONL.
        #[arg(long)]
        output: PathBuf,
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
    /// Inspect a Slipstream NonfungiblePositionManager NFT: owner, range, liquidity,
    /// owed tokens, current pool tick, and whether the NFT appears staked in the
    /// pool gauge. Read-only; never signs.
    InspectPosition {
        #[arg(long, env = "BASE_RPC_URL")]
        rpc_url: String,
        #[arg(long)]
        token_id: u64,
        /// Optional pool address. If omitted, the command resolves it from the
        /// position tokens + tick spacing via the Slipstream pool factory.
        #[arg(long)]
        pool_address: Option<String>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    /// Poll a Slipstream NPM position and append read-only JSONL snapshots for
    /// shadow monitoring. This never signs or broadcasts.
    MonitorPosition {
        #[arg(long, env = "BASE_RPC_URL")]
        rpc_url: String,
        #[arg(long)]
        token_id: u64,
        /// Optional pool address. If omitted, the command resolves it from the
        /// position tokens + tick spacing via the Slipstream pool factory.
        #[arg(long)]
        pool_address: Option<String>,
        #[arg(long, default_value = "logs/base/positions.jsonl")]
        output: PathBuf,
        #[arg(long, default_value_t = 60)]
        poll_seconds: u64,
        /// Number of snapshots to write. Use 0 for an endless monitor.
        #[arg(long, default_value_t = 1)]
        iterations: u64,
        /// USD value of one token0. When set, monitor snapshots include token
        /// amounts, USD exposure, owed-value estimates, risk share, and alerts.
        #[arg(long)]
        token0_usd: Option<f64>,
        /// Which side is the risk asset for exposure/kill-switch checks.
        #[arg(long, value_enum, default_value_t = RiskTokenSide::Token1)]
        risk_token_side: RiskTokenSide,
        /// Maximum share of position value allowed in the risk token.
        #[arg(long, default_value_t = 0.8)]
        max_risk_token_share: f64,
        /// Warn when current tick is this close to either range edge.
        #[arg(long, default_value_t = 120)]
        min_distance_to_edge_ticks: i32,
        /// Warn when uncollected fees exceed this USD value. Requires --token0-usd.
        #[arg(long)]
        max_owed_value_usd: Option<f64>,
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
    /// pool: reads pool state, proposes the collect→decrease→collect→swap→mint
    /// (+stake) action sequence with slippage-protected min amounts, estimates gas,
    /// runs hard risk gates, and NEVER signs or broadcasts. v2 (Aerodrome V1) and v4
    /// need separate adapters — this planner is Slipstream/v3 only.
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
        /// Existing position NFT id (enables a real collect+decrease+mint multicall).
        #[arg(long)]
        token_id: Option<u64>,
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
        #[arg(long, action = clap::ArgAction::Set, default_value_t = true)]
        staked: bool,
        /// Which side should be counted as the risk inventory for the one-sided
        /// inventory gate. For WETH-USDC in natural pool order this is token0; for
        /// WETH-AERO this is usually token1/AERO.
        #[arg(long, value_enum, default_value_t = RiskTokenSide::Token1)]
        risk_token_side: RiskTokenSide,
        /// Maximum share of portfolio value allowed in the risk token after mint.
        #[arg(long, default_value_t = 0.8)]
        max_risk_token_share: f64,
        /// Recipient/owner address baked into the swap & mint calldata (set to the
        /// funded test account when executing the calldata on a local fork).
        #[arg(long, default_value = "0x000000000000000000000000000000000000dEaD")]
        recipient: String,
        /// Skip the on-chain Quoter eth_call and rely on the local v3 state
        /// simulation. Useful on forks/free RPCs where revert data or upstream
        /// account fetches are unreliable.
        #[arg(long, default_value_t = false)]
        skip_quoter: bool,
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
    /// Seconds per block/slot in the replay stream. Base uses ~2s; Solana
    /// normalized streams should use slot time, commonly ~0.4s.
    #[arg(long, default_value_t = 2.0)]
    block_seconds: f64,
    /// Which token side is the volatile/risk inventory after any --invert
    /// normalization.
    #[arg(long, value_enum, default_value_t = RiskTokenSide::Token1)]
    risk_token_side: RiskTokenSide,
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
            block_seconds: self.block_seconds,
            funding_bps_per_day: self.funding_bps_per_day,
            risk_asset_is_token1: self.risk_token_side == RiskTokenSide::Token1,
        }
    }
}

#[derive(Debug, clap::Args)]
struct NormalizedReplayParams {
    /// USD value of the replay numeraire token. If omitted, the spec must provide
    /// it (stablecoin-numeraire specs use 1.0).
    #[arg(long)]
    token0_usd: Option<f64>,
    #[arg(long, default_value_t = 10_000.0)]
    capital_usd: f64,
    #[arg(long, default_value_t = 0.005)]
    rebalance_gas_usd: f64,
    #[arg(long, default_value_t = 5.0)]
    rebalance_slippage_bps: f64,
    #[arg(long, default_value_t = 300)]
    narrow_half_width: i32,
    #[arg(long, default_value_t = 3_000)]
    wide_half_width: i32,
    #[arg(long, default_value_t = 1.5)]
    vol_k: f64,
    #[arg(long, default_value_t = 2)]
    action_delay_blocks: u64,
    #[arg(long)]
    block_seconds: Option<f64>,
    #[arg(long, value_enum)]
    risk_token_side: Option<RiskTokenSide>,
    #[arg(long, default_value_t = 0.0)]
    funding_bps_per_day: f64,
    #[arg(long, default_value_t = 1.0)]
    hedge_fraction: f64,
    #[arg(long, default_value_t = 6.0)]
    trend_exit_threshold: f64,
    /// Annual reward APR as a fraction, e.g. 0.20 for 20%.
    #[arg(long)]
    reward_apr: Option<f64>,
    #[arg(long, default_value_t = 0.0)]
    reward_haircut: f64,
}

#[derive(Debug, Clone, Copy, clap::Args)]
struct RegimeHedgeRuleArgs {
    /// Hedge fraction used after a prior range window.
    #[arg(long, default_value_t = 1.0)]
    rule_range_hedge_fraction: f64,
    /// Hedge fraction used after a prior volatile-range window.
    #[arg(long, default_value_t = 1.0)]
    rule_volatile_hedge_fraction: f64,
    /// Hedge fraction used after a prior trend toward the money side.
    #[arg(long, default_value_t = 0.25)]
    rule_trend_money_hedge_fraction: f64,
    /// Hedge fraction used after a prior trend toward the risk side.
    #[arg(long, default_value_t = 1.0)]
    rule_trend_risk_hedge_fraction: f64,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct RegimeHedgeRule {
    range_hedge_fraction: f64,
    volatile_hedge_fraction: f64,
    trend_money_hedge_fraction: f64,
    trend_risk_hedge_fraction: f64,
}

impl From<RegimeHedgeRuleArgs> for RegimeHedgeRule {
    fn from(value: RegimeHedgeRuleArgs) -> Self {
        Self {
            range_hedge_fraction: value.rule_range_hedge_fraction,
            volatile_hedge_fraction: value.rule_volatile_hedge_fraction,
            trend_money_hedge_fraction: value.rule_trend_money_hedge_fraction,
            trend_risk_hedge_fraction: value.rule_trend_risk_hedge_fraction,
        }
    }
}

impl RegimeHedgeRule {
    fn fractions(&self) -> [f64; 4] {
        [
            self.range_hedge_fraction,
            self.volatile_hedge_fraction,
            self.trend_money_hedge_fraction,
            self.trend_risk_hedge_fraction,
        ]
    }

    fn hedge_fraction_for_prior_regime(&self, regime: &str) -> f64 {
        match regime {
            "trend_down_money" => self.trend_money_hedge_fraction,
            "trend_up_risk" => self.trend_risk_hedge_fraction,
            "volatile_range" => self.volatile_hedge_fraction,
            _ => self.range_hedge_fraction,
        }
    }

    fn describe(&self) -> String {
        format!(
            "range={:.2},volatile={:.2},money_trend={:.2},risk_trend={:.2}",
            self.range_hedge_fraction,
            self.volatile_hedge_fraction,
            self.trend_money_hedge_fraction,
            self.trend_risk_hedge_fraction
        )
    }
}

#[derive(Debug, Clone, Copy, clap::Args)]
struct RegimePolicyRuleArgs {
    /// Policy used after a prior range window.
    #[arg(long, value_enum, default_value_t = RegimeSwitchPolicy::DeltaHedged)]
    rule_range_policy: RegimeSwitchPolicy,
    /// Policy used after a prior volatile-range window.
    #[arg(long, value_enum, default_value_t = RegimeSwitchPolicy::HedgedWide)]
    rule_volatile_policy: RegimeSwitchPolicy,
    /// Policy used after a prior trend toward the money side.
    #[arg(long, value_enum, default_value_t = RegimeSwitchPolicy::HedgedWide)]
    rule_trend_money_policy: RegimeSwitchPolicy,
    /// Policy used after a prior trend toward the risk side.
    #[arg(long, value_enum, default_value_t = RegimeSwitchPolicy::HedgedWide)]
    rule_trend_risk_policy: RegimeSwitchPolicy,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct RegimePolicyRule {
    range_policy: RegimeSwitchPolicy,
    volatile_policy: RegimeSwitchPolicy,
    trend_money_policy: RegimeSwitchPolicy,
    trend_risk_policy: RegimeSwitchPolicy,
}

impl From<RegimePolicyRuleArgs> for RegimePolicyRule {
    fn from(value: RegimePolicyRuleArgs) -> Self {
        Self {
            range_policy: value.rule_range_policy,
            volatile_policy: value.rule_volatile_policy,
            trend_money_policy: value.rule_trend_money_policy,
            trend_risk_policy: value.rule_trend_risk_policy,
        }
    }
}

impl RegimePolicyRule {
    fn policy_for_prior_regime(&self, regime: &str) -> &'static str {
        match regime {
            "trend_down_money" => self.trend_money_policy.label(),
            "trend_up_risk" => self.trend_risk_policy.label(),
            "volatile_range" => self.volatile_policy.label(),
            _ => self.range_policy.label(),
        }
    }

    fn describe(&self) -> String {
        format!(
            "range={},volatile={},money_trend={},risk_trend={}",
            self.range_policy.label(),
            self.volatile_policy.label(),
            self.trend_money_policy.label(),
            self.trend_risk_policy.label()
        )
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
        Command::SolanaUniverse {
            min_tvl_usd,
            min_volume_usd_1d,
            min_base_apy,
            max_reward_share,
            min_fee_bps,
            include_outliers,
            concentrated_only,
            blue_chip_only,
            projects,
            limit,
            output,
            format,
        } => {
            solana_universe(SolanaUniverseArgs {
                min_tvl_usd,
                min_volume_usd_1d,
                min_base_apy,
                max_reward_share,
                min_fee_bps,
                include_outliers,
                concentrated_only,
                blue_chip_only,
                projects,
                limit,
                output,
                format,
            })
            .await?
        }
        Command::SolanaDiscover {
            venues,
            min_tvl_usd,
            min_volume_usd_24h,
            min_fee_apr,
            max_fee_apr,
            verified_only,
            page_size,
            limit,
            output,
            format,
        } => {
            solana_discover(SolanaDiscoverArgs {
                venues,
                min_tvl_usd,
                min_volume_usd_24h,
                min_fee_apr,
                max_fee_apr,
                verified_only,
                page_size,
                limit,
                output,
                format,
            })
            .await?
        }
        Command::HotPoolCandidates {
            venues,
            min_tvl_usd,
            min_volume_usd_24h,
            min_fee_apr,
            max_fee_apr,
            target_fee_apr,
            min_volume_tvl_24h,
            page_size,
            limit,
            output,
            format,
        } => {
            hot_pool_candidates(HotPoolCandidateArgs {
                venues,
                min_tvl_usd,
                min_volume_usd_24h,
                min_fee_apr,
                max_fee_apr,
                target_fee_apr,
                min_volume_tvl_24h,
                page_size,
                limit,
                output,
                format,
            })
            .await?
        }
        Command::HotPoolExperimentPlan {
            input,
            data_dir,
            limit,
            include_p2,
            capital_usd,
            narrow_half_width,
            wide_half_width,
            max_drawdown_pct,
            max_rebalances_per_day,
            output,
            write_specs,
            format,
        } => hot_pool_experiment_plan(HotPoolExperimentArgs {
            input,
            data_dir,
            limit,
            include_p2,
            capital_usd,
            narrow_half_width,
            wide_half_width,
            max_drawdown_pct,
            max_rebalances_per_day,
            output,
            write_specs,
            format,
        })?,
        Command::SolanaProxyReplay {
            venues,
            min_tvl_usd,
            min_volume_usd_24h,
            min_fee_apr,
            max_fee_apr,
            min_volume_tvl_24h,
            page_size,
            limit,
            capital_usd,
            half_width_pct,
            max_concentration,
            rebalance_slippage_bps,
            rebalance_tx_cost_usd,
            max_rebalances_per_day,
            output,
            format,
        } => {
            solana_proxy_replay(SolanaProxyReplayArgs {
                venues,
                min_tvl_usd,
                min_volume_usd_24h,
                min_fee_apr,
                max_fee_apr,
                min_volume_tvl_24h,
                page_size,
                limit,
                capital_usd,
                half_width_pct,
                max_concentration,
                rebalance_slippage_bps,
                rebalance_tx_cost_usd,
                max_rebalances_per_day,
                output,
                format,
            })
            .await?
        }
        Command::SampleSolanaPoolSwaps {
            rpc_url,
            pool_address,
            program_id,
            token0_mint,
            token1_mint,
            active_liquidity,
            limit,
            signature_scan_limit,
            max_signature_pages,
            before_signature,
            min_normalized_swaps,
            request_sleep_ms,
            output,
            normalized_output,
            format,
        } => {
            sample_solana_pool_swaps(SampleSolanaPoolSwapsArgs {
                rpc_url,
                pool_address,
                program_id,
                token0_mint,
                token1_mint,
                active_liquidity,
                limit,
                signature_scan_limit,
                max_signature_pages,
                before_signature,
                min_normalized_swaps,
                request_sleep_ms,
                output,
                normalized_output,
                format,
            })
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
        Command::ReplayNormalizedSwaps {
            spec,
            swaps,
            params,
            format,
        } => replay_normalized_swaps(spec, swaps, &params, format)?,
        Command::ReplayNormalizedWindows {
            spec,
            swaps,
            window_swaps,
            step_swaps,
            min_windows,
            params,
            format,
        } => replay_normalized_windows(
            spec,
            swaps,
            window_swaps,
            step_swaps,
            min_windows,
            &params,
            format,
        )?,
        Command::ReplayNormalizedHedgeGrid {
            spec,
            swaps,
            window_swaps,
            step_swaps,
            min_windows,
            hedge_fractions,
            regime_rule,
            params,
            format,
        } => replay_normalized_hedge_grid(
            spec,
            swaps,
            window_swaps,
            step_swaps,
            min_windows,
            hedge_fractions,
            regime_rule.into(),
            &params,
            format,
        )?,
        Command::ReplayPromotionGate {
            spec,
            swaps,
            window_configs,
            gate_policy,
            hedge_fractions,
            regime_rule,
            policy_rule,
            min_p05_net_apr_pct,
            min_mean_vs_hold_usd,
            min_win_rate_vs_hold_pct,
            max_drawdown_pct,
            params,
            format,
        } => replay_promotion_gate(
            spec,
            swaps,
            window_configs,
            gate_policy,
            hedge_fractions,
            regime_rule.into(),
            policy_rule.into(),
            PromotionGateThresholds {
                min_p05_net_apr_pct,
                min_mean_vs_hold_usd,
                min_win_rate_vs_hold_pct,
                max_drawdown_pct,
            },
            &params,
            format,
        )?,
        Command::MergeNormalizedSwaps {
            inputs,
            output,
            format,
        } => merge_normalized_swaps(inputs, output, format)?,
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
        Command::InspectPosition {
            rpc_url,
            token_id,
            pool_address,
            format,
        } => inspect_position(rpc_url, token_id, pool_address, format).await?,
        Command::MonitorPosition {
            rpc_url,
            token_id,
            pool_address,
            output,
            poll_seconds,
            iterations,
            token0_usd,
            risk_token_side,
            max_risk_token_share,
            min_distance_to_edge_ticks,
            max_owed_value_usd,
            format,
        } => {
            monitor_position(PositionMonitorArgs {
                rpc_url,
                token_id,
                pool_address,
                output,
                poll_seconds,
                iterations,
                risk: PositionRiskOptions {
                    token0_usd,
                    risk_token_side,
                    max_risk_token_share,
                    min_distance_to_edge_ticks,
                    max_owed_value_usd,
                },
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
            token_id,
            slippage_bps,
            rebalance_gas_units,
            eth_usd,
            expected_edge_usd,
            max_gas_to_edge_pct,
            staked,
            risk_token_side,
            max_risk_token_share,
            recipient,
            skip_quoter,
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
                token_id,
                recipient,
                slippage_bps,
                rebalance_gas_units,
                eth_usd,
                expected_edge_usd,
                max_gas_to_edge_pct,
                staked,
                risk_token_side,
                max_risk_token_share,
                skip_quoter,
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

#[derive(Debug, Clone)]
struct SolanaUniverseArgs {
    min_tvl_usd: f64,
    min_volume_usd_1d: f64,
    min_base_apy: f64,
    max_reward_share: f64,
    min_fee_bps: f64,
    include_outliers: bool,
    concentrated_only: bool,
    blue_chip_only: bool,
    projects: Vec<String>,
    limit: usize,
    output: Option<PathBuf>,
    format: OutputFormat,
}

#[derive(Debug, Serialize)]
struct SolanaCandidateRow {
    project: String,
    protocol: String,
    symbol: String,
    pool_id: String,
    pool_meta: Option<String>,
    fee_bps: Option<f64>,
    tvl_usd: f64,
    volume_usd_1d: f64,
    volume_usd_7d: f64,
    volume_tvl_1d: f64,
    apy: f64,
    apy_base: f64,
    apy_reward: f64,
    reward_share: f64,
    stablecoin: Option<bool>,
    outlier: bool,
    underlying_tokens: Vec<String>,
    blue_chip_score: f64,
    deployability_score: f64,
    notes: Vec<String>,
}

fn reward_share(apy: f64, apy_reward: f64) -> f64 {
    if apy.abs() <= f64::EPSILON {
        0.0
    } else {
        (apy_reward.max(0.0) / apy.max(0.0)).clamp(0.0, 1.0)
    }
}

fn blue_chip_score(symbol: &str) -> f64 {
    let key = symbol.to_ascii_uppercase();
    let majors = [
        "SOL", "WSOL", "USDC", "USDT", "JUP", "JTO", "JITOSOL", "BONK", "WIF",
    ];
    let count = key
        .split(['-', '/', '_'])
        .filter(|part| majors.contains(part))
        .count();
    match count {
        0 => 0.0,
        1 => 0.5,
        _ => 1.0,
    }
}

fn solana_candidate_row(pool: DefiLlamaPool) -> SolanaCandidateRow {
    let snapshot = pool.clone().into_snapshot();
    let tvl_usd = pool.tvl_usd.unwrap_or(0.0);
    let volume_usd_1d = pool.volume_usd_1d.unwrap_or(0.0);
    let volume_usd_7d = pool.volume_usd_7d.unwrap_or(0.0);
    let apy = pool.apy.unwrap_or(0.0);
    let apy_base = pool.apy_base.unwrap_or(0.0);
    let apy_reward = pool.apy_reward.unwrap_or(0.0);
    let reward_share = reward_share(apy, apy_reward);
    let volume_tvl_1d = if tvl_usd > 0.0 {
        volume_usd_1d / tvl_usd
    } else {
        0.0
    };
    let blue_chip_score = blue_chip_score(&pool.symbol);
    let liquidity_weight = (1.0 + tvl_usd / 100_000.0).ln();
    let flow_weight = (1.0 + volume_usd_1d / 100_000.0).ln();
    let organic_weight = 1.0 - reward_share;
    let blue_chip_weight = 0.5 + blue_chip_score;
    let deployability_score = apy_base.max(0.0)
        * liquidity_weight.max(0.0)
        * flow_weight.max(0.0)
        * organic_weight.max(0.0)
        * blue_chip_weight;
    let mut notes = Vec::new();
    if pool.outlier.unwrap_or(false) {
        notes.push("defillama_outlier".to_string());
    }
    if reward_share > 0.25 {
        notes.push("reward_heavy".to_string());
    }
    if volume_tvl_1d > 1.0 {
        notes.push("high_turnover".to_string());
    }
    if blue_chip_score < 0.5 {
        notes.push("long_tail_inventory".to_string());
    }
    if snapshot.pool.fee_tier_bps.is_none() {
        notes.push("fee_tier_missing".to_string());
    }

    SolanaCandidateRow {
        project: pool.project,
        protocol: format!("{:?}", snapshot.pool.protocol),
        symbol: pool.symbol,
        pool_id: pool.pool,
        pool_meta: pool.pool_meta,
        fee_bps: snapshot.pool.fee_tier_bps,
        tvl_usd,
        volume_usd_1d,
        volume_usd_7d,
        volume_tvl_1d,
        apy,
        apy_base,
        apy_reward,
        reward_share,
        stablecoin: pool.stablecoin,
        outlier: pool.outlier.unwrap_or(false),
        underlying_tokens: pool.underlying_tokens.unwrap_or_default(),
        blue_chip_score,
        deployability_score,
        notes,
    }
}

fn is_concentrated_solana_candidate(row: &SolanaCandidateRow) -> bool {
    let project = row.project.to_ascii_lowercase();
    let meta = row.pool_meta.as_deref().unwrap_or("").to_ascii_lowercase();
    match project.as_str() {
        "raydium-amm" => meta.contains("concentrated"),
        "orca-dex" => true,
        "kamino-liquidity" => true,
        "meteora" | "meteora-dlmm" => true,
        _ => false,
    }
}

async fn solana_universe(args: SolanaUniverseArgs) -> Result<()> {
    let client = DefiLlamaClient::default();
    let projects = args
        .projects
        .iter()
        .map(|project| project.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let mut rows = client
        .fetch_pools()
        .await?
        .into_iter()
        .filter(|pool| pool.chain.eq_ignore_ascii_case("Solana"))
        .filter(|pool| projects.is_empty() || projects.contains(&pool.project.to_ascii_lowercase()))
        .filter(|pool| {
            pool.il_risk.as_deref() == Some("yes") || pool.exposure.as_deref() == Some("multi")
        })
        .map(solana_candidate_row)
        .filter(|row| args.include_outliers || !row.outlier)
        .filter(|row| row.tvl_usd >= args.min_tvl_usd)
        .filter(|row| row.volume_usd_1d >= args.min_volume_usd_1d)
        .filter(|row| row.apy_base >= args.min_base_apy)
        .filter(|row| row.reward_share <= args.max_reward_share)
        .filter(|row| row.fee_bps.unwrap_or(0.0) >= args.min_fee_bps)
        .filter(|row| !args.concentrated_only || is_concentrated_solana_candidate(row))
        .filter(|row| !args.blue_chip_only || row.blue_chip_score >= 0.5)
        .collect::<Vec<_>>();

    rows.sort_by(|left, right| {
        right
            .deployability_score
            .total_cmp(&left.deployability_score)
    });

    let limited_rows = rows.into_iter().take(args.limit).collect::<Vec<_>>();
    if let Some(output) = args.output.as_ref() {
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating output directory {}", parent.display()))?;
        }
        fs::write(output, serde_json::to_string_pretty(&limited_rows)?)
            .with_context(|| format!("writing Solana universe snapshot {}", output.display()))?;
    }

    if args.format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&limited_rows)?);
        return Ok(());
    }

    println!(
        "solana LP universe projects={:?} filters: tvl>={:.0} vol1d>={:.0} base_apy>={:.1}% reward_share<={:.0}% outliers={} concentrated_only={} blue_chip_only={}",
        args.projects,
        args.min_tvl_usd,
        args.min_volume_usd_1d,
        args.min_base_apy,
        args.max_reward_share * 100.0,
        args.include_outliers,
        args.concentrated_only,
        args.blue_chip_only
    );
    println!(
        "{:<13} {:<18} {:<24} {:>8} {:>10} {:>12} {:>8} {:>8} {:>8} {:>10}  notes",
        "project",
        "protocol",
        "symbol",
        "fee_bps",
        "tvl_usd",
        "vol_1d",
        "vol/tvl",
        "base",
        "reward",
        "score"
    );
    for row in limited_rows {
        println!(
            "{:<13} {:<18} {:<24} {:>8} {:>10.0} {:>12.0} {:>8.2} {:>7.2}% {:>7.2}% {:>10.1}  {}",
            row.project,
            row.protocol,
            row.symbol,
            row.fee_bps
                .map(|value| format!("{value:.2}"))
                .unwrap_or_else(|| "-".to_string()),
            row.tvl_usd,
            row.volume_usd_1d,
            row.volume_tvl_1d,
            row.apy_base,
            row.apy_reward,
            row.deployability_score,
            row.notes.join(",")
        );
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct SolanaDiscoverArgs {
    venues: Vec<SolanaDiscoverVenue>,
    min_tvl_usd: f64,
    min_volume_usd_24h: f64,
    min_fee_apr: f64,
    max_fee_apr: f64,
    verified_only: bool,
    page_size: usize,
    limit: usize,
    output: Option<PathBuf>,
    format: OutputFormat,
}

async fn solana_discover(args: SolanaDiscoverArgs) -> Result<()> {
    let venues = if args.venues.is_empty() {
        vec![
            SolanaVenue::OrcaWhirlpool,
            SolanaVenue::RaydiumClmm,
            SolanaVenue::MeteoraDlmm,
        ]
    } else {
        args.venues.iter().copied().map(Into::into).collect()
    };
    let options = DiscoveryOptions {
        min_tvl_usd: args.min_tvl_usd,
        page_size: args.page_size,
    };
    let client = SolanaDiscoveryClient::default();
    let mut rows = client.discover_many(&venues, &options).await?;

    rows.retain(|row| row.tvl_usd >= args.min_tvl_usd);
    rows.retain(|row| row.volume_usd_24h.unwrap_or(0.0) >= args.min_volume_usd_24h);
    rows.retain(|row| row.fee_apr_24h.unwrap_or(0.0) >= args.min_fee_apr);
    rows.retain(|row| row.fee_apr_24h.unwrap_or(f64::INFINITY) <= args.max_fee_apr);
    rows.retain(|row| !args.verified_only || row.verified);
    rows.sort_by(|left, right| {
        right
            .deployability_score
            .total_cmp(&left.deployability_score)
    });

    let limited_rows = rows.into_iter().take(args.limit).collect::<Vec<_>>();
    if let Some(output) = args.output.as_ref() {
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating output directory {}", parent.display()))?;
        }
        fs::write(output, serde_json::to_string_pretty(&limited_rows)?)
            .with_context(|| format!("writing Solana discovery snapshot {}", output.display()))?;
    }

    if args.format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&limited_rows)?);
        return Ok(());
    }

    println!(
        "solana protocol discovery venues={:?} filters: tvl>={:.0} vol24h>={:.0} fee_apr={:.1}%..{:.1}% verified_only={} page_size={}",
        venues.iter().map(|venue| venue.label()).collect::<Vec<_>>(),
        args.min_tvl_usd,
        args.min_volume_usd_24h,
        args.min_fee_apr,
        args.max_fee_apr,
        args.verified_only,
        args.page_size
    );
    println!(
        "{:<8} {:<24} {:>8} {:>10} {:>12} {:>9} {:>9} {:>10} {:<12}  warnings",
        "venue",
        "symbol",
        "fee_bps",
        "tvl_usd",
        "vol_24h",
        "fee_apr",
        "total_apr",
        "score",
        "spacing"
    );
    for row in limited_rows {
        println!(
            "{:<8} {:<24} {:>8} {:>10.0} {:>12.0} {:>8} {:>8} {:>10.1} {:<12}  {}",
            row.venue.label(),
            row.symbol,
            format_opt_f64(row.fee_bps, 2),
            row.tvl_usd,
            row.volume_usd_24h.unwrap_or(0.0),
            format_opt_pct(row.fee_apr_24h),
            format_opt_pct(row.total_apr),
            row.deployability_score,
            solana_spacing_label(&row),
            row.warnings.join(",")
        );
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct HotPoolCandidateArgs {
    venues: Vec<SolanaDiscoverVenue>,
    min_tvl_usd: f64,
    min_volume_usd_24h: f64,
    min_fee_apr: f64,
    max_fee_apr: f64,
    target_fee_apr: f64,
    min_volume_tvl_24h: f64,
    page_size: usize,
    limit: usize,
    output: Option<PathBuf>,
    format: OutputFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HotPoolTokenRow {
    address: String,
    symbol: String,
    name: Option<String>,
    decimals: Option<u8>,
    verified: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HotPoolCandidateRow {
    venue: String,
    #[serde(default)]
    pool_kind: String,
    symbol: String,
    pool_address: String,
    #[serde(default)]
    tokens: Vec<HotPoolTokenRow>,
    fee_bps: Option<f64>,
    tick_spacing: Option<i32>,
    bin_step: Option<i32>,
    tvl_usd: f64,
    volume_usd_24h: f64,
    volume_tvl_24h: f64,
    fee_apr_24h: f64,
    fee_apr_7d: Option<f64>,
    reward_apr_pct: Option<f64>,
    total_apr: Option<f64>,
    formula_fee_apr_24h: Option<f64>,
    reported_to_formula_apr: Option<f64>,
    current_price: Option<f64>,
    price_min_24h: Option<f64>,
    price_max_24h: Option<f64>,
    price_range_24h_pct: Option<f64>,
    price_change_24h_pct: Option<f64>,
    current_tick: Option<i32>,
    active_liquidity: Option<String>,
    updated_slot: Option<u64>,
    #[serde(default)]
    verified: bool,
    target_fee_apr: f64,
    required_volume_tvl_for_target: Option<f64>,
    target_progress: Option<f64>,
    hot_score: f64,
    autoresearch_status: String,
    experiment_priority: String,
    next_step: String,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct HotPoolExperimentArgs {
    input: PathBuf,
    data_dir: PathBuf,
    limit: usize,
    include_p2: bool,
    capital_usd: f64,
    narrow_half_width: i32,
    wide_half_width: i32,
    max_drawdown_pct: f64,
    max_rebalances_per_day: u32,
    output: Option<PathBuf>,
    write_specs: bool,
    format: OutputFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReplayPoolSpec {
    chain: String,
    venue: String,
    pool_kind: String,
    pool_address: String,
    symbol: String,
    token0: Option<HotPoolTokenRow>,
    token1: Option<HotPoolTokenRow>,
    fee_bps: f64,
    tick_spacing: Option<i32>,
    bin_step: Option<i32>,
    replay_model: String,
    invert_for_numeraire: bool,
    token0_usd: Option<f64>,
    block_seconds: f64,
    risk_token_side: String,
    reward_apr_fraction: Option<f64>,
    active_liquidity: Option<String>,
    source_priority: String,
    source_warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct HotPoolPromotionGates {
    min_windows: usize,
    min_win_rate_vs_hold_and_wide: f64,
    require_positive_fee_minus_lvr: bool,
    max_drawdown_usd: f64,
    max_rebalances_per_day: u32,
    require_capacity_check: bool,
    require_api_cross_check: bool,
}

#[derive(Debug, Clone, Serialize)]
struct HotPoolExperimentRow {
    experiment_id: String,
    venue: String,
    symbol: String,
    pool_address: String,
    priority: String,
    status: String,
    next_step: String,
    replay_model: String,
    normalized_swaps_path: String,
    spec_path: String,
    normalized_swaps: usize,
    candidate_fee_apr_24h: f64,
    formula_fee_apr_24h: Option<f64>,
    volume_tvl_24h: f64,
    price_range_24h_pct: Option<f64>,
    price_change_24h_pct: Option<f64>,
    target_progress: Option<f64>,
    capital_usd: f64,
    baseline_policies: Vec<String>,
    data_requirements: Vec<String>,
    reject_reasons: Vec<String>,
    promotion_gates: HotPoolPromotionGates,
    replay_command: Option<String>,
    spec: ReplayPoolSpec,
}

#[derive(Debug, Clone)]
struct SolanaProxyReplayArgs {
    venues: Vec<SolanaDiscoverVenue>,
    min_tvl_usd: f64,
    min_volume_usd_24h: f64,
    min_fee_apr: f64,
    max_fee_apr: f64,
    min_volume_tvl_24h: f64,
    page_size: usize,
    limit: usize,
    capital_usd: f64,
    half_width_pct: Vec<f64>,
    max_concentration: f64,
    rebalance_slippage_bps: f64,
    rebalance_tx_cost_usd: f64,
    max_rebalances_per_day: u32,
    output: Option<PathBuf>,
    format: OutputFormat,
}

#[derive(Debug, Clone, Serialize)]
struct SolanaProxyReplayRow {
    venue: String,
    symbol: String,
    pool_address: String,
    pool_kind: String,
    fee_bps: Option<f64>,
    tvl_usd: f64,
    volume_usd_24h: f64,
    volume_tvl_24h: f64,
    pool_fee_apr_24h: f64,
    pool_fee_apr_7d: Option<f64>,
    price_range_24h_pct: Option<f64>,
    price_change_24h_pct: Option<f64>,
    modeled_price_range_pct: Option<f64>,
    business_status: String,
    best: SolanaProxyScenario,
    scenarios: Vec<SolanaProxyScenario>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SolanaProxyScenario {
    half_width_pct: f64,
    estimated_concentration: f64,
    no_rebalance_occupancy: f64,
    estimated_rebalances_per_day: u32,
    gross_fee_apr_proxy: f64,
    churn_cost_apr: f64,
    net_fee_apr_proxy: f64,
    max_inventory_drawdown_proxy_pct: f64,
    risk_grade: String,
    verdict: String,
}

async fn hot_pool_candidates(args: HotPoolCandidateArgs) -> Result<()> {
    let venues = if args.venues.is_empty() {
        vec![
            SolanaVenue::OrcaWhirlpool,
            SolanaVenue::RaydiumClmm,
            SolanaVenue::MeteoraDlmm,
        ]
    } else {
        args.venues.iter().copied().map(Into::into).collect()
    };
    let options = DiscoveryOptions {
        min_tvl_usd: args.min_tvl_usd,
        page_size: args.page_size,
    };
    let client = SolanaDiscoveryClient::default();
    let mut rows = client
        .discover_many(&venues, &options)
        .await?
        .into_iter()
        .filter_map(|candidate| hot_pool_candidate_row(candidate, &args))
        .filter(|row| row.autoresearch_status != "discard")
        .collect::<Vec<_>>();

    rows.sort_by(|left, right| right.hot_score.total_cmp(&left.hot_score));
    let limited_rows = rows.into_iter().take(args.limit).collect::<Vec<_>>();
    if let Some(output) = args.output.as_ref() {
        if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating output directory {}", parent.display()))?;
        }
        fs::write(output, serde_json::to_string_pretty(&limited_rows)?)
            .with_context(|| format!("writing hot-pool candidate queue {}", output.display()))?;
    }

    if args.format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&limited_rows)?);
        return Ok(());
    }

    println!(
        "hot-pool candidates venues={:?} filters: tvl>={:.0} vol24h>={:.0} fee_apr={:.1}%..{:.1}% vol/tvl>={:.2} target={:.0}%",
        venues.iter().map(|venue| venue.label()).collect::<Vec<_>>(),
        args.min_tvl_usd,
        args.min_volume_usd_24h,
        args.min_fee_apr,
        args.max_fee_apr,
        args.min_volume_tvl_24h,
        args.target_fee_apr
    );
    println!(
        "{:<8} {:<22} {:>7} {:>9} {:>10} {:>7} {:>8} {:>8} {:<18} {:<20}  warnings",
        "venue",
        "symbol",
        "fee",
        "tvl",
        "vol24h",
        "v/tvl",
        "fee_apr",
        "score",
        "priority",
        "next_step"
    );
    for row in limited_rows {
        println!(
            "{:<8} {:<22} {:>7} {:>9.0} {:>10.0} {:>7.2} {:>7.1}% {:>8.1} {:<18} {:<20}  {}",
            row.venue,
            row.symbol,
            format_opt_f64(row.fee_bps, 1),
            row.tvl_usd,
            row.volume_usd_24h,
            row.volume_tvl_24h,
            row.fee_apr_24h,
            row.hot_score,
            row.experiment_priority,
            row.next_step,
            row.warnings.join(",")
        );
    }

    Ok(())
}

fn hot_pool_candidate_row(
    candidate: SolanaPoolCandidate,
    args: &HotPoolCandidateArgs,
) -> Option<HotPoolCandidateRow> {
    let fee_apr = candidate.fee_apr_24h?;
    let volume = candidate.volume_usd_24h?;
    if candidate.tvl_usd < args.min_tvl_usd
        || volume < args.min_volume_usd_24h
        || fee_apr < args.min_fee_apr
        || fee_apr > args.max_fee_apr
    {
        return None;
    }
    let volume_tvl = if candidate.tvl_usd > 0.0 {
        volume / candidate.tvl_usd
    } else {
        0.0
    };
    if volume_tvl < args.min_volume_tvl_24h {
        return None;
    }
    let required_volume_tvl_for_target = candidate
        .fee_bps
        .and_then(|fee_bps| required_volume_tvl_for_apr(args.target_fee_apr, fee_bps));
    let target_progress = required_volume_tvl_for_target.and_then(|required| {
        if required > 0.0 {
            Some(volume_tvl / required)
        } else {
            None
        }
    });
    let formula_fee_apr = candidate
        .fee_bps
        .map(|fee_bps| formula_fee_apr_from_volume_tvl(volume_tvl, fee_bps));
    let reported_to_formula_apr = formula_fee_apr.and_then(|formula| {
        if formula > 0.0 {
            Some(fee_apr / formula)
        } else {
            None
        }
    });
    let mut warnings = candidate.warnings.clone();
    if let (Some(formula), Some(ratio)) = (formula_fee_apr, reported_to_formula_apr) {
        if fee_apr > 100.0 && ratio > 5.0 && fee_apr - formula > 100.0 {
            warnings.push("fee_apr_formula_mismatch".to_string());
        }
    }
    warnings.sort();
    warnings.dedup();
    let warning_penalty = hot_warning_penalty(&warnings);
    let hot_score = fee_apr
        * (1.0 + volume_tvl).ln().max(0.0)
        * (1.0 + candidate.tvl_usd / 100_000.0).ln().max(0.0)
        * warning_penalty;
    let (experiment_priority, next_step) = hot_pool_priority(&warnings, fee_apr, target_progress);

    Some(HotPoolCandidateRow {
        venue: candidate.venue.label().to_string(),
        pool_kind: candidate.pool_kind,
        symbol: candidate.symbol,
        pool_address: candidate.pool_address,
        tokens: candidate
            .tokens
            .into_iter()
            .map(|token| HotPoolTokenRow {
                address: token.address,
                symbol: token.symbol,
                name: token.name,
                decimals: token.decimals,
                verified: token.verified,
            })
            .collect(),
        fee_bps: candidate.fee_bps,
        tick_spacing: candidate.tick_spacing,
        bin_step: candidate.bin_step,
        tvl_usd: candidate.tvl_usd,
        volume_usd_24h: volume,
        volume_tvl_24h: volume_tvl,
        fee_apr_24h: fee_apr,
        fee_apr_7d: candidate.fee_apr_7d,
        reward_apr_pct: candidate.reward_apr,
        total_apr: candidate.total_apr,
        formula_fee_apr_24h: formula_fee_apr,
        reported_to_formula_apr,
        current_price: candidate.current_price,
        price_min_24h: candidate.price_min_24h,
        price_max_24h: candidate.price_max_24h,
        price_range_24h_pct: candidate.price_range_24h_pct,
        price_change_24h_pct: candidate.price_change_24h_pct,
        current_tick: candidate.current_tick,
        active_liquidity: candidate.active_liquidity,
        updated_slot: candidate.updated_slot,
        verified: candidate.verified,
        target_fee_apr: args.target_fee_apr,
        required_volume_tvl_for_target,
        target_progress,
        hot_score,
        autoresearch_status: "needs_validation".to_string(),
        experiment_priority,
        next_step,
        warnings,
    })
}

fn formula_fee_apr_from_volume_tvl(volume_tvl_24h: f64, fee_bps: f64) -> f64 {
    volume_tvl_24h.max(0.0) * (fee_bps.max(0.0) / 10_000.0) * 365.0 * 100.0
}

fn required_volume_tvl_for_apr(target_fee_apr: f64, fee_bps: f64) -> Option<f64> {
    let fee_fraction = fee_bps / 10_000.0;
    if target_fee_apr <= 0.0 || fee_fraction <= 0.0 {
        return None;
    }
    Some((target_fee_apr / 100.0) / (fee_fraction * 365.0))
}

fn hot_warning_penalty(warnings: &[String]) -> f64 {
    warnings.iter().fold(1.0_f64, |penalty, warning| {
        let factor = match warning.as_str() {
            "fee_apr_outlier" => 0.25,
            "fee_apr_formula_mismatch" => 0.2,
            "unverified_or_warning" => 0.35,
            "long_tail_inventory" => 0.55,
            "meteora_daily_ratio_disagrees_with_apy" => 0.65,
            "wide_price_range_24h" => 0.80,
            "large_price_move_24h" => 0.80,
            "high_turnover" => 0.9,
            _ => 0.8,
        };
        penalty * factor
    })
}

fn hot_pool_priority(
    warnings: &[String],
    fee_apr: f64,
    target_progress: Option<f64>,
) -> (String, String) {
    let has_warning = |needle: &str| warnings.iter().any(|warning| warning == needle);
    if has_warning("fee_apr_outlier")
        || has_warning("fee_apr_formula_mismatch")
        || has_warning("unverified_or_warning")
    {
        return (
            "P2_validate_api".to_string(),
            "verify_api_first".to_string(),
        );
    }
    if has_warning("meteora_daily_ratio_disagrees_with_apy") || has_warning("long_tail_inventory") {
        return (
            "P1_verify_replay".to_string(),
            "freeze_state_replay".to_string(),
        );
    }
    if fee_apr >= 500.0 || target_progress.unwrap_or(0.0) >= 0.5 {
        return ("P0_replay_now".to_string(), "replay_baselines".to_string());
    }
    (
        "P1_replay_queue".to_string(),
        "replay_baselines".to_string(),
    )
}

async fn solana_proxy_replay(args: SolanaProxyReplayArgs) -> Result<()> {
    let venues = if args.venues.is_empty() {
        vec![
            SolanaVenue::OrcaWhirlpool,
            SolanaVenue::RaydiumClmm,
            SolanaVenue::MeteoraDlmm,
        ]
    } else {
        args.venues.iter().copied().map(Into::into).collect()
    };
    let options = DiscoveryOptions {
        min_tvl_usd: args.min_tvl_usd,
        page_size: args.page_size,
    };
    let client = SolanaDiscoveryClient::default();
    let mut rows = client
        .discover_many(&venues, &options)
        .await?
        .into_iter()
        .filter_map(|candidate| solana_proxy_replay_row(candidate, &args))
        .collect::<Vec<_>>();

    rows.sort_by(|left, right| {
        right
            .best
            .net_fee_apr_proxy
            .total_cmp(&left.best.net_fee_apr_proxy)
    });
    let limited_rows = rows.into_iter().take(args.limit).collect::<Vec<_>>();

    if let Some(output) = args.output.as_ref() {
        if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating output directory {}", parent.display()))?;
        }
        fs::write(output, serde_json::to_string_pretty(&limited_rows)?)
            .with_context(|| format!("writing Solana proxy replay {}", output.display()))?;
    }

    if args.format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&limited_rows)?);
        return Ok(());
    }

    println!(
        "solana proxy replay venues={:?} capital=${:.0} hw={:?}% filters: tvl>={:.0} vol24h>={:.0} fee_apr={:.1}%..{:.1}% vol/tvl>={:.2}",
        venues.iter().map(|venue| venue.label()).collect::<Vec<_>>(),
        args.capital_usd,
        args.half_width_pct,
        args.min_tvl_usd,
        args.min_volume_usd_24h,
        args.min_fee_apr,
        args.max_fee_apr,
        args.min_volume_tvl_24h,
    );
    println!(
        "{:<8} {:<22} {:>7} {:>8} {:>8} {:>8} {:>8} {:>7} {:>6} {:>9} {:>9} {:<9}  verdict",
        "venue",
        "symbol",
        "feeAPR",
        "range24",
        "bestHW",
        "conc",
        "gross",
        "cost",
        "reb/d",
        "netAPR",
        "ddProxy",
        "risk"
    );
    for row in &limited_rows {
        println!(
            "{:<8} {:<22} {:>6.1}% {:>7} {:>7.1}% {:>8.2} {:>7.0}% {:>6.1}% {:>6} {:>8.0}% {:>8.1}% {:<9}  {}",
            row.venue,
            row.symbol,
            row.pool_fee_apr_24h,
            row.modeled_price_range_pct
                .map(|value| format!("{value:.1}%"))
                .unwrap_or_else(|| "-".to_string()),
            row.best.half_width_pct,
            row.best.estimated_concentration,
            row.best.gross_fee_apr_proxy,
            row.best.churn_cost_apr,
            row.best.estimated_rebalances_per_day,
            row.best.net_fee_apr_proxy,
            row.best.max_inventory_drawdown_proxy_pct,
            row.best.risk_grade,
            row.best.verdict
        );
    }

    Ok(())
}

fn solana_proxy_replay_row(
    candidate: SolanaPoolCandidate,
    args: &SolanaProxyReplayArgs,
) -> Option<SolanaProxyReplayRow> {
    let fee_apr = candidate.fee_apr_24h?;
    let volume = candidate.volume_usd_24h?;
    if candidate.tvl_usd < args.min_tvl_usd
        || volume < args.min_volume_usd_24h
        || fee_apr < args.min_fee_apr
        || fee_apr > args.max_fee_apr
    {
        return None;
    }
    let volume_tvl = if candidate.tvl_usd > 0.0 {
        volume / candidate.tvl_usd
    } else {
        0.0
    };
    if volume_tvl < args.min_volume_tvl_24h {
        return None;
    }

    let modeled_range = modeled_price_range_pct(&candidate);
    let mut scenarios = args
        .half_width_pct
        .iter()
        .copied()
        .filter(|half_width| *half_width > 0.0)
        .map(|half_width| solana_proxy_scenario(&candidate, half_width, modeled_range, args))
        .collect::<Vec<_>>();
    if scenarios.is_empty() {
        return None;
    }
    scenarios.sort_by(|left, right| right.net_fee_apr_proxy.total_cmp(&left.net_fee_apr_proxy));
    let best = scenarios[0].clone();
    let business_status = if candidate.venue == SolanaVenue::MeteoraDlmm {
        "proxy_only_requires_dlmm_replay".to_string()
    } else if modeled_range.is_none() {
        "proxy_fee_only_missing_price_range".to_string()
    } else {
        "proxy_ready_needs_tick_replay".to_string()
    };

    Some(SolanaProxyReplayRow {
        venue: candidate.venue.label().to_string(),
        symbol: candidate.symbol,
        pool_address: candidate.pool_address,
        pool_kind: candidate.pool_kind,
        fee_bps: candidate.fee_bps,
        tvl_usd: candidate.tvl_usd,
        volume_usd_24h: volume,
        volume_tvl_24h: volume_tvl,
        pool_fee_apr_24h: fee_apr,
        pool_fee_apr_7d: candidate.fee_apr_7d,
        price_range_24h_pct: candidate.price_range_24h_pct,
        price_change_24h_pct: candidate.price_change_24h_pct,
        modeled_price_range_pct: modeled_range,
        business_status,
        best,
        scenarios,
        warnings: candidate.warnings,
    })
}

fn solana_proxy_scenario(
    candidate: &SolanaPoolCandidate,
    half_width_pct: f64,
    modeled_range_pct: Option<f64>,
    args: &SolanaProxyReplayArgs,
) -> SolanaProxyScenario {
    let range_log = modeled_range_pct
        .map(|pct| (1.0 + pct.max(0.0) / 100.0).ln())
        .unwrap_or(0.0);
    let half_log = (1.0 + half_width_pct.max(0.0) / 100.0).ln();
    let band_log = (2.0 * half_log).max(1e-9);
    let range_over_band = if range_log > 0.0 {
        (range_log / band_log).max(1.0)
    } else {
        1.0
    };
    let estimated_concentration = range_over_band.min(args.max_concentration.max(1.0));
    let no_rebalance_occupancy = if range_log > 0.0 {
        (band_log / range_log).min(1.0)
    } else {
        1.0
    };
    let estimated_rebalances_per_day = if range_log > band_log {
        (range_log / band_log).ceil() as u32 - 1
    } else {
        0
    };
    let executable_rebalances = estimated_rebalances_per_day.min(args.max_rebalances_per_day);
    let rebalance_coverage = if estimated_rebalances_per_day == 0 {
        1.0
    } else {
        executable_rebalances as f64 / estimated_rebalances_per_day as f64
    };
    let pool_fee_apr = candidate.fee_apr_24h.unwrap_or_default();
    let gross_fee_apr_proxy = pool_fee_apr * estimated_concentration * rebalance_coverage;
    let per_rebalance_cost_usd = args.rebalance_tx_cost_usd
        + args.capital_usd * 0.5 * args.rebalance_slippage_bps / 10_000.0;
    let churn_cost_apr = if args.capital_usd > 0.0 {
        per_rebalance_cost_usd * estimated_rebalances_per_day as f64 * 365.0 / args.capital_usd
            * 100.0
    } else {
        0.0
    };
    let net_fee_apr_proxy = (gross_fee_apr_proxy - churn_cost_apr).max(0.0);
    let max_inventory_drawdown_proxy_pct =
        inventory_drawdown_proxy_pct(modeled_range_pct, half_width_pct, candidate);
    let risk_grade = if modeled_range_pct.is_none() {
        "unknown".to_string()
    } else {
        proxy_risk_grade(
            max_inventory_drawdown_proxy_pct,
            estimated_rebalances_per_day,
            half_width_pct,
            candidate,
        )
    };
    let verdict = proxy_verdict(net_fee_apr_proxy, &risk_grade, candidate);

    SolanaProxyScenario {
        half_width_pct,
        estimated_concentration,
        no_rebalance_occupancy,
        estimated_rebalances_per_day,
        gross_fee_apr_proxy,
        churn_cost_apr,
        net_fee_apr_proxy,
        max_inventory_drawdown_proxy_pct,
        risk_grade,
        verdict,
    }
}

fn modeled_price_range_pct(candidate: &SolanaPoolCandidate) -> Option<f64> {
    candidate.price_range_24h_pct.or_else(|| {
        candidate
            .price_change_24h_pct
            .map(|change| (change.abs() * 2.0).max(change.abs()))
    })
}

fn inventory_drawdown_proxy_pct(
    modeled_range_pct: Option<f64>,
    half_width_pct: f64,
    candidate: &SolanaPoolCandidate,
) -> f64 {
    let range =
        modeled_range_pct.unwrap_or_else(|| candidate.price_change_24h_pct.unwrap_or(0.0).abs());
    let out_of_range_move = (range / 2.0 - half_width_pct).max(0.0);
    let il = constant_product_il_pct(range / 100.0);
    (il + out_of_range_move * 0.65).max(il).min(100.0)
}

fn constant_product_il_pct(abs_move_fraction: f64) -> f64 {
    let up = 1.0 + abs_move_fraction.max(0.0);
    if up <= 0.0 {
        return 0.0;
    }
    (1.0 - 2.0 * up.sqrt() / (1.0 + up)) * 100.0
}

fn proxy_risk_grade(
    drawdown_pct: f64,
    rebalances_per_day: u32,
    half_width_pct: f64,
    candidate: &SolanaPoolCandidate,
) -> String {
    let long_tail = solana_major_token_score(&candidate.tokens) < 0.5;
    if drawdown_pct >= 20.0 || rebalances_per_day > 12 || (long_tail && half_width_pct <= 5.0) {
        "severe".to_string()
    } else if drawdown_pct >= 10.0 || rebalances_per_day > 6 || long_tail {
        "high".to_string()
    } else if drawdown_pct >= 5.0 || rebalances_per_day > 2 {
        "medium".to_string()
    } else {
        "low".to_string()
    }
}

fn solana_major_token_score(tokens: &[SolanaToken]) -> f64 {
    let majors = [
        "SOL", "WSOL", "USDC", "USDT", "JUP", "JTO", "JITOSOL", "BONK", "WIF", "RAY",
    ];
    let count = tokens
        .iter()
        .filter(|token| majors.contains(&token.symbol.to_ascii_uppercase().as_str()))
        .count();
    match count {
        0 => 0.0,
        1 => 0.5,
        _ => 1.0,
    }
}

fn proxy_verdict(net_apr: f64, risk_grade: &str, candidate: &SolanaPoolCandidate) -> String {
    if candidate.venue == SolanaVenue::MeteoraDlmm {
        return "needs_dlmm_replay".to_string();
    }
    match risk_grade {
        "unknown" => "needs_price_range_or_replay".to_string(),
        "severe" if net_apr >= 1_000.0 => "shadow_only_tail_risk".to_string(),
        "severe" => "reject_until_replay".to_string(),
        "high" if net_apr >= 500.0 => "candidate_shadow".to_string(),
        "medium" | "low" if net_apr >= 200.0 => "candidate_replay".to_string(),
        _ => "weak_after_risk".to_string(),
    }
}

const RAYDIUM_CLMM_PROGRAM_ID: &str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";
const RAYDIUM_CLMM_SWAP_EVENT_DISCRIMINATOR: [u8; 8] =
    [0x40, 0xc6, 0xcd, 0xe8, 0x26, 0x08, 0x71, 0xe2];
const RAYDIUM_CLMM_SWAP_EVENT_LEN: usize = 221;
const ORCA_WHIRLPOOL_PROGRAM_ID: &str = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc";
const ORCA_WHIRLPOOL_TRADED_EVENT_DISCRIMINATOR: [u8; 8] =
    [0xe1, 0xca, 0x49, 0xaf, 0x93, 0x2b, 0xa0, 0x96];
const ORCA_WHIRLPOOL_TRADED_EVENT_LEN: usize = 121;
const TWO_POW_64_F64: f64 = 18_446_744_073_709_551_616.0;
const TWO_POW_32_F64: f64 = 4_294_967_296.0;

#[derive(Debug, Clone)]
struct SampleSolanaPoolSwapsArgs {
    rpc_url: String,
    pool_address: String,
    program_id: String,
    token0_mint: String,
    token1_mint: String,
    active_liquidity: Option<f64>,
    limit: usize,
    signature_scan_limit: usize,
    max_signature_pages: usize,
    before_signature: Option<String>,
    min_normalized_swaps: Option<usize>,
    request_sleep_ms: u64,
    output: Option<PathBuf>,
    normalized_output: Option<PathBuf>,
    format: OutputFormat,
}

#[derive(Debug, Clone, Serialize)]
struct SolanaPoolSwapSample {
    signature: String,
    slot: u64,
    block_time: Option<i64>,
    instruction: String,
    token0_mint: String,
    token1_mint: String,
    token0_pool_delta_raw: i128,
    token1_pool_delta_raw: i128,
    token0_decimals: Option<u8>,
    token1_decimals: Option<u8>,
    token0_in: bool,
    token1_in: bool,
    program_data_count: usize,
    program_data_base64: Vec<String>,
    raydium_clmm_events: Vec<RaydiumClmmSwapEvent>,
    orca_whirlpool_events: Vec<OrcaWhirlpoolTradedEvent>,
    normalized_swap_previews: Vec<SolanaSwapObsPreview>,
}

#[derive(Debug, Clone, Serialize)]
struct RaydiumClmmSwapEvent {
    pool_state: String,
    sender: String,
    token_account_0: String,
    token_account_1: String,
    amount_0: u64,
    transfer_fee_0: u64,
    amount_1: u64,
    transfer_fee_1: u64,
    zero_for_one: bool,
    #[serde(serialize_with = "serialize_u128_as_string")]
    sqrt_price_x64: u128,
    #[serde(serialize_with = "serialize_u128_as_string")]
    liquidity: u128,
    tick: i32,
    trade_fee_0: u64,
    trade_fee_1: u64,
}

#[derive(Debug, Clone, Serialize)]
struct OrcaWhirlpoolTradedEvent {
    whirlpool: String,
    a_to_b: bool,
    #[serde(serialize_with = "serialize_u128_as_string")]
    pre_sqrt_price: u128,
    #[serde(serialize_with = "serialize_u128_as_string")]
    post_sqrt_price: u128,
    input_amount: u64,
    output_amount: u64,
    input_transfer_fee: u64,
    output_transfer_fee: u64,
    lp_fee: u64,
    protocol_fee: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct SolanaSwapObsPreview {
    amount0: f64,
    amount1: f64,
    sqrt_price_x96: f64,
    liquidity: f64,
    tick: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct SolanaSignatureInfo {
    signature: String,
    slot: u64,
    #[serde(rename = "blockTime")]
    block_time: Option<i64>,
    err: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
struct SolanaProgramSwapInvocation {
    instruction: String,
    program_data_base64: Vec<String>,
}

fn serialize_u128_as_string<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

async fn sample_solana_pool_swaps(args: SampleSolanaPoolSwapsArgs) -> Result<()> {
    let rpc = SolanaLightRpc::new(args.rpc_url.clone());
    let mut samples = Vec::new();
    let mut scanned_signatures = 0usize;
    let mut transaction_errors = 0usize;
    let mut cursor = args.before_signature.clone();
    let max_pages = args.max_signature_pages.max(1);
    'pages: for _ in 0..max_pages {
        let signatures = rpc
            .get_signatures_for_address(
                &args.pool_address,
                args.signature_scan_limit,
                cursor.as_deref(),
            )
            .await?;
        if signatures.is_empty() {
            break;
        }
        cursor = signatures.last().map(|sig| sig.signature.clone());
        for sig in signatures.into_iter().filter(|sig| sig.err.is_none()) {
            scanned_signatures += 1;
            let tx = match rpc.get_transaction(&sig.signature).await {
                Ok(tx) => tx,
                Err(_) => {
                    transaction_errors += 1;
                    continue;
                }
            };
            if let Some(sample) = parse_solana_pool_swap_sample(&args, &sig, &tx) {
                samples.push(sample);
            }
            if scanned_signatures % 50 == 0 {
                eprintln!(
                    "sample progress scanned={} kept={} normalized={} tx_errors={}",
                    scanned_signatures,
                    samples.len(),
                    normalized_preview_count(&samples),
                    transaction_errors
                );
            }
            if sample_goal_reached(&args, &samples) {
                break 'pages;
            }
            if args.request_sleep_ms > 0 {
                tokio::time::sleep(Duration::from_millis(args.request_sleep_ms)).await;
            }
        }
        if sample_goal_reached(&args, &samples) {
            break;
        }
        if args.request_sleep_ms > 0 {
            tokio::time::sleep(Duration::from_millis(args.request_sleep_ms)).await;
        }
    }

    if let Some(output) = args.output.as_ref() {
        if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating output directory {}", parent.display()))?;
        }
        fs::write(output, serde_json::to_string_pretty(&samples)?)
            .with_context(|| format!("writing Solana pool swap samples {}", output.display()))?;
    }
    let normalized_rows = if let Some(output) = args.normalized_output.as_ref() {
        Some(write_normalized_swap_jsonl(output, &samples)?)
    } else {
        None
    };

    if args.format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&samples)?);
        return Ok(());
    }

    println!(
        "solana pool swap samples pool={} program={} scanned={} kept={} tx_errors={}",
        args.pool_address,
        args.program_id,
        scanned_signatures,
        samples.len(),
        transaction_errors
    );
    if let Some(last_sample) = samples.last() {
        println!(
            "next_before_signature={} oldest_kept_slot={}",
            last_sample.signature, last_sample.slot
        );
    }
    if let Some(normalized_rows) = normalized_rows {
        println!(
            "normalized SwapObs rows written={} path={}",
            normalized_rows,
            args.normalized_output
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default()
        );
    }
    println!(
        "{:<12} {:>10} {:<8} {:>18} {:>18} {:>5} {:>8}  data",
        "signature", "slot", "ix", "delta0_raw", "delta1_raw", "dir", "tick"
    );
    for sample in &samples {
        let direction = if sample.token0_in {
            "0in"
        } else if sample.token1_in {
            "1in"
        } else {
            "-"
        };
        let tick = sample
            .normalized_swap_previews
            .first()
            .map(|preview| {
                if sample.normalized_swap_previews.len() > 1 {
                    format!("{}+", preview.tick)
                } else {
                    preview.tick.to_string()
                }
            })
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<12} {:>10} {:<8} {:>18} {:>18} {:>5} {:>8}  {}",
            &sample.signature[..sample.signature.len().min(12)],
            sample.slot,
            sample.instruction,
            sample.token0_pool_delta_raw,
            sample.token1_pool_delta_raw,
            direction,
            tick,
            sample.program_data_count
        );
    }

    Ok(())
}

fn normalized_preview_count(samples: &[SolanaPoolSwapSample]) -> usize {
    samples
        .iter()
        .map(|sample| sample.normalized_swap_previews.len())
        .sum()
}

fn sample_goal_reached(args: &SampleSolanaPoolSwapsArgs, samples: &[SolanaPoolSwapSample]) -> bool {
    if let Some(min_normalized_swaps) = args.min_normalized_swaps {
        return normalized_preview_count(samples) >= min_normalized_swaps;
    }
    samples.len() >= args.limit
}

fn write_normalized_swap_jsonl(path: &Path, samples: &[SolanaPoolSwapSample]) -> Result<usize> {
    let mut swaps = samples
        .iter()
        .enumerate()
        .flat_map(|(idx, sample)| sample_to_swap_obs(sample, idx as u64))
        .collect::<Vec<_>>();
    swaps.sort_by(|a, b| {
        a.block
            .cmp(&b.block)
            .then_with(|| a.log_index.cmp(&b.log_index))
    });
    for (idx, swap) in swaps.iter_mut().enumerate() {
        swap.log_index = idx as u64;
    }
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating output directory {}", parent.display()))?;
    }
    let mut file = File::create(path)
        .with_context(|| format!("creating normalized swaps {}", path.display()))?;
    for swap in &swaps {
        writeln!(file, "{}", serde_json::to_string(swap)?)
            .with_context(|| format!("writing normalized swaps {}", path.display()))?;
    }
    Ok(swaps.len())
}

fn sample_to_swap_obs(
    sample: &SolanaPoolSwapSample,
    log_index: u64,
) -> Vec<autopool_backtest::SwapObs> {
    sample
        .normalized_swap_previews
        .iter()
        .enumerate()
        .map(|(idx, preview)| autopool_backtest::SwapObs {
            block: sample.slot,
            log_index: log_index * 1_000 + idx as u64,
            amount0: preview.amount0,
            amount1: preview.amount1,
            sqrt_price_x96: preview.sqrt_price_x96,
            liquidity: preview.liquidity,
            tick: preview.tick,
        })
        .collect()
}

struct SolanaLightRpc {
    rpc_url: String,
    http: reqwest::Client,
}

impl SolanaLightRpc {
    fn new(rpc_url: String) -> Self {
        Self {
            rpc_url,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(SOLANA_HTTP_TIMEOUT_SECS))
                .build()
                .expect("SolanaLightRpc timeout config should be valid"),
        }
    }

    async fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 0..5 {
            match self.http.post(&self.rpc_url).json(&body).send().await {
                Ok(response) if response.status().is_success() => {
                    let response = response.json::<serde_json::Value>().await?;
                    if let Some(error) = response.get("error") {
                        anyhow::bail!("Solana RPC {method} error: {error}");
                    }
                    return response
                        .get("result")
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("Solana RPC {method} missing result"));
                }
                Ok(response) => {
                    last_error = Some(anyhow::anyhow!(
                        "Solana RPC {method} HTTP status {}",
                        response.status()
                    ));
                }
                Err(error) => {
                    last_error = Some(anyhow::Error::new(error));
                }
            }
            tokio::time::sleep(Duration::from_millis(250 * (attempt + 1) as u64)).await;
        }
        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("Solana RPC {method} failed without response")))
    }

    async fn get_signatures_for_address(
        &self,
        address: &str,
        limit: usize,
        before: Option<&str>,
    ) -> Result<Vec<SolanaSignatureInfo>> {
        let mut config = json!({"limit": limit.min(1_000)});
        if let Some(before) = before {
            config["before"] = json!(before);
        }
        let result = self
            .call("getSignaturesForAddress", json!([address, config]))
            .await?;
        serde_json::from_value(result).context("parsing getSignaturesForAddress result")
    }

    async fn get_transaction(&self, signature: &str) -> Result<serde_json::Value> {
        self.call(
            "getTransaction",
            json!([signature, {"encoding": "jsonParsed", "maxSupportedTransactionVersion": 0}]),
        )
        .await
    }
}

fn parse_solana_pool_swap_sample(
    args: &SampleSolanaPoolSwapsArgs,
    sig: &SolanaSignatureInfo,
    tx: &serde_json::Value,
) -> Option<SolanaPoolSwapSample> {
    let logs = tx
        .get("meta")?
        .get("logMessages")?
        .as_array()?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    let invocations = target_program_swap_invocations(&logs, &args.program_id);
    if invocations.is_empty() {
        return None;
    }
    let (token0_pool_delta_raw, token0_decimals) =
        pool_token_delta(tx, &args.pool_address, &args.token0_mint);
    let (token1_pool_delta_raw, token1_decimals) =
        pool_token_delta(tx, &args.pool_address, &args.token1_mint);
    if token0_pool_delta_raw == 0 && token1_pool_delta_raw == 0 {
        return None;
    }
    let raydium_matches = if args.program_id == RAYDIUM_CLMM_PROGRAM_ID {
        matching_raydium_clmm_swap_events(&invocations, &args.pool_address)
    } else {
        Vec::new()
    };
    let orca_matches = if args.program_id == ORCA_WHIRLPOOL_PROGRAM_ID {
        matching_orca_whirlpool_traded_events(&invocations, &args.pool_address)
    } else {
        Vec::new()
    };
    let instruction = raydium_matches
        .first()
        .map(|(instruction, _, _)| instruction.clone())
        .or_else(|| {
            orca_matches
                .first()
                .map(|(instruction, _, _)| instruction.clone())
        })
        .unwrap_or_else(|| invocations[0].instruction.clone());
    let program_data_base64 = if !raydium_matches.is_empty() {
        raydium_matches
            .iter()
            .map(|(_, data, _)| data.clone())
            .collect::<Vec<_>>()
    } else if !orca_matches.is_empty() {
        orca_matches
            .iter()
            .map(|(_, data, _)| data.clone())
            .collect::<Vec<_>>()
    } else {
        invocations
            .iter()
            .flat_map(|invocation| invocation.program_data_base64.iter().cloned())
            .collect::<Vec<_>>()
    };
    let raydium_clmm_events = raydium_matches
        .into_iter()
        .map(|(_, _, event)| event)
        .collect::<Vec<_>>();
    let orca_whirlpool_events = orca_matches
        .into_iter()
        .map(|(_, _, event)| event)
        .collect::<Vec<_>>();
    let mut normalized_swap_previews = raydium_clmm_events
        .iter()
        .map(raydium_event_to_swap_obs_preview)
        .collect::<Vec<_>>();
    if let Some(active_liquidity) = args.active_liquidity {
        normalized_swap_previews.extend(
            orca_whirlpool_events
                .iter()
                .map(|event| orca_event_to_swap_obs_preview(event, active_liquidity)),
        );
    }

    Some(SolanaPoolSwapSample {
        signature: sig.signature.clone(),
        slot: sig.slot,
        block_time: sig.block_time,
        instruction,
        token0_mint: args.token0_mint.clone(),
        token1_mint: args.token1_mint.clone(),
        token0_pool_delta_raw,
        token1_pool_delta_raw,
        token0_decimals,
        token1_decimals,
        token0_in: token0_pool_delta_raw > 0,
        token1_in: token1_pool_delta_raw > 0,
        program_data_count: program_data_base64.len(),
        program_data_base64,
        raydium_clmm_events,
        orca_whirlpool_events,
        normalized_swap_previews,
    })
}

fn decode_raydium_clmm_swap_event(
    program_data_base64: &str,
    expected_pool_address: &str,
) -> Option<RaydiumClmmSwapEvent> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(program_data_base64)
        .ok()?;
    if bytes.len() < RAYDIUM_CLMM_SWAP_EVENT_LEN {
        return None;
    }
    if bytes.get(0..8)? != RAYDIUM_CLMM_SWAP_EVENT_DISCRIMINATOR {
        return None;
    }
    let mut offset = 8; // Anchor event discriminator.
    let pool_state = read_pubkey_base58(&bytes, &mut offset)?;
    let sender = read_pubkey_base58(&bytes, &mut offset)?;
    let token_account_0 = read_pubkey_base58(&bytes, &mut offset)?;
    let token_account_1 = read_pubkey_base58(&bytes, &mut offset)?;
    let amount_0 = read_u64_le(&bytes, &mut offset)?;
    let transfer_fee_0 = read_u64_le(&bytes, &mut offset)?;
    let amount_1 = read_u64_le(&bytes, &mut offset)?;
    let transfer_fee_1 = read_u64_le(&bytes, &mut offset)?;
    let zero_for_one = read_bool(&bytes, &mut offset)?;
    let sqrt_price_x64 = read_u128_le(&bytes, &mut offset)?;
    let liquidity = read_u128_le(&bytes, &mut offset)?;
    let tick = read_i32_le(&bytes, &mut offset)?;
    let trade_fee_0 = read_u64_le(&bytes, &mut offset)?;
    let trade_fee_1 = read_u64_le(&bytes, &mut offset)?;
    if pool_state != expected_pool_address {
        return None;
    }
    Some(RaydiumClmmSwapEvent {
        pool_state,
        sender,
        token_account_0,
        token_account_1,
        amount_0,
        transfer_fee_0,
        amount_1,
        transfer_fee_1,
        zero_for_one,
        sqrt_price_x64,
        liquidity,
        tick,
        trade_fee_0,
        trade_fee_1,
    })
}

fn matching_raydium_clmm_swap_events(
    invocations: &[SolanaProgramSwapInvocation],
    expected_pool_address: &str,
) -> Vec<(String, String, RaydiumClmmSwapEvent)> {
    invocations
        .iter()
        .flat_map(|invocation| {
            invocation.program_data_base64.iter().filter_map(|data| {
                decode_raydium_clmm_swap_event(data, expected_pool_address)
                    .map(|event| (invocation.instruction.clone(), data.clone(), event))
            })
        })
        .collect()
}

fn decode_orca_whirlpool_traded_event(
    program_data_base64: &str,
    expected_pool_address: &str,
) -> Option<OrcaWhirlpoolTradedEvent> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(program_data_base64)
        .ok()?;
    if bytes.len() < ORCA_WHIRLPOOL_TRADED_EVENT_LEN {
        return None;
    }
    if bytes.get(0..8)? != ORCA_WHIRLPOOL_TRADED_EVENT_DISCRIMINATOR {
        return None;
    }
    let mut offset = 8; // Anchor event discriminator.
    let whirlpool = read_pubkey_base58(&bytes, &mut offset)?;
    let a_to_b = read_bool(&bytes, &mut offset)?;
    let pre_sqrt_price = read_u128_le(&bytes, &mut offset)?;
    let post_sqrt_price = read_u128_le(&bytes, &mut offset)?;
    let input_amount = read_u64_le(&bytes, &mut offset)?;
    let output_amount = read_u64_le(&bytes, &mut offset)?;
    let input_transfer_fee = read_u64_le(&bytes, &mut offset)?;
    let output_transfer_fee = read_u64_le(&bytes, &mut offset)?;
    let lp_fee = read_u64_le(&bytes, &mut offset)?;
    let protocol_fee = read_u64_le(&bytes, &mut offset)?;
    if whirlpool != expected_pool_address {
        return None;
    }
    Some(OrcaWhirlpoolTradedEvent {
        whirlpool,
        a_to_b,
        pre_sqrt_price,
        post_sqrt_price,
        input_amount,
        output_amount,
        input_transfer_fee,
        output_transfer_fee,
        lp_fee,
        protocol_fee,
    })
}

fn matching_orca_whirlpool_traded_events(
    invocations: &[SolanaProgramSwapInvocation],
    expected_pool_address: &str,
) -> Vec<(String, String, OrcaWhirlpoolTradedEvent)> {
    invocations
        .iter()
        .flat_map(|invocation| {
            invocation.program_data_base64.iter().filter_map(|data| {
                decode_orca_whirlpool_traded_event(data, expected_pool_address)
                    .map(|event| (invocation.instruction.clone(), data.clone(), event))
            })
        })
        .collect()
}

fn raydium_event_to_swap_obs_preview(event: &RaydiumClmmSwapEvent) -> SolanaSwapObsPreview {
    let amount0 = if event.zero_for_one {
        event.amount_0 as f64
    } else {
        -(event.amount_0 as f64)
    };
    let amount1 = if event.zero_for_one {
        -(event.amount_1 as f64)
    } else {
        event.amount_1 as f64
    };
    SolanaSwapObsPreview {
        amount0,
        amount1,
        sqrt_price_x96: event.sqrt_price_x64 as f64 * TWO_POW_32_F64,
        liquidity: event.liquidity as f64,
        tick: event.tick,
    }
}

fn orca_event_to_swap_obs_preview(
    event: &OrcaWhirlpoolTradedEvent,
    active_liquidity: f64,
) -> SolanaSwapObsPreview {
    let amount0 = if event.a_to_b {
        event.input_amount as f64
    } else {
        -(event.output_amount as f64)
    };
    let amount1 = if event.a_to_b {
        -(event.output_amount as f64)
    } else {
        event.input_amount as f64
    };
    SolanaSwapObsPreview {
        amount0,
        amount1,
        sqrt_price_x96: event.post_sqrt_price as f64 * TWO_POW_32_F64,
        liquidity: active_liquidity,
        tick: sqrt_price_x64_to_tick(event.post_sqrt_price),
    }
}

fn sqrt_price_x64_to_tick(sqrt_price_x64: u128) -> i32 {
    let sqrt_price = sqrt_price_x64 as f64 / TWO_POW_64_F64;
    if sqrt_price <= 0.0 {
        return 0;
    }
    let price = sqrt_price * sqrt_price;
    (price.ln() / 1.0001_f64.ln()).floor() as i32
}

fn read_bytes<'a>(bytes: &'a [u8], offset: &mut usize, len: usize) -> Option<&'a [u8]> {
    let end = offset.checked_add(len)?;
    let slice = bytes.get(*offset..end)?;
    *offset = end;
    Some(slice)
}

fn read_pubkey_base58(bytes: &[u8], offset: &mut usize) -> Option<String> {
    Some(bs58::encode(read_bytes(bytes, offset, 32)?).into_string())
}

fn read_u64_le(bytes: &[u8], offset: &mut usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        read_bytes(bytes, offset, 8)?.try_into().ok()?,
    ))
}

fn read_u128_le(bytes: &[u8], offset: &mut usize) -> Option<u128> {
    Some(u128::from_le_bytes(
        read_bytes(bytes, offset, 16)?.try_into().ok()?,
    ))
}

fn read_i32_le(bytes: &[u8], offset: &mut usize) -> Option<i32> {
    Some(i32::from_le_bytes(
        read_bytes(bytes, offset, 4)?.try_into().ok()?,
    ))
}

fn read_bool(bytes: &[u8], offset: &mut usize) -> Option<bool> {
    match read_bytes(bytes, offset, 1)?[0] {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

fn target_program_swap_invocations(
    logs: &[String],
    program_id: &str,
) -> Vec<SolanaProgramSwapInvocation> {
    let mut in_target = false;
    let mut instruction: Option<String> = None;
    let mut data = Vec::new();
    let mut invocations = Vec::new();
    let invoke_prefix = format!("Program {program_id} invoke");
    let success_prefix = format!("Program {program_id} success");
    let failed_prefix = format!("Program {program_id} failed");
    for log in logs {
        if log.starts_with(&invoke_prefix) {
            in_target = true;
            instruction = None;
            data.clear();
            continue;
        }
        if in_target && log.starts_with("Program log: Instruction: ") {
            let ix = log
                .trim_start_matches("Program log: Instruction: ")
                .to_string();
            if ix.to_ascii_lowercase().contains("swap") {
                instruction = Some(ix);
            }
            continue;
        }
        if in_target && log.starts_with("Program data: ") {
            data.push(log.trim_start_matches("Program data: ").to_string());
            continue;
        }
        if in_target && (log.starts_with(&success_prefix) || log.starts_with(&failed_prefix)) {
            if let Some(ix) = instruction.clone() {
                invocations.push(SolanaProgramSwapInvocation {
                    instruction: ix,
                    program_data_base64: data.clone(),
                });
            }
            in_target = false;
            data.clear();
        }
    }
    invocations
}

fn pool_token_delta(tx: &serde_json::Value, pool_address: &str, mint: &str) -> (i128, Option<u8>) {
    let pre = token_amount_by_account(tx, "preTokenBalances", pool_address, mint);
    let post = token_amount_by_account(tx, "postTokenBalances", pool_address, mint);
    let decimals = post
        .iter()
        .chain(pre.iter())
        .find_map(|(_, decimals)| *decimals);
    let pre_sum = pre.iter().map(|(amount, _)| *amount).sum::<i128>();
    let post_sum = post.iter().map(|(amount, _)| *amount).sum::<i128>();
    (post_sum - pre_sum, decimals)
}

fn token_amount_by_account(
    tx: &serde_json::Value,
    field: &str,
    pool_address: &str,
    mint: &str,
) -> Vec<(i128, Option<u8>)> {
    tx.get("meta")
        .and_then(|meta| meta.get(field))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|balance| {
            balance.get("owner").and_then(serde_json::Value::as_str) == Some(pool_address)
                && balance.get("mint").and_then(serde_json::Value::as_str) == Some(mint)
        })
        .filter_map(|balance| {
            let amount = balance
                .get("uiTokenAmount")?
                .get("amount")?
                .as_str()?
                .parse::<i128>()
                .ok()?;
            let decimals = balance
                .get("uiTokenAmount")
                .and_then(|value| value.get("decimals"))
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u8::try_from(value).ok());
            Some((amount, decimals))
        })
        .collect()
}

fn hot_pool_experiment_plan(args: HotPoolExperimentArgs) -> Result<()> {
    let input = fs::read_to_string(&args.input)
        .with_context(|| format!("reading hot-pool candidates {}", args.input.display()))?;
    let mut candidates = serde_json::from_str::<Vec<HotPoolCandidateRow>>(&input)
        .with_context(|| format!("parsing hot-pool candidates {}", args.input.display()))?;
    if !args.include_p2 {
        candidates.retain(|row| !row.experiment_priority.starts_with("P2"));
    }

    let mut rows = Vec::new();
    for candidate in candidates.into_iter().take(args.limit) {
        rows.push(hot_pool_experiment_row(&candidate, &args)?);
    }

    if let Some(output) = args.output.as_ref() {
        if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating output directory {}", parent.display()))?;
        }
        fs::write(output, serde_json::to_string_pretty(&rows)?)
            .with_context(|| format!("writing hot-pool experiment plan {}", output.display()))?;
    }

    if args.write_specs {
        let spec_dir = args.data_dir.join("specs");
        fs::create_dir_all(&spec_dir)
            .with_context(|| format!("creating spec directory {}", spec_dir.display()))?;
        for row in &rows {
            fs::write(
                Path::new(&row.spec_path),
                serde_json::to_string_pretty(&row.spec)?,
            )
            .with_context(|| format!("writing replay spec {}", row.spec_path))?;
        }
    }

    if args.format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    println!(
        "hot-pool experiment plan input={} planned={} include_p2={} capital=${:.0}",
        args.input.display(),
        rows.len(),
        args.include_p2,
        args.capital_usd
    );
    println!(
        "{:<8} {:<22} {:<18} {:<31} {:>6} {:>8} {:<22}  next_step",
        "venue", "symbol", "priority", "status", "swaps", "feeAPR", "model"
    );
    for row in &rows {
        println!(
            "{:<8} {:<22} {:<18} {:<31} {:>6} {:>7.1}% {:<22}  {}",
            row.venue,
            row.symbol,
            row.priority,
            row.status,
            row.normalized_swaps,
            row.candidate_fee_apr_24h,
            row.replay_model,
            row.next_step
        );
    }

    Ok(())
}

fn hot_pool_experiment_row(
    candidate: &HotPoolCandidateRow,
    args: &HotPoolExperimentArgs,
) -> Result<HotPoolExperimentRow> {
    let experiment_id = hot_pool_experiment_id(candidate);
    let normalized_swaps_path = args
        .data_dir
        .join("swaps")
        .join(candidate.pool_address.to_ascii_lowercase())
        .join("swaps.jsonl");
    let spec_path = args
        .data_dir
        .join("specs")
        .join(format!("{experiment_id}.json"));
    let normalized_swaps = count_nonempty_lines_if_exists(&normalized_swaps_path)?;
    let replay_model = hot_pool_replay_model(candidate);
    let spec = replay_spec_from_candidate(candidate, &replay_model);
    let reject_reasons = hot_pool_reject_reasons(candidate);
    let data_requirements =
        hot_pool_data_requirements(candidate, &replay_model, normalized_swaps, &reject_reasons);
    let status = hot_pool_experiment_status(
        candidate,
        &replay_model,
        normalized_swaps,
        &data_requirements,
        &reject_reasons,
    );
    let next_step = hot_pool_next_step(&status);
    let replay_command = (status == "ready_for_replay").then(|| {
        format!(
            "cargo run -p autopool-cli -- replay-normalized-swaps --spec {} --swaps {} --capital-usd {:.0} --narrow-half-width {} --wide-half-width {}",
            spec_path.display(),
            normalized_swaps_path.display(),
            args.capital_usd,
            args.narrow_half_width,
            args.wide_half_width
        )
    });

    Ok(HotPoolExperimentRow {
        experiment_id,
        venue: candidate.venue.clone(),
        symbol: candidate.symbol.clone(),
        pool_address: candidate.pool_address.clone(),
        priority: candidate.experiment_priority.clone(),
        status,
        next_step,
        replay_model,
        normalized_swaps_path: normalized_swaps_path.display().to_string(),
        spec_path: spec_path.display().to_string(),
        normalized_swaps,
        candidate_fee_apr_24h: candidate.fee_apr_24h,
        formula_fee_apr_24h: candidate.formula_fee_apr_24h,
        volume_tvl_24h: candidate.volume_tvl_24h,
        price_range_24h_pct: candidate.price_range_24h_pct,
        price_change_24h_pct: candidate.price_change_24h_pct,
        target_progress: candidate.target_progress,
        capital_usd: args.capital_usd,
        baseline_policies: hot_pool_baseline_policies(),
        data_requirements,
        reject_reasons,
        promotion_gates: HotPoolPromotionGates {
            min_windows: 5,
            min_win_rate_vs_hold_and_wide: 0.70,
            require_positive_fee_minus_lvr: true,
            max_drawdown_usd: args.capital_usd * args.max_drawdown_pct,
            max_rebalances_per_day: args.max_rebalances_per_day,
            require_capacity_check: true,
            require_api_cross_check: true,
        },
        replay_command,
        spec,
    })
}

fn replay_spec_from_candidate(
    candidate: &HotPoolCandidateRow,
    replay_model: &str,
) -> ReplayPoolSpec {
    let stable_index = stable_token_index(&candidate.tokens);
    let invert_for_numeraire = stable_index == Some(1);
    ReplayPoolSpec {
        chain: "solana".to_string(),
        venue: candidate.venue.clone(),
        pool_kind: candidate.pool_kind.clone(),
        pool_address: candidate.pool_address.clone(),
        symbol: candidate.symbol.clone(),
        token0: candidate.tokens.first().cloned(),
        token1: candidate.tokens.get(1).cloned(),
        fee_bps: candidate.fee_bps.unwrap_or_default(),
        tick_spacing: candidate.tick_spacing,
        bin_step: candidate.bin_step,
        replay_model: replay_model.to_string(),
        invert_for_numeraire,
        token0_usd: stable_index.map(|_| 1.0),
        block_seconds: 0.4,
        risk_token_side: RiskTokenSide::Token1.label().to_string(),
        reward_apr_fraction: candidate.reward_apr_pct.map(|pct| pct / 100.0),
        active_liquidity: candidate.active_liquidity.clone(),
        source_priority: candidate.experiment_priority.clone(),
        source_warnings: candidate.warnings.clone(),
    }
}

fn hot_pool_replay_model(candidate: &HotPoolCandidateRow) -> String {
    let venue = candidate.venue.to_ascii_lowercase();
    let kind = candidate.pool_kind.to_ascii_lowercase();
    if venue == "meteora" || kind.contains("dlmm") || candidate.bin_step.is_some() {
        "dlmm_bin_replay".to_string()
    } else {
        "clmm_tick_replay".to_string()
    }
}

fn hot_pool_reject_reasons(candidate: &HotPoolCandidateRow) -> Vec<String> {
    let mut reasons = Vec::new();
    for warning in &candidate.warnings {
        match warning.as_str() {
            "fee_apr_formula_mismatch" => reasons.push(
                "reported APR fails fee*turnover sanity check; verify provider math first"
                    .to_string(),
            ),
            "fee_apr_outlier" => {
                reasons.push("reported APR is an outlier; cross-check before replay".to_string())
            }
            "unverified_or_warning" => {
                reasons.push("protocol row is unverified or carries API warnings".to_string())
            }
            _ => {}
        }
    }
    reasons.sort();
    reasons.dedup();
    reasons
}

fn hot_pool_data_requirements(
    candidate: &HotPoolCandidateRow,
    replay_model: &str,
    normalized_swaps: usize,
    reject_reasons: &[String],
) -> Vec<String> {
    let mut requirements = Vec::new();
    if !reject_reasons.is_empty() {
        requirements.push("independent_api_cross_check".to_string());
    }
    if candidate.tokens.len() < 2
        || candidate
            .tokens
            .iter()
            .any(|token| token.decimals.is_none())
    {
        requirements.push("token_mints_and_decimals".to_string());
    }
    if candidate.fee_bps.unwrap_or_default() <= 0.0 {
        requirements.push("fee_tier_bps".to_string());
    }
    if replay_model == "dlmm_bin_replay" {
        requirements.push("dlmm_bin_replay_engine".to_string());
        requirements.push("bin_liquidity_snapshots".to_string());
        requirements.push("dlmm_swap_decoder".to_string());
    } else {
        if candidate.tick_spacing.is_none() {
            requirements.push("tick_spacing".to_string());
        }
        if normalized_swaps == 0 {
            requirements.push("normalized_clmm_swap_stream".to_string());
        }
        requirements.push("active_liquidity_per_swap_or_snapshot".to_string());
    }
    requirements.sort();
    requirements.dedup();
    requirements
}

fn hot_pool_experiment_status(
    candidate: &HotPoolCandidateRow,
    replay_model: &str,
    normalized_swaps: usize,
    data_requirements: &[String],
    reject_reasons: &[String],
) -> String {
    if candidate.experiment_priority.starts_with("P2") || !reject_reasons.is_empty() {
        return "blocked_api_validation".to_string();
    }
    if replay_model == "dlmm_bin_replay" {
        return "blocked_needs_bin_replay".to_string();
    }
    if data_requirements.iter().any(|requirement| {
        matches!(
            requirement.as_str(),
            "token_mints_and_decimals" | "fee_tier_bps" | "tick_spacing"
        )
    }) {
        return "blocked_missing_replay_metadata".to_string();
    }
    if normalized_swaps == 0 {
        return "blocked_missing_normalized_swaps".to_string();
    }
    "ready_for_replay".to_string()
}

fn hot_pool_next_step(status: &str) -> String {
    match status {
        "blocked_api_validation" => "verify_provider_apr_and_fee_math".to_string(),
        "blocked_needs_bin_replay" => "implement_dlmm_bin_replay_adapter".to_string(),
        "blocked_missing_replay_metadata" => "refresh_protocol_state_metadata".to_string(),
        "blocked_missing_normalized_swaps" => "collect_and_decode_clmm_swaps".to_string(),
        "ready_for_replay" => "run_baseline_replay_battery".to_string(),
        _ => "investigate".to_string(),
    }
}

fn hot_pool_baseline_policies() -> Vec<String> {
    [
        "hold_50_50",
        "passive_wide",
        "narrow_static",
        "narrow_rebalance",
        "vol_scaled_rebalance",
        "hard_exit_stop",
        "hedged_narrow",
        "adaptive_regime",
    ]
    .iter()
    .map(|policy| (*policy).to_string())
    .collect()
}

fn hot_pool_experiment_id(candidate: &HotPoolCandidateRow) -> String {
    let address = candidate.pool_address.trim_start_matches("0x");
    let prefix = &address[..address.len().min(8)];
    format!(
        "{}-{}-{}",
        candidate.venue.to_ascii_lowercase(),
        symbol_key(&candidate.symbol).to_ascii_lowercase(),
        prefix.to_ascii_lowercase()
    )
}

fn stable_token_index(tokens: &[HotPoolTokenRow]) -> Option<usize> {
    tokens
        .iter()
        .position(|token| is_stable_symbol(&token.symbol))
}

fn is_stable_symbol(symbol: &str) -> bool {
    matches!(
        symbol.to_ascii_uppercase().as_str(),
        "USDC" | "USDT" | "USDG" | "USDS" | "PYUSD" | "FDUSD" | "DAI"
    )
}

fn count_nonempty_lines_if_exists(path: &Path) -> Result<usize> {
    if !path.exists() {
        return Ok(0);
    }
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut lines = 0usize;
    for line in reader.lines() {
        if !line?.trim().is_empty() {
            lines += 1;
        }
    }
    Ok(lines)
}

impl NormalizedReplayParams {
    fn to_replay_params(&self, spec: &ReplayPoolSpec) -> Result<ReplayParams> {
        if spec.fee_bps <= 0.0 {
            anyhow::bail!(
                "replay spec {} has invalid fee_bps={}",
                spec.symbol,
                spec.fee_bps
            );
        }
        let natural_decimals0 = spec
            .token0
            .as_ref()
            .and_then(|token| token.decimals)
            .with_context(|| format!("replay spec {} missing token0 decimals", spec.symbol))?;
        let natural_decimals1 = spec
            .token1
            .as_ref()
            .and_then(|token| token.decimals)
            .with_context(|| format!("replay spec {} missing token1 decimals", spec.symbol))?;
        let (decimals0, decimals1) = if spec.invert_for_numeraire {
            (natural_decimals1, natural_decimals0)
        } else {
            (natural_decimals0, natural_decimals1)
        };
        let token0_usd = self.token0_usd.or(spec.token0_usd).with_context(|| {
            format!(
                "replay spec {} has no token0_usd anchor; pass --token0-usd",
                spec.symbol
            )
        })?;
        let risk_token_side = self
            .risk_token_side
            .unwrap_or_else(|| risk_token_side_from_label(&spec.risk_token_side));

        Ok(ReplayParams {
            fee_bps: spec.fee_bps,
            decimals0,
            decimals1,
            token0_usd,
            capital_usd: self.capital_usd,
            rebalance_gas_usd: self.rebalance_gas_usd,
            rebalance_slippage_bps: self.rebalance_slippage_bps,
            narrow_half_width: self.narrow_half_width,
            wide_half_width: self.wide_half_width,
            vol_k: self.vol_k,
            action_delay_blocks: self.action_delay_blocks,
            block_seconds: self.block_seconds.unwrap_or(spec.block_seconds),
            risk_token_side,
            funding_bps_per_day: self.funding_bps_per_day,
            hedge_fraction: self.hedge_fraction,
            trend_exit_threshold: self.trend_exit_threshold,
            reward_apr: self
                .reward_apr
                .or(spec.reward_apr_fraction)
                .unwrap_or_default(),
            reward_haircut: self.reward_haircut,
            invert: spec.invert_for_numeraire,
        })
    }
}

fn risk_token_side_from_label(label: &str) -> RiskTokenSide {
    if label.eq_ignore_ascii_case("token0") {
        RiskTokenSide::Token0
    } else {
        RiskTokenSide::Token1
    }
}

fn read_replay_pool_spec(path: &Path) -> Result<ReplayPoolSpec> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("reading replay spec {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parsing replay spec {}", path.display()))
}

fn load_normalized_swaps(path: &Path) -> Result<Vec<autopool_backtest::SwapObs>> {
    let file = File::open(path)
        .with_context(|| format!("opening normalized swap stream {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut swaps = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let swap =
            serde_json::from_str::<autopool_backtest::SwapObs>(&line).with_context(|| {
                format!(
                    "failed to parse normalized swap {} line {}",
                    path.display(),
                    index + 1
                )
            })?;
        swaps.push(swap);
    }
    swaps.sort_by(|left, right| {
        left.block
            .cmp(&right.block)
            .then_with(|| left.log_index.cmp(&right.log_index))
    });
    Ok(swaps)
}

fn solana_spacing_label(row: &SolanaPoolCandidate) -> String {
    if let Some(tick_spacing) = row.tick_spacing {
        return format!("tick:{tick_spacing}");
    }
    if let Some(bin_step) = row.bin_step {
        return format!("bin:{bin_step}");
    }
    "-".to_string()
}

fn format_opt_pct(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.1}%"))
        .unwrap_or_else(|| "-".to_string())
}

fn format_opt_f64(value: Option<f64>, decimals: usize) -> String {
    value
        .map(|value| format!("{value:.prec$}", prec = decimals))
        .unwrap_or_else(|| "-".to_string())
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

#[derive(Debug, Serialize)]
struct NormalizedWindowReplayReport {
    label: String,
    swaps: usize,
    window_swaps: usize,
    step_swaps: usize,
    windows: usize,
    config: serde_json::Value,
    summaries: Vec<WindowPolicySummary>,
    rows: Vec<WindowPolicyRow>,
}

#[derive(Debug, Clone, Serialize)]
struct WindowPolicyRow {
    window_index: usize,
    swap_start: usize,
    swap_end_exclusive: usize,
    swaps: usize,
    block_first: u64,
    block_last: u64,
    tick_first: i32,
    tick_last: i32,
    tick_delta: i32,
    trend_strength: f64,
    regime: String,
    minutes: f64,
    policy: String,
    net_pnl_usd: f64,
    net_vs_hold_usd: f64,
    fee_minus_lvr_usd: f64,
    net_apr_pct: Option<f64>,
    fee_lvr_apr_pct: Option<f64>,
    max_drawdown_usd: f64,
    rebalances: u32,
}

#[derive(Debug, Serialize)]
struct WindowPolicySummary {
    policy: String,
    windows: usize,
    win_rate_vs_hold_pct: f64,
    mean_net_pnl_usd: f64,
    mean_net_vs_hold_usd: f64,
    mean_fee_minus_lvr_usd: f64,
    mean_net_apr_pct: Option<f64>,
    p05_net_apr_pct: Option<f64>,
    p50_net_apr_pct: Option<f64>,
    p95_net_apr_pct: Option<f64>,
    mean_fee_lvr_apr_pct: Option<f64>,
    worst_max_drawdown_usd: f64,
    mean_rebalances: f64,
}

#[derive(Debug, Serialize)]
struct HedgeGridReport {
    label: String,
    swaps: usize,
    window_swaps: usize,
    step_swaps: usize,
    windows: usize,
    config: serde_json::Value,
    rows: Vec<HedgeGridRow>,
    regime_rows: Vec<HedgeGridRegimeRow>,
    rule_rows: Vec<HedgeGridRuleRow>,
}

#[derive(Debug, Serialize)]
struct HedgeGridRow {
    hedge_fraction: f64,
    policy: String,
    windows: usize,
    win_rate_vs_hold_pct: f64,
    mean_net_pnl_usd: f64,
    mean_net_vs_hold_usd: f64,
    mean_net_apr_pct: Option<f64>,
    p05_net_apr_pct: Option<f64>,
    mean_fee_lvr_apr_pct: Option<f64>,
    worst_max_drawdown_usd: f64,
    score: Option<f64>,
}

#[derive(Debug, Serialize)]
struct HedgeGridRegimeRow {
    hedge_fraction: f64,
    policy: String,
    regime: String,
    windows: usize,
    win_rate_vs_hold_pct: f64,
    mean_net_vs_hold_usd: f64,
    p05_net_apr_pct: Option<f64>,
    worst_max_drawdown_usd: f64,
}

#[derive(Debug, Serialize)]
struct HedgeGridRuleRow {
    rule: String,
    hedge_map: String,
    windows: usize,
    skipped_windows: usize,
    win_rate_vs_hold_pct: f64,
    mean_net_pnl_usd: f64,
    mean_net_vs_hold_usd: f64,
    mean_net_apr_pct: Option<f64>,
    p05_net_apr_pct: Option<f64>,
    worst_max_drawdown_usd: f64,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct PromotionGateThresholds {
    min_p05_net_apr_pct: f64,
    min_mean_vs_hold_usd: f64,
    min_win_rate_vs_hold_pct: f64,
    max_drawdown_pct: f64,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct PromotionWindowConfig {
    window_swaps: usize,
    step_swaps: usize,
    min_windows: usize,
}

#[derive(Debug, Serialize)]
struct PromotionGateReport {
    label: String,
    swaps: usize,
    policy: String,
    hedge_map: String,
    thresholds: PromotionGateThresholds,
    max_worst_drawdown_usd: f64,
    verdict: String,
    reasons: Vec<String>,
    rows: Vec<PromotionGateWindowRow>,
}

#[derive(Debug, Serialize)]
struct PromotionGateWindowRow {
    window_swaps: usize,
    step_swaps: usize,
    min_windows: usize,
    windows: usize,
    skipped_windows: usize,
    win_rate_vs_hold_pct: f64,
    mean_net_pnl_usd: f64,
    mean_net_vs_hold_usd: f64,
    mean_net_apr_pct: Option<f64>,
    p05_net_apr_pct: Option<f64>,
    worst_max_drawdown_usd: f64,
    passed: bool,
    reasons: Vec<String>,
}

#[derive(Debug, Serialize)]
struct MergeNormalizedSwapsReport {
    inputs: Vec<String>,
    output: String,
    input_rows: usize,
    unique_rows: usize,
    duplicate_rows: usize,
    block_first: Option<u64>,
    block_last: Option<u64>,
    tick_first: Option<i32>,
    tick_last: Option<i32>,
}

#[derive(Debug, Clone)]
struct WindowRegimeLabel {
    tick_first: i32,
    tick_last: i32,
    tick_delta: i32,
    trend_strength: f64,
    label: String,
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
        "block_seconds": params.block_seconds,
        "risk_token_side": params.risk_token_side.label(),
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
    let window_years = replay_window_years(&report, params);
    if let Some(years) = window_years {
        println!(
            "window: {:.1}min; APR columns are mechanical window annualization, not a forecast",
            years * 365.0 * 24.0 * 60.0
        );
    }
    println!(
        "config: fee={:.2}bps capital=${:.0} token0=${:.0} gas=${:.3}/reb delay={}blk block_s={:.2} risk={} fund={}bps/d hedge={} narrow=±{} wide=±{}",
        params.fee_bps,
        params.capital_usd,
        params.token0_usd,
        params.rebalance_gas_usd,
        params.action_delay_blocks,
        params.block_seconds,
        params.risk_token_side.label(),
        params.funding_bps_per_day,
        params.hedge_fraction,
        params.narrow_half_width,
        params.wide_half_width,
    );
    println!(
        "{:<22} {:>10} {:>10} {:>9} {:>9} {:>9} {:>10} {:>9} {:>10} {:>9} {:>8}",
        "policy",
        "net_pnl",
        "vs_hold",
        "fees",
        "reward",
        "LVR",
        "fee-LVR",
        "netAPR",
        "feeLVRAPR",
        "maxDD",
        "rebals"
    );
    for policy in &report.policies {
        println!(
            "{:<22} {:>10.2} {:>10.2} {:>9.2} {:>9.2} {:>9.2} {:>10.2} {:>9} {:>10} {:>9.2} {:>8}",
            policy.policy,
            policy.net_pnl_usd,
            policy.net_vs_hold_usd,
            policy.fee_income_usd,
            policy.reward_income_usd,
            policy.lvr_usd,
            policy.fee_minus_lvr_usd,
            format_window_apr(policy.net_pnl_usd, params.capital_usd, window_years),
            format_window_apr(policy.fee_minus_lvr_usd, params.capital_usd, window_years),
            policy.max_drawdown_usd,
            policy.rebalances,
        );
    }
    Ok(())
}

fn replay_window_years(report: &ReplayReport, params: &ReplayParams) -> Option<f64> {
    let first = report.block_first?;
    let last = report.block_last?;
    if last <= first || params.block_seconds <= 0.0 {
        return None;
    }
    Some(((last - first) as f64 * params.block_seconds) / (365.0 * 24.0 * 60.0 * 60.0))
}

fn replay_window_years_from_swaps(
    swaps: &[autopool_backtest::SwapObs],
    params: &ReplayParams,
) -> Option<f64> {
    let first = swaps.first()?.block;
    let last = swaps.last()?.block;
    if last <= first || params.block_seconds <= 0.0 {
        return None;
    }
    Some(((last - first) as f64 * params.block_seconds) / (365.0 * 24.0 * 60.0 * 60.0))
}

fn window_apr_pct(value_usd: f64, capital_usd: f64, window_years: Option<f64>) -> Option<f64> {
    match window_years {
        Some(years) if years > 0.0 && capital_usd > 0.0 => {
            Some(value_usd / capital_usd / years * 100.0)
        }
        _ => None,
    }
}

fn format_window_apr(value_usd: f64, capital_usd: f64, window_years: Option<f64>) -> String {
    window_apr_pct(value_usd, capital_usd, window_years)
        .map(|value| format!("{value:.0}%"))
        .unwrap_or_else(|| "-".to_string())
}

fn summarize_window_rows(rows: &[WindowPolicyRow]) -> Vec<WindowPolicySummary> {
    let mut policies = rows
        .iter()
        .map(|row| row.policy.clone())
        .collect::<Vec<_>>();
    policies.sort();
    policies.dedup();
    policies
        .into_iter()
        .filter_map(|policy| {
            let policy_rows = rows
                .iter()
                .filter(|row| row.policy == policy)
                .collect::<Vec<_>>();
            if policy_rows.is_empty() {
                return None;
            }
            let windows = policy_rows.len();
            let net_aprs = policy_rows
                .iter()
                .filter_map(|row| row.net_apr_pct)
                .collect::<Vec<_>>();
            let fee_lvr_aprs = policy_rows
                .iter()
                .filter_map(|row| row.fee_lvr_apr_pct)
                .collect::<Vec<_>>();
            Some(WindowPolicySummary {
                policy,
                windows,
                win_rate_vs_hold_pct: mean_values(
                    &policy_rows
                        .iter()
                        .map(|row| {
                            if row.net_vs_hold_usd > 0.0 {
                                100.0
                            } else {
                                0.0
                            }
                        })
                        .collect::<Vec<_>>(),
                ),
                mean_net_pnl_usd: mean_values(
                    &policy_rows
                        .iter()
                        .map(|row| row.net_pnl_usd)
                        .collect::<Vec<_>>(),
                ),
                mean_net_vs_hold_usd: mean_values(
                    &policy_rows
                        .iter()
                        .map(|row| row.net_vs_hold_usd)
                        .collect::<Vec<_>>(),
                ),
                mean_fee_minus_lvr_usd: mean_values(
                    &policy_rows
                        .iter()
                        .map(|row| row.fee_minus_lvr_usd)
                        .collect::<Vec<_>>(),
                ),
                mean_net_apr_pct: mean_values_opt(&net_aprs),
                p05_net_apr_pct: percentile_f64(&net_aprs, 5.0),
                p50_net_apr_pct: percentile_f64(&net_aprs, 50.0),
                p95_net_apr_pct: percentile_f64(&net_aprs, 95.0),
                mean_fee_lvr_apr_pct: mean_values_opt(&fee_lvr_aprs),
                worst_max_drawdown_usd: policy_rows
                    .iter()
                    .map(|row| row.max_drawdown_usd)
                    .fold(0.0, f64::max),
                mean_rebalances: mean_values(
                    &policy_rows
                        .iter()
                        .map(|row| row.rebalances as f64)
                        .collect::<Vec<_>>(),
                ),
            })
        })
        .collect()
}

fn emit_normalized_window_report(
    report: NormalizedWindowReplayReport,
    format: OutputFormat,
) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "window replay {} swaps={} windows={} window_swaps={} step_swaps={}",
        report.label, report.swaps, report.windows, report.window_swaps, report.step_swaps
    );
    println!("APR columns are mechanical per-window annualization summaries, not forecasts");
    println!(
        "{:<22} {:>4} {:>8} {:>10} {:>10} {:>10} {:>10} {:>12} {:>10} {:>9}",
        "policy",
        "n",
        "win%",
        "meanNet",
        "meanVsH",
        "meanAPR",
        "p05APR",
        "feeLVRAPR",
        "worstDD",
        "reb/win"
    );
    for summary in &report.summaries {
        println!(
            "{:<22} {:>4} {:>7.0}% {:>10.2} {:>10.2} {:>9} {:>9} {:>11} {:>10.2} {:>9.2}",
            summary.policy,
            summary.windows,
            summary.win_rate_vs_hold_pct,
            summary.mean_net_pnl_usd,
            summary.mean_net_vs_hold_usd,
            optional_pct(summary.mean_net_apr_pct),
            optional_pct(summary.p05_net_apr_pct),
            optional_pct(summary.mean_fee_lvr_apr_pct),
            summary.worst_max_drawdown_usd,
            summary.mean_rebalances
        );
    }
    Ok(())
}

fn mean_values(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn mean_values_opt(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| mean_values(values))
}

fn stddev_values(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let mean = mean_values(values);
    let variance = values
        .iter()
        .map(|value| {
            let diff = *value - mean;
            diff * diff
        })
        .sum::<f64>()
        / values.len() as f64;
    variance.sqrt()
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

fn replay_normalized_swaps(
    spec_path: PathBuf,
    swaps_path: Option<PathBuf>,
    overrides: &NormalizedReplayParams,
    format: OutputFormat,
) -> Result<()> {
    let spec = read_replay_pool_spec(&spec_path)?;
    if spec.replay_model != "clmm_tick_replay" {
        anyhow::bail!(
            "unsupported replay_model `{}` in {}; normalized replay currently supports clmm_tick_replay only",
            spec.replay_model,
            spec_path.display()
        );
    }
    let swaps_path = swaps_path.unwrap_or_else(|| {
        spec_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("swaps.jsonl")
    });
    let params = overrides.to_replay_params(&spec)?;
    let mut swaps = load_normalized_swaps(&swaps_path)?;
    if params.invert {
        for swap in &mut swaps {
            *swap = swap.inverted();
        }
    }
    if swaps.is_empty() {
        anyhow::bail!("no normalized swaps found in {}", swaps_path.display());
    }

    let policies = run_battery(&params, &swaps);
    let report = ReplayReport {
        label: format!("{} ({})", spec.symbol, spec.pool_address),
        swaps: swaps.len(),
        block_first: swaps.first().map(|swap| swap.block),
        block_last: swaps.last().map(|swap| swap.block),
        tick_first: swaps.first().map(|swap| swap.tick),
        tick_last: swaps.last().map(|swap| swap.tick),
        config: params_json(&params),
        policies,
    };
    emit_replay_report(report, &params, format)
}

fn replay_normalized_windows(
    spec_path: PathBuf,
    swaps_path: Option<PathBuf>,
    window_swaps: usize,
    step_swaps: usize,
    min_windows: usize,
    overrides: &NormalizedReplayParams,
    format: OutputFormat,
) -> Result<()> {
    if window_swaps == 0 || step_swaps == 0 {
        anyhow::bail!("window_swaps and step_swaps must be positive");
    }
    let report = build_normalized_window_report(
        &spec_path,
        swaps_path,
        window_swaps,
        step_swaps,
        min_windows,
        overrides,
    )?;
    emit_normalized_window_report(report, format)
}

fn merge_normalized_swaps(
    inputs: Vec<PathBuf>,
    output: PathBuf,
    format: OutputFormat,
) -> Result<()> {
    if inputs.is_empty() {
        anyhow::bail!("pass at least one --input");
    }
    let mut seen = BTreeSet::new();
    let mut merged = Vec::new();
    let mut input_rows = 0usize;
    for input in &inputs {
        let swaps = load_normalized_swaps(input)?;
        input_rows += swaps.len();
        for swap in swaps {
            if seen.insert(normalized_swap_key(&swap)) {
                merged.push(swap);
            }
        }
    }
    merged.sort_by(|left, right| {
        left.block
            .cmp(&right.block)
            .then_with(|| left.log_index.cmp(&right.log_index))
            .then_with(|| left.tick.cmp(&right.tick))
    });
    for (index, swap) in merged.iter_mut().enumerate() {
        swap.log_index = index as u64;
    }
    write_swap_obs_jsonl(&output, &merged)?;

    let report = MergeNormalizedSwapsReport {
        inputs: inputs
            .iter()
            .map(|input| input.display().to_string())
            .collect(),
        output: output.display().to_string(),
        input_rows,
        unique_rows: merged.len(),
        duplicate_rows: input_rows.saturating_sub(merged.len()),
        block_first: merged.first().map(|swap| swap.block),
        block_last: merged.last().map(|swap| swap.block),
        tick_first: merged.first().map(|swap| swap.tick),
        tick_last: merged.last().map(|swap| swap.tick),
    };

    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "merged normalized swaps inputs={} input_rows={} unique_rows={} duplicates={} output={}",
        report.inputs.len(),
        report.input_rows,
        report.unique_rows,
        report.duplicate_rows,
        report.output
    );
    println!(
        "span blocks={}..{} ticks={}..{}",
        optional_u64(report.block_first),
        optional_u64(report.block_last),
        optional_i32(report.tick_first),
        optional_i32(report.tick_last),
    );
    Ok(())
}

fn normalized_swap_key(swap: &autopool_backtest::SwapObs) -> (u64, u64, u64, u64, u64, i32) {
    (
        swap.block,
        swap.amount0.to_bits(),
        swap.amount1.to_bits(),
        swap.sqrt_price_x96.to_bits(),
        swap.liquidity.to_bits(),
        swap.tick,
    )
}

fn write_swap_obs_jsonl(path: &Path, swaps: &[autopool_backtest::SwapObs]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating output directory {}", parent.display()))?;
    }
    let mut file = File::create(path)
        .with_context(|| format!("creating normalized swaps {}", path.display()))?;
    for swap in swaps {
        writeln!(file, "{}", serde_json::to_string(swap)?)
            .with_context(|| format!("writing normalized swaps {}", path.display()))?;
    }
    Ok(())
}

fn build_normalized_window_report(
    spec_path: &Path,
    swaps_path: Option<PathBuf>,
    window_swaps: usize,
    step_swaps: usize,
    min_windows: usize,
    overrides: &NormalizedReplayParams,
) -> Result<NormalizedWindowReplayReport> {
    let spec = read_replay_pool_spec(spec_path)?;
    if spec.replay_model != "clmm_tick_replay" {
        anyhow::bail!(
            "unsupported replay_model `{}` in {}; normalized windows currently supports clmm_tick_replay only",
            spec.replay_model,
            spec_path.display()
        );
    }
    let swaps_path = swaps_path.unwrap_or_else(|| {
        spec_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("swaps.jsonl")
    });
    let params = overrides.to_replay_params(&spec)?;
    let mut swaps = load_normalized_swaps(&swaps_path)?;
    if params.invert {
        for swap in &mut swaps {
            *swap = swap.inverted();
        }
    }
    if swaps.len() < window_swaps {
        anyhow::bail!(
            "need at least {} swaps for one window, found {} in {}",
            window_swaps,
            swaps.len(),
            swaps_path.display()
        );
    }
    let rows = build_window_rows(&swaps, &params, window_swaps, step_swaps);
    let windows = rows
        .iter()
        .map(|row| row.window_index)
        .max()
        .map(|max| max + 1)
        .unwrap_or(0);
    if windows < min_windows {
        anyhow::bail!(
            "need at least {} windows, got {} (swaps={}, window_swaps={}, step_swaps={})",
            min_windows,
            windows,
            swaps.len(),
            window_swaps,
            step_swaps
        );
    }
    Ok(NormalizedWindowReplayReport {
        label: format!("{} ({})", spec.symbol, spec.pool_address),
        swaps: swaps.len(),
        window_swaps,
        step_swaps,
        windows,
        config: params_json(&params),
        summaries: summarize_window_rows(&rows),
        rows,
    })
}

fn build_window_rows(
    swaps: &[autopool_backtest::SwapObs],
    params: &ReplayParams,
    window_swaps: usize,
    step_swaps: usize,
) -> Vec<WindowPolicyRow> {
    let mut rows = Vec::new();
    let mut window_index = 0usize;
    let mut start = 0usize;
    while start + window_swaps <= swaps.len() {
        let end = start + window_swaps;
        let window = &swaps[start..end];
        let regime = classify_window_regime(window);
        let years = replay_window_years_from_swaps(window, params);
        let minutes = years.unwrap_or(0.0) * 365.0 * 24.0 * 60.0;
        let policies = run_battery(params, window);
        for policy in policies {
            rows.push(WindowPolicyRow {
                window_index,
                swap_start: start,
                swap_end_exclusive: end,
                swaps: window.len(),
                block_first: window.first().map(|swap| swap.block).unwrap_or(0),
                block_last: window.last().map(|swap| swap.block).unwrap_or(0),
                tick_first: regime.tick_first,
                tick_last: regime.tick_last,
                tick_delta: regime.tick_delta,
                trend_strength: regime.trend_strength,
                regime: regime.label.clone(),
                minutes,
                policy: policy.policy,
                net_pnl_usd: policy.net_pnl_usd,
                net_vs_hold_usd: policy.net_vs_hold_usd,
                fee_minus_lvr_usd: policy.fee_minus_lvr_usd,
                net_apr_pct: window_apr_pct(policy.net_pnl_usd, params.capital_usd, years),
                fee_lvr_apr_pct: window_apr_pct(
                    policy.fee_minus_lvr_usd,
                    params.capital_usd,
                    years,
                ),
                max_drawdown_usd: policy.max_drawdown_usd,
                rebalances: policy.rebalances,
            });
        }
        window_index += 1;
        start += step_swaps;
    }
    rows
}

fn classify_window_regime(swaps: &[autopool_backtest::SwapObs]) -> WindowRegimeLabel {
    let tick_first = swaps.first().map(|swap| swap.tick).unwrap_or(0);
    let tick_last = swaps.last().map(|swap| swap.tick).unwrap_or(tick_first);
    let tick_delta = tick_last - tick_first;
    let deltas = swaps
        .windows(2)
        .map(|pair| (pair[1].tick - pair[0].tick) as f64)
        .collect::<Vec<_>>();
    let realized = stddev_values(&deltas).max(1.0);
    let trend_strength =
        (tick_delta as f64).abs() / (realized * (swaps.len().max(1) as f64).sqrt());
    let abs_delta = tick_delta.abs();
    let label = if trend_strength >= 1.0 && abs_delta >= 60 {
        if tick_delta > 0 {
            "trend_up_risk".to_string()
        } else {
            "trend_down_money".to_string()
        }
    } else if realized >= 20.0 {
        "volatile_range".to_string()
    } else {
        "range".to_string()
    };
    WindowRegimeLabel {
        tick_first,
        tick_last,
        tick_delta,
        trend_strength,
        label,
    }
}

#[allow(clippy::too_many_arguments)]
fn replay_normalized_hedge_grid(
    spec_path: PathBuf,
    swaps_path: Option<PathBuf>,
    window_swaps: usize,
    step_swaps: usize,
    min_windows: usize,
    hedge_fractions: Vec<f64>,
    regime_rule: RegimeHedgeRule,
    overrides: &NormalizedReplayParams,
    format: OutputFormat,
) -> Result<()> {
    if hedge_fractions.is_empty() {
        anyhow::bail!("pass at least one --grid-hedge-fraction");
    }
    for hedge_fraction in regime_rule.fractions() {
        if !(0.0..=1.5).contains(&hedge_fraction) {
            anyhow::bail!(
                "regime rule hedge fraction {hedge_fraction} outside supported range 0.0..=1.5"
            );
        }
    }
    let mut rows = Vec::new();
    let mut label = String::new();
    let mut swaps = 0usize;
    let mut windows = 0usize;
    let mut config = serde_json::Value::Null;
    let mut regime_source_rows = Vec::new();
    let mut fractions = hedge_fractions;
    fractions.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    fractions.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let control_fraction = *fractions.first().unwrap_or(&0.0);

    for hedge_fraction in fractions {
        if !(0.0..=1.5).contains(&hedge_fraction) {
            anyhow::bail!("hedge fraction {hedge_fraction} outside supported range 0.0..=1.5");
        }
        let grid_overrides = overrides.with_hedge_fraction(hedge_fraction);
        let report = build_normalized_window_report(
            &spec_path,
            swaps_path.clone(),
            window_swaps,
            step_swaps,
            min_windows,
            &grid_overrides,
        )?;
        if label.is_empty() {
            label = report.label.clone();
            swaps = report.swaps;
            windows = report.windows;
        }
        config = report.config.clone();
        for window_row in report.rows.iter().filter(|row| {
            row.policy == "hedged_narrow"
                || ((row.policy == "delta_hedged" || row.policy == "hedged_wide")
                    && (hedge_fraction - control_fraction).abs() < 1e-9)
        }) {
            regime_source_rows.push((hedge_fraction, window_row.clone()));
        }
        for summary in report.summaries.into_iter().filter(|summary| {
            summary.policy == "hedged_narrow"
                || ((summary.policy == "delta_hedged" || summary.policy == "hedged_wide")
                    && (hedge_fraction - control_fraction).abs() < 1e-9)
        }) {
            let score = hedge_grid_score(&summary);
            rows.push(HedgeGridRow {
                hedge_fraction,
                policy: summary.policy,
                windows: summary.windows,
                win_rate_vs_hold_pct: summary.win_rate_vs_hold_pct,
                mean_net_pnl_usd: summary.mean_net_pnl_usd,
                mean_net_vs_hold_usd: summary.mean_net_vs_hold_usd,
                mean_net_apr_pct: summary.mean_net_apr_pct,
                p05_net_apr_pct: summary.p05_net_apr_pct,
                mean_fee_lvr_apr_pct: summary.mean_fee_lvr_apr_pct,
                worst_max_drawdown_usd: summary.worst_max_drawdown_usd,
                score,
            });
        }
    }
    rows.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.policy.cmp(&b.policy))
            .then_with(|| {
                a.hedge_fraction
                    .partial_cmp(&b.hedge_fraction)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    let report = HedgeGridReport {
        label,
        swaps,
        window_swaps,
        step_swaps,
        windows,
        config,
        rows,
        regime_rows: summarize_hedge_regime_rows(&regime_source_rows),
        rule_rows: summarize_lagged_regime_rule_rows(&regime_source_rows, &regime_rule),
    };
    emit_hedge_grid_report(report, format)
}

fn replay_promotion_gate(
    spec_path: PathBuf,
    swaps_path: Option<PathBuf>,
    window_configs: Vec<String>,
    gate_policy: PromotionGatePolicy,
    hedge_fractions: Vec<f64>,
    regime_rule: RegimeHedgeRule,
    policy_rule: RegimePolicyRule,
    thresholds: PromotionGateThresholds,
    overrides: &NormalizedReplayParams,
    format: OutputFormat,
) -> Result<()> {
    let configs = parse_promotion_window_configs(&window_configs)?;
    let params = {
        let spec = read_replay_pool_spec(&spec_path)?;
        overrides.to_replay_params(&spec)?
    };
    let max_worst_drawdown_usd = params.capital_usd * thresholds.max_drawdown_pct;
    let mut rows = Vec::new();
    let mut label = String::new();
    let mut swaps = 0usize;
    for config in configs {
        let (row_label, row_swaps, row) = match gate_policy {
            PromotionGatePolicy::LaggedRegimeRule => {
                let (row_label, row_swaps, rule_row) = evaluate_lagged_rule_window(
                    &spec_path,
                    swaps_path.clone(),
                    config,
                    &hedge_fractions,
                    &regime_rule,
                    overrides,
                )?;
                (
                    row_label,
                    row_swaps,
                    promotion_window_row(config, rule_row, &thresholds, max_worst_drawdown_usd),
                )
            }
            PromotionGatePolicy::LaggedPolicySwitch => {
                let (row_label, row_swaps, rule_row) = evaluate_lagged_policy_switch_window(
                    &spec_path,
                    swaps_path.clone(),
                    config,
                    &policy_rule,
                    overrides,
                )?;
                (
                    row_label,
                    row_swaps,
                    promotion_window_row(config, rule_row, &thresholds, max_worst_drawdown_usd),
                )
            }
            PromotionGatePolicy::HedgedWide | PromotionGatePolicy::DeltaHedged => {
                let (row_label, row_swaps, summary) = evaluate_fixed_policy_window(
                    &spec_path,
                    swaps_path.clone(),
                    config,
                    gate_policy.label(),
                    overrides,
                )?;
                (
                    row_label,
                    row_swaps,
                    promotion_window_row_from_summary(
                        config,
                        summary,
                        &thresholds,
                        max_worst_drawdown_usd,
                    ),
                )
            }
        };
        if label.is_empty() {
            label = row_label;
            swaps = row_swaps;
        }
        rows.push(row);
    }
    let mut reasons = Vec::new();
    for row in &rows {
        if !row.passed {
            reasons.push(format!(
                "{}:{} failed ({})",
                row.window_swaps,
                row.step_swaps,
                row.reasons.join("; ")
            ));
        }
    }
    let verdict = if rows.iter().all(|row| row.passed) {
        "candidate_shadow"
    } else if rows.iter().any(|row| row.mean_net_vs_hold_usd < 0.0)
        || rows
            .iter()
            .any(|row| row.p05_net_apr_pct.unwrap_or(f64::NEG_INFINITY) < 0.0)
    {
        "reject_replay"
    } else {
        "needs_more_data"
    }
    .to_string();
    let report = PromotionGateReport {
        label,
        swaps,
        policy: gate_policy.label().to_string(),
        hedge_map: match gate_policy {
            PromotionGatePolicy::LaggedRegimeRule => regime_rule.describe(),
            PromotionGatePolicy::LaggedPolicySwitch => policy_rule.describe(),
            PromotionGatePolicy::HedgedWide | PromotionGatePolicy::DeltaHedged => "-".to_string(),
        },
        thresholds,
        max_worst_drawdown_usd,
        verdict,
        reasons,
        rows,
    };
    emit_promotion_gate_report(report, format)
}

fn default_promotion_window_configs() -> Vec<PromotionWindowConfig> {
    vec![
        PromotionWindowConfig {
            window_swaps: 25,
            step_swaps: 10,
            min_windows: 4,
        },
        PromotionWindowConfig {
            window_swaps: 40,
            step_swaps: 15,
            min_windows: 4,
        },
        PromotionWindowConfig {
            window_swaps: 60,
            step_swaps: 20,
            min_windows: 3,
        },
        PromotionWindowConfig {
            window_swaps: 80,
            step_swaps: 25,
            min_windows: 3,
        },
    ]
}

fn parse_promotion_window_configs(raw: &[String]) -> Result<Vec<PromotionWindowConfig>> {
    if raw.is_empty() {
        return Ok(default_promotion_window_configs());
    }
    raw.iter()
        .map(|item| {
            let parts = item.split(':').collect::<Vec<_>>();
            if parts.len() != 3 {
                anyhow::bail!(
                    "invalid --window-config `{item}`; expected window_swaps:step_swaps:min_windows"
                );
            }
            let window_swaps = parts[0]
                .parse::<usize>()
                .with_context(|| format!("invalid window_swaps in `{item}`"))?;
            let step_swaps = parts[1]
                .parse::<usize>()
                .with_context(|| format!("invalid step_swaps in `{item}`"))?;
            let min_windows = parts[2]
                .parse::<usize>()
                .with_context(|| format!("invalid min_windows in `{item}`"))?;
            if window_swaps == 0 || step_swaps == 0 || min_windows == 0 {
                anyhow::bail!("window config values must be positive in `{item}`");
            }
            Ok(PromotionWindowConfig {
                window_swaps,
                step_swaps,
                min_windows,
            })
        })
        .collect()
}

fn evaluate_lagged_rule_window(
    spec_path: &Path,
    swaps_path: Option<PathBuf>,
    config: PromotionWindowConfig,
    hedge_fractions: &[f64],
    regime_rule: &RegimeHedgeRule,
    overrides: &NormalizedReplayParams,
) -> Result<(String, usize, HedgeGridRuleRow)> {
    let mut fractions = hedge_fractions.to_vec();
    fractions.extend(regime_rule.fractions());
    fractions.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    fractions.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let mut regime_source_rows = Vec::new();
    let mut label = String::new();
    let mut swaps = 0usize;
    for hedge_fraction in fractions {
        if !(0.0..=1.5).contains(&hedge_fraction) {
            anyhow::bail!("hedge fraction {hedge_fraction} outside supported range 0.0..=1.5");
        }
        let report = build_normalized_window_report(
            spec_path,
            swaps_path.clone(),
            config.window_swaps,
            config.step_swaps,
            config.min_windows,
            &overrides.with_hedge_fraction(hedge_fraction),
        )?;
        if label.is_empty() {
            label = report.label.clone();
            swaps = report.swaps;
        }
        for window_row in report
            .rows
            .iter()
            .filter(|row| row.policy == "hedged_narrow")
        {
            regime_source_rows.push((hedge_fraction, window_row.clone()));
        }
    }
    let rule_row = summarize_lagged_regime_rule_rows(&regime_source_rows, regime_rule)
        .into_iter()
        .next()
        .with_context(|| {
            format!(
                "no lagged regime windows for config {}:{}",
                config.window_swaps, config.step_swaps
            )
        })?;
    Ok((label, swaps, rule_row))
}

fn evaluate_lagged_policy_switch_window(
    spec_path: &Path,
    swaps_path: Option<PathBuf>,
    config: PromotionWindowConfig,
    policy_rule: &RegimePolicyRule,
    overrides: &NormalizedReplayParams,
) -> Result<(String, usize, HedgeGridRuleRow)> {
    let report = build_normalized_window_report(
        spec_path,
        swaps_path,
        config.window_swaps,
        config.step_swaps,
        config.min_windows,
        overrides,
    )?;
    let rule_row = summarize_lagged_policy_switch_rows(&report.rows, policy_rule)
        .into_iter()
        .next()
        .with_context(|| {
            format!(
                "no lagged policy-switch windows for config {}:{}",
                config.window_swaps, config.step_swaps
            )
        })?;
    Ok((report.label, report.swaps, rule_row))
}

fn evaluate_fixed_policy_window(
    spec_path: &Path,
    swaps_path: Option<PathBuf>,
    config: PromotionWindowConfig,
    policy: &str,
    overrides: &NormalizedReplayParams,
) -> Result<(String, usize, WindowPolicySummary)> {
    let report = build_normalized_window_report(
        spec_path,
        swaps_path,
        config.window_swaps,
        config.step_swaps,
        config.min_windows,
        overrides,
    )?;
    let summary = report
        .summaries
        .into_iter()
        .find(|summary| summary.policy == policy)
        .with_context(|| {
            format!(
                "policy `{policy}` not found for config {}:{}",
                config.window_swaps, config.step_swaps
            )
        })?;
    Ok((report.label, report.swaps, summary))
}

fn promotion_window_row(
    config: PromotionWindowConfig,
    row: HedgeGridRuleRow,
    thresholds: &PromotionGateThresholds,
    max_worst_drawdown_usd: f64,
) -> PromotionGateWindowRow {
    promotion_window_row_from_metrics(
        config,
        row.windows,
        row.skipped_windows,
        row.win_rate_vs_hold_pct,
        row.mean_net_pnl_usd,
        row.mean_net_vs_hold_usd,
        row.mean_net_apr_pct,
        row.p05_net_apr_pct,
        row.worst_max_drawdown_usd,
        thresholds,
        max_worst_drawdown_usd,
    )
}

fn promotion_window_row_from_summary(
    config: PromotionWindowConfig,
    summary: WindowPolicySummary,
    thresholds: &PromotionGateThresholds,
    max_worst_drawdown_usd: f64,
) -> PromotionGateWindowRow {
    promotion_window_row_from_metrics(
        config,
        summary.windows,
        0,
        summary.win_rate_vs_hold_pct,
        summary.mean_net_pnl_usd,
        summary.mean_net_vs_hold_usd,
        summary.mean_net_apr_pct,
        summary.p05_net_apr_pct,
        summary.worst_max_drawdown_usd,
        thresholds,
        max_worst_drawdown_usd,
    )
}

#[allow(clippy::too_many_arguments)]
fn promotion_window_row_from_metrics(
    config: PromotionWindowConfig,
    windows: usize,
    skipped_windows: usize,
    win_rate_vs_hold_pct: f64,
    mean_net_pnl_usd: f64,
    mean_net_vs_hold_usd: f64,
    mean_net_apr_pct: Option<f64>,
    p05_net_apr_pct: Option<f64>,
    worst_max_drawdown_usd: f64,
    thresholds: &PromotionGateThresholds,
    max_worst_drawdown_usd: f64,
) -> PromotionGateWindowRow {
    let mut reasons = Vec::new();
    if windows < config.min_windows {
        reasons.push(format!("windows {} < min {}", windows, config.min_windows));
    }
    if win_rate_vs_hold_pct < thresholds.min_win_rate_vs_hold_pct {
        reasons.push(format!(
            "win_rate {:.0}% < {:.0}%",
            win_rate_vs_hold_pct, thresholds.min_win_rate_vs_hold_pct
        ));
    }
    if mean_net_vs_hold_usd < thresholds.min_mean_vs_hold_usd {
        reasons.push(format!(
            "mean_vs_hold {:.2} < {:.2}",
            mean_net_vs_hold_usd, thresholds.min_mean_vs_hold_usd
        ));
    }
    match p05_net_apr_pct {
        Some(p05) if p05 >= thresholds.min_p05_net_apr_pct => {}
        Some(p05) => reasons.push(format!(
            "p05_apr {:.0}% < {:.0}%",
            p05, thresholds.min_p05_net_apr_pct
        )),
        None => reasons.push("missing_p05_apr".to_string()),
    }
    if worst_max_drawdown_usd > max_worst_drawdown_usd {
        reasons.push(format!(
            "worst_dd {:.2} > {:.2}",
            worst_max_drawdown_usd, max_worst_drawdown_usd
        ));
    }
    PromotionGateWindowRow {
        window_swaps: config.window_swaps,
        step_swaps: config.step_swaps,
        min_windows: config.min_windows,
        windows,
        skipped_windows,
        win_rate_vs_hold_pct,
        mean_net_pnl_usd,
        mean_net_vs_hold_usd,
        mean_net_apr_pct,
        p05_net_apr_pct,
        worst_max_drawdown_usd,
        passed: reasons.is_empty(),
        reasons,
    }
}

fn emit_promotion_gate_report(report: PromotionGateReport, format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "promotion gate {} swaps={} policy={} verdict={} hedge_map={}",
        report.label, report.swaps, report.policy, report.verdict, report.hedge_map
    );
    println!(
        "thresholds: win>={:.0}% meanVsH>={:.2} p05APR>={:.0}% worstDD<={:.2}",
        report.thresholds.min_win_rate_vs_hold_pct,
        report.thresholds.min_mean_vs_hold_usd,
        report.thresholds.min_p05_net_apr_pct,
        report.max_worst_drawdown_usd
    );
    println!(
        "{:<9} {:>4} {:>5} {:>8} {:>10} {:>10} {:>10} {:>9} {:<5}  reasons",
        "window", "n", "skip", "win%", "meanNet", "meanVsH", "p05APR", "worstDD", "pass"
    );
    for row in &report.rows {
        println!(
            "{:<9} {:>4} {:>5} {:>7.0}% {:>10.2} {:>10.2} {:>9} {:>9.2} {:<5}  {}",
            format!("{}:{}", row.window_swaps, row.step_swaps),
            row.windows,
            row.skipped_windows,
            row.win_rate_vs_hold_pct,
            row.mean_net_pnl_usd,
            row.mean_net_vs_hold_usd,
            optional_pct(row.p05_net_apr_pct),
            row.worst_max_drawdown_usd,
            if row.passed { "yes" } else { "no" },
            if row.reasons.is_empty() {
                "-".to_string()
            } else {
                row.reasons.join("; ")
            }
        );
    }
    if !report.reasons.is_empty() {
        println!("gate reasons: {}", report.reasons.join(" | "));
    }
    Ok(())
}

impl NormalizedReplayParams {
    fn with_hedge_fraction(&self, hedge_fraction: f64) -> Self {
        Self {
            token0_usd: self.token0_usd,
            capital_usd: self.capital_usd,
            rebalance_gas_usd: self.rebalance_gas_usd,
            rebalance_slippage_bps: self.rebalance_slippage_bps,
            narrow_half_width: self.narrow_half_width,
            wide_half_width: self.wide_half_width,
            vol_k: self.vol_k,
            action_delay_blocks: self.action_delay_blocks,
            block_seconds: self.block_seconds,
            risk_token_side: self.risk_token_side,
            funding_bps_per_day: self.funding_bps_per_day,
            hedge_fraction,
            trend_exit_threshold: self.trend_exit_threshold,
            reward_apr: self.reward_apr,
            reward_haircut: self.reward_haircut,
        }
    }
}

fn hedge_grid_score(summary: &WindowPolicySummary) -> Option<f64> {
    let p05 = summary.p05_net_apr_pct?;
    let mean = summary.mean_net_apr_pct.unwrap_or(0.0);
    let drawdown_penalty = summary.worst_max_drawdown_usd * 25.0;
    Some(p05 + summary.win_rate_vs_hold_pct * 20.0 + mean * 0.05 - drawdown_penalty)
}

fn summarize_hedge_regime_rows(rows: &[(f64, WindowPolicyRow)]) -> Vec<HedgeGridRegimeRow> {
    let mut keys = rows
        .iter()
        .map(|(hedge, row)| (*hedge, row.policy.clone(), row.regime.clone()))
        .collect::<Vec<_>>();
    keys.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    keys.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-9 && a.1 == b.1 && a.2 == b.2);
    let mut out = keys
        .into_iter()
        .filter_map(|(hedge_fraction, policy, regime)| {
            let policy_rows = rows
                .iter()
                .filter(|(hedge, row)| {
                    (*hedge - hedge_fraction).abs() < 1e-9
                        && row.policy == policy
                        && row.regime == regime
                })
                .map(|(_, row)| row)
                .collect::<Vec<_>>();
            if policy_rows.is_empty() {
                return None;
            }
            let net_aprs = policy_rows
                .iter()
                .filter_map(|row| row.net_apr_pct)
                .collect::<Vec<_>>();
            Some(HedgeGridRegimeRow {
                hedge_fraction,
                policy,
                regime,
                windows: policy_rows.len(),
                win_rate_vs_hold_pct: mean_values(
                    &policy_rows
                        .iter()
                        .map(|row| {
                            if row.net_vs_hold_usd > 0.0 {
                                100.0
                            } else {
                                0.0
                            }
                        })
                        .collect::<Vec<_>>(),
                ),
                mean_net_vs_hold_usd: mean_values(
                    &policy_rows
                        .iter()
                        .map(|row| row.net_vs_hold_usd)
                        .collect::<Vec<_>>(),
                ),
                p05_net_apr_pct: percentile_f64(&net_aprs, 5.0),
                worst_max_drawdown_usd: policy_rows
                    .iter()
                    .map(|row| row.max_drawdown_usd)
                    .fold(0.0, f64::max),
            })
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| {
        a.policy
            .cmp(&b.policy)
            .then_with(|| a.regime.cmp(&b.regime))
            .then_with(|| {
                a.hedge_fraction
                    .partial_cmp(&b.hedge_fraction)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    out
}

fn summarize_lagged_regime_rule_rows(
    rows: &[(f64, WindowPolicyRow)],
    rule: &RegimeHedgeRule,
) -> Vec<HedgeGridRuleRow> {
    let hedged_rows = rows
        .iter()
        .filter(|(_, row)| row.policy == "hedged_narrow")
        .map(|(hedge, row)| (*hedge, row.clone()))
        .collect::<Vec<_>>();
    if hedged_rows.is_empty() {
        return Vec::new();
    }
    let mut window_indices = hedged_rows
        .iter()
        .map(|(_, row)| row.window_index)
        .collect::<Vec<_>>();
    window_indices.sort_unstable();
    window_indices.dedup();

    let mut selected = Vec::new();
    let mut skipped = 0usize;
    for window_index in window_indices {
        if window_index == 0 {
            skipped += 1;
            continue;
        }
        let prior_regime = hedged_rows
            .iter()
            .find(|(_, row)| row.window_index + 1 == window_index)
            .map(|(_, row)| row.regime.clone());
        let Some(prior_regime) = prior_regime else {
            skipped += 1;
            continue;
        };
        let target_hedge = rule.hedge_fraction_for_prior_regime(&prior_regime);
        if let Some((_, row)) = hedged_rows
            .iter()
            .filter(|(_, row)| row.window_index == window_index)
            .min_by(|(left_hedge, _), (right_hedge, _)| {
                (*left_hedge - target_hedge)
                    .abs()
                    .partial_cmp(&(*right_hedge - target_hedge).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        {
            selected.push(row.clone());
        } else {
            skipped += 1;
        }
    }
    if selected.is_empty() {
        return Vec::new();
    }

    let net_aprs = selected
        .iter()
        .filter_map(|row| row.net_apr_pct)
        .collect::<Vec<_>>();
    vec![HedgeGridRuleRow {
        rule: "lagged_regime_rule".to_string(),
        hedge_map: rule.describe(),
        windows: selected.len(),
        skipped_windows: skipped,
        win_rate_vs_hold_pct: mean_values(
            &selected
                .iter()
                .map(|row| {
                    if row.net_vs_hold_usd > 0.0 {
                        100.0
                    } else {
                        0.0
                    }
                })
                .collect::<Vec<_>>(),
        ),
        mean_net_pnl_usd: mean_values(
            &selected
                .iter()
                .map(|row| row.net_pnl_usd)
                .collect::<Vec<_>>(),
        ),
        mean_net_vs_hold_usd: mean_values(
            &selected
                .iter()
                .map(|row| row.net_vs_hold_usd)
                .collect::<Vec<_>>(),
        ),
        mean_net_apr_pct: mean_values_opt(&net_aprs),
        p05_net_apr_pct: percentile_f64(&net_aprs, 5.0),
        worst_max_drawdown_usd: selected
            .iter()
            .map(|row| row.max_drawdown_usd)
            .fold(0.0, f64::max),
    }]
}

fn summarize_lagged_policy_switch_rows(
    rows: &[WindowPolicyRow],
    rule: &RegimePolicyRule,
) -> Vec<HedgeGridRuleRow> {
    let mut window_indices = rows.iter().map(|row| row.window_index).collect::<Vec<_>>();
    window_indices.sort_unstable();
    window_indices.dedup();

    let mut selected = Vec::new();
    let mut skipped = 0usize;
    for window_index in window_indices {
        if window_index == 0 {
            skipped += 1;
            continue;
        }
        let prior_regime = rows
            .iter()
            .find(|row| row.window_index + 1 == window_index)
            .map(|row| row.regime.clone());
        let Some(prior_regime) = prior_regime else {
            skipped += 1;
            continue;
        };
        let target_policy = rule.policy_for_prior_regime(&prior_regime);
        if let Some(row) = rows
            .iter()
            .find(|row| row.window_index == window_index && row.policy == target_policy)
        {
            selected.push(row.clone());
        } else {
            skipped += 1;
        }
    }
    if selected.is_empty() {
        return Vec::new();
    }

    let net_aprs = selected
        .iter()
        .filter_map(|row| row.net_apr_pct)
        .collect::<Vec<_>>();
    vec![HedgeGridRuleRow {
        rule: "lagged_policy_switch".to_string(),
        hedge_map: rule.describe(),
        windows: selected.len(),
        skipped_windows: skipped,
        win_rate_vs_hold_pct: mean_values(
            &selected
                .iter()
                .map(|row| {
                    if row.net_vs_hold_usd > 0.0 {
                        100.0
                    } else {
                        0.0
                    }
                })
                .collect::<Vec<_>>(),
        ),
        mean_net_pnl_usd: mean_values(
            &selected
                .iter()
                .map(|row| row.net_pnl_usd)
                .collect::<Vec<_>>(),
        ),
        mean_net_vs_hold_usd: mean_values(
            &selected
                .iter()
                .map(|row| row.net_vs_hold_usd)
                .collect::<Vec<_>>(),
        ),
        mean_net_apr_pct: mean_values_opt(&net_aprs),
        p05_net_apr_pct: percentile_f64(&net_aprs, 5.0),
        worst_max_drawdown_usd: selected
            .iter()
            .map(|row| row.max_drawdown_usd)
            .fold(0.0, f64::max),
    }]
}

fn emit_hedge_grid_report(report: HedgeGridReport, format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "hedge grid {} swaps={} windows={} window_swaps={} step_swaps={}",
        report.label, report.swaps, report.windows, report.window_swaps, report.step_swaps
    );
    println!(
        "score = p05APR + 20*win_rate_pct + 0.05*meanAPR - 25*worstDD; APR is mechanical window annualization"
    );
    println!("delta_hedged and hedged_wide are dynamic controls and are shown once");
    println!(
        "{:<14} {:<16} {:>4} {:>8} {:>10} {:>10} {:>10} {:>10} {:>9} {:>10}",
        "hedge",
        "policy",
        "n",
        "win%",
        "meanNet",
        "meanVsH",
        "meanAPR",
        "p05APR",
        "worstDD",
        "score"
    );
    for row in &report.rows {
        println!(
            "{:<14.2} {:<16} {:>4} {:>7.0}% {:>10.2} {:>10.2} {:>9} {:>9} {:>9.2} {:>10}",
            row.hedge_fraction,
            row.policy,
            row.windows,
            row.win_rate_vs_hold_pct,
            row.mean_net_pnl_usd,
            row.mean_net_vs_hold_usd,
            optional_pct(row.mean_net_apr_pct),
            optional_pct(row.p05_net_apr_pct),
            row.worst_max_drawdown_usd,
            optional_f64(row.score)
        );
    }
    if !report.regime_rows.is_empty() {
        println!();
        println!("by regime:");
        println!(
            "{:<14} {:<16} {:<18} {:>4} {:>8} {:>10} {:>10} {:>9}",
            "hedge", "policy", "regime", "n", "win%", "meanVsH", "p05APR", "worstDD"
        );
        for row in &report.regime_rows {
            println!(
                "{:<14.2} {:<16} {:<18} {:>4} {:>7.0}% {:>10.2} {:>9} {:>9.2}",
                row.hedge_fraction,
                row.policy,
                row.regime,
                row.windows,
                row.win_rate_vs_hold_pct,
                row.mean_net_vs_hold_usd,
                optional_pct(row.p05_net_apr_pct),
                row.worst_max_drawdown_usd,
            );
        }
    }
    if !report.rule_rows.is_empty() {
        println!();
        println!("lagged regime rules:");
        println!(
            "{:<22} {:>4} {:>5} {:>8} {:>10} {:>10} {:>10} {:>10} {:>9}  map",
            "rule", "n", "skip", "win%", "meanNet", "meanVsH", "meanAPR", "p05APR", "worstDD"
        );
        for row in &report.rule_rows {
            println!(
                "{:<22} {:>4} {:>5} {:>7.0}% {:>10.2} {:>10.2} {:>9} {:>9} {:>9.2}  {}",
                row.rule,
                row.windows,
                row.skipped_windows,
                row.win_rate_vs_hold_pct,
                row.mean_net_pnl_usd,
                row.mean_net_vs_hold_usd,
                optional_pct(row.mean_net_apr_pct),
                optional_pct(row.p05_net_apr_pct),
                row.worst_max_drawdown_usd,
                row.hedge_map,
            );
        }
    }
    Ok(())
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
    token_id: Option<u64>,
    recipient: String,
    slippage_bps: f64,
    rebalance_gas_units: u64,
    eth_usd: f64,
    expected_edge_usd: f64,
    max_gas_to_edge_pct: f64,
    staked: bool,
    risk_token_side: RiskTokenSide,
    max_risk_token_share: f64,
    skip_quoter: bool,
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

#[derive(Debug, Clone, Copy)]
struct DryRunSwapPlan {
    zero_for_one: bool,
    amount_in: f64,
    expected_out: f64,
    price_impact_bps: f64,
}

fn solve_inventory_swap(
    sqrt_x96: f64,
    liquidity: f64,
    fee_fraction: f64,
    decimals0: u8,
    decimals1: u8,
    current0: f64,
    current1: f64,
    target_ratio_1_per_0: f64,
) -> Option<DryRunSwapPlan> {
    if liquidity <= 0.0
        || current0 < 0.0
        || current1 < 0.0
        || !target_ratio_1_per_0.is_finite()
        || target_ratio_1_per_0 <= 0.0
    {
        return None;
    }

    let current_ratio = if current0 > 0.0 {
        current1 / current0
    } else {
        f64::INFINITY
    };
    if current_ratio.is_finite()
        && ((current_ratio - target_ratio_1_per_0).abs() / target_ratio_1_per_0) < 0.0001
    {
        return None;
    }

    if current_ratio < target_ratio_1_per_0 {
        let mut lo = 0.0;
        let mut hi = current0 * 0.999;
        if hi <= 0.0 {
            return None;
        }
        for _ in 0..80 {
            let mid = (lo + hi) / 2.0;
            let sim = autopool_backtest::simulate_v3_swap(
                sqrt_x96,
                liquidity,
                fee_fraction,
                mid * 10f64.powi(decimals0 as i32),
                true,
            );
            let out = sim.amount_out / 10f64.powi(decimals1 as i32);
            let post0 = current0 - mid;
            let post1 = current1 + out;
            let ratio = if post0 > 0.0 {
                post1 / post0
            } else {
                f64::INFINITY
            };
            if ratio < target_ratio_1_per_0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let sim = autopool_backtest::simulate_v3_swap(
            sqrt_x96,
            liquidity,
            fee_fraction,
            hi * 10f64.powi(decimals0 as i32),
            true,
        );
        return Some(DryRunSwapPlan {
            zero_for_one: true,
            amount_in: hi,
            expected_out: sim.amount_out / 10f64.powi(decimals1 as i32),
            price_impact_bps: sim.price_impact_bps,
        });
    }

    let mut lo = 0.0;
    let mut hi = current1 * 0.999;
    if hi <= 0.0 {
        return None;
    }
    for _ in 0..80 {
        let mid = (lo + hi) / 2.0;
        let sim = autopool_backtest::simulate_v3_swap(
            sqrt_x96,
            liquidity,
            fee_fraction,
            mid * 10f64.powi(decimals1 as i32),
            false,
        );
        let out = sim.amount_out / 10f64.powi(decimals0 as i32);
        let post0 = current0 + out;
        let post1 = current1 - mid;
        let ratio = if post0 > 0.0 {
            post1 / post0
        } else {
            f64::INFINITY
        };
        if ratio > target_ratio_1_per_0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let sim = autopool_backtest::simulate_v3_swap(
        sqrt_x96,
        liquidity,
        fee_fraction,
        hi * 10f64.powi(decimals1 as i32),
        false,
    );
    Some(DryRunSwapPlan {
        zero_for_one: false,
        amount_in: hi,
        expected_out: sim.amount_out / 10f64.powi(decimals0 as i32),
        price_impact_bps: sim.price_impact_bps,
    })
}

fn fit_to_range_ratio(balance0: f64, balance1: f64, target_ratio_1_per_0: f64) -> (f64, f64) {
    if !target_ratio_1_per_0.is_finite() || target_ratio_1_per_0 <= 0.0 {
        return (balance0.max(0.0), balance1.max(0.0));
    }
    let amount0 = balance0
        .max(0.0)
        .min(balance1.max(0.0) / target_ratio_1_per_0);
    (amount0, amount0 * target_ratio_1_per_0)
}

fn risk_token_inventory_share(
    amount0: f64,
    amount1: f64,
    price_token1_per_token0: f64,
    side: RiskTokenSide,
) -> f64 {
    if amount0 < 0.0 || amount1 < 0.0 || price_token1_per_token0 <= 0.0 {
        return 0.0;
    }
    let token1_value_in_token0 = amount1 / price_token1_per_token0;
    let total_value_in_token0 = amount0 + token1_value_in_token0;
    if total_value_in_token0 <= 0.0 {
        return 0.0;
    }
    match side {
        RiskTokenSide::Token0 => amount0 / total_value_in_token0,
        RiskTokenSide::Token1 => token1_value_in_token0 / total_value_in_token0,
    }
}

fn raw_u128_to_human(value: u128, decimals: u8) -> f64 {
    value as f64 / 10f64.powi(decimals as i32)
}

#[derive(Debug, Clone, Copy)]
struct NpmPositionSnapshot {
    tick_lower: i32,
    tick_upper: i32,
    liquidity: u128,
}

#[derive(Debug, Clone)]
struct NpmPositionDetail {
    token0: String,
    token1: String,
    tick_spacing: i32,
    tick_lower: i32,
    tick_upper: i32,
    liquidity: u128,
    fee_growth_inside0_last_x128: String,
    fee_growth_inside1_last_x128: String,
    tokens_owed0: u128,
    tokens_owed1: u128,
}

async fn read_npm_position(
    rpc: &JsonRpcClient,
    npm: &str,
    token_id: u64,
) -> Result<NpmPositionSnapshot> {
    let detail = read_npm_position_detail(rpc, npm, token_id).await?;

    Ok(NpmPositionSnapshot {
        tick_lower: detail.tick_lower,
        tick_upper: detail.tick_upper,
        liquidity: detail.liquidity,
    })
}

async fn read_npm_position_detail(
    rpc: &JsonRpcClient,
    npm: &str,
    token_id: u64,
) -> Result<NpmPositionDetail> {
    let calldata = format!(
        "0x{}{}",
        autopool_evm::abi::selector("positions(uint256)"),
        autopool_evm::abi::enc_uint(token_id as u128)
    );
    let raw = rpc.eth_call(npm, &calldata).await?;
    let words = abi_words(&raw).context("positions(tokenId) returned malformed ABI")?;
    let token0 = decode_address_abi_word(
        words
            .get(2)
            .context("positions(tokenId) missing token0 word")?,
    )?;
    let token1 = decode_address_abi_word(
        words
            .get(3)
            .context("positions(tokenId) missing token1 word")?,
    )?;
    let tick_spacing = decode_i24_abi_word(
        words
            .get(4)
            .context("positions(tokenId) missing tickSpacing word")?,
    )?;
    let tick_lower = decode_i24_abi_word(
        words
            .get(5)
            .context("positions(tokenId) missing tickLower word")?,
    )?;
    let tick_upper = decode_i24_abi_word(
        words
            .get(6)
            .context("positions(tokenId) missing tickUpper word")?,
    )?;
    let liquidity = decode_u128_abi_word(
        words
            .get(7)
            .context("positions(tokenId) missing liquidity word")?,
    )?;
    let fee_growth_inside0_last_x128 = words
        .get(8)
        .map(|word| format!("0x{word}"))
        .unwrap_or_else(|| "0x0".to_string());
    let fee_growth_inside1_last_x128 = words
        .get(9)
        .map(|word| format!("0x{word}"))
        .unwrap_or_else(|| "0x0".to_string());
    let tokens_owed0 = words
        .get(10)
        .map(|word| decode_u128_abi_word(word))
        .transpose()?
        .unwrap_or(0);
    let tokens_owed1 = words
        .get(11)
        .map(|word| decode_u128_abi_word(word))
        .transpose()?
        .unwrap_or(0);

    Ok(NpmPositionDetail {
        token0,
        token1,
        tick_spacing,
        tick_lower,
        tick_upper,
        liquidity,
        fee_growth_inside0_last_x128,
        fee_growth_inside1_last_x128,
        tokens_owed0,
        tokens_owed1,
    })
}

fn decode_address_abi_word(word: &str) -> Result<String> {
    Ok(format!("0x{}", &word[word.len().saturating_sub(40)..]))
}

fn decode_i24_abi_word(word: &str) -> Result<i32> {
    let lower = &word[word.len().saturating_sub(6)..];
    let raw = i32::from_str_radix(lower, 16)?;
    Ok(if raw & 0x80_0000 != 0 {
        raw - 0x100_0000
    } else {
        raw
    })
}

fn decode_u128_abi_word(word: &str) -> Result<u128> {
    Ok(u128::from_str_radix(
        &word[word.len().saturating_sub(32)..],
        16,
    )?)
}

async fn read_owner_of(rpc: &JsonRpcClient, npm: &str, token_id: u64) -> Result<String> {
    let calldata = format!(
        "0x{}{}",
        autopool_evm::abi::selector("ownerOf(uint256)"),
        autopool_evm::abi::enc_uint(token_id as u128)
    );
    let raw = rpc.eth_call(npm, &calldata).await?;
    let words = abi_words(&raw).context("ownerOf(tokenId) returned malformed ABI")?;
    decode_address_abi_word(
        words
            .first()
            .context("ownerOf(tokenId) missing owner word")?,
    )
}

async fn read_pool_gauge(rpc: &JsonRpcClient, pool_address: &str) -> Result<Option<String>> {
    let calldata = format!("0x{}", autopool_evm::abi::selector("gauge()"));
    let raw = match rpc.eth_call(pool_address, &calldata).await {
        Ok(raw) => raw,
        Err(_) => return Ok(None),
    };
    let words = abi_words(&raw).context("gauge() returned malformed ABI")?;
    let gauge = decode_address_abi_word(words.first().context("gauge() missing address word")?)?;
    if gauge.eq_ignore_ascii_case("0x0000000000000000000000000000000000000000") {
        Ok(None)
    } else {
        Ok(Some(gauge))
    }
}

async fn read_erc20_decimals(rpc: &JsonRpcClient, token: &str) -> Result<Option<u8>> {
    let calldata = format!("0x{}", autopool_evm::abi::selector("decimals()"));
    let raw = match rpc.eth_call(token, &calldata).await {
        Ok(raw) => raw,
        Err(_) => return Ok(None),
    };
    Ok(parse_hex_u64_lossy(&raw).and_then(|value| u8::try_from(value).ok()))
}

#[derive(Debug, Clone, Copy)]
struct PositionRiskOptions {
    token0_usd: Option<f64>,
    risk_token_side: RiskTokenSide,
    max_risk_token_share: f64,
    min_distance_to_edge_ticks: i32,
    max_owed_value_usd: Option<f64>,
}

impl Default for PositionRiskOptions {
    fn default() -> Self {
        Self {
            token0_usd: None,
            risk_token_side: RiskTokenSide::Token1,
            max_risk_token_share: 0.8,
            min_distance_to_edge_ticks: 120,
            max_owed_value_usd: None,
        }
    }
}

async fn build_position_snapshot(
    rpc: &JsonRpcClient,
    token_id: u64,
    pool_address: Option<&str>,
    risk: PositionRiskOptions,
) -> Result<serde_json::Value> {
    let c = BASE_SLIPSTREAM_GAUGES_V3;
    let npm = c.nonfungible_position_manager;
    let owner = read_owner_of(rpc, npm, token_id).await?;
    let position = read_npm_position_detail(rpc, npm, token_id).await?;
    let decimals0 = read_erc20_decimals(rpc, &position.token0).await?;
    let decimals1 = read_erc20_decimals(rpc, &position.token1).await?;
    let resolved_pool = match pool_address {
        Some(address) => Some(address.to_string()),
        None => {
            rpc.get_cl_pool(
                c.pool_factory,
                &position.token0,
                &position.token1,
                position.tick_spacing,
            )
            .await?
        }
    };

    let mut current_tick: Option<i32> = None;
    let mut tick_spacing_live: Option<i32> = None;
    let mut in_range: Option<bool> = None;
    let mut fee_bps: Option<f64> = None;
    let mut gauge: Option<String> = None;
    let mut sqrt_price_x96_hex: Option<String> = None;
    let mut pool_liquidity_raw: Option<String> = None;

    if let Some(pool) = resolved_pool.as_deref() {
        let state = rpc.read_cl_pool_state(pool).await?;
        current_tick = Some(state.current_tick);
        tick_spacing_live = Some(state.tick_spacing);
        sqrt_price_x96_hex = Some(state.sqrt_price_x96_hex.clone());
        pool_liquidity_raw = Some(state.liquidity.clone());
        in_range = Some(
            state.current_tick >= position.tick_lower && state.current_tick < position.tick_upper,
        );
        fee_bps = rpc
            .eth_call(pool, "0xddca3f43")
            .await
            .ok()
            .and_then(|hex| parse_hex_u64_lossy(&hex))
            .map(|value| value as f64 / 100.0);
        gauge = read_pool_gauge(&rpc, pool).await?;
    }

    let appears_staked = gauge
        .as_deref()
        .map(|value| owner.eq_ignore_ascii_case(value))
        .unwrap_or(false);
    let range_width_ticks = position.tick_upper - position.tick_lower;
    let distance_to_lower = current_tick.map(|tick| tick - position.tick_lower);
    let distance_to_upper = current_tick.map(|tick| position.tick_upper - tick);
    let price_token1_per_token0 = match (&sqrt_price_x96_hex, decimals0, decimals1) {
        (Some(sqrt_hex), Some(decimals0), Some(decimals1)) => {
            let sqrt_x96 = hex_word_to_f64(sqrt_hex);
            let raw_price = (sqrt_x96 / 79_228_162_514_264_337_593_543_950_336.0).powi(2);
            Some(raw_price * 10f64.powi(decimals0 as i32 - decimals1 as i32))
        }
        _ => None,
    };
    let (amount0, amount1) = match (&sqrt_price_x96_hex, decimals0, decimals1) {
        (Some(sqrt_hex), Some(decimals0), Some(decimals1)) => {
            let sqrt_x96 = hex_word_to_f64(sqrt_hex);
            let (amount0, amount1) = autopool_backtest::cl_position_amounts(
                decimals0,
                decimals1,
                position.tick_lower,
                position.tick_upper,
                sqrt_x96,
                position.liquidity as f64,
            );
            (Some(amount0), Some(amount1))
        }
        _ => (None, None),
    };
    let owed0 = decimals0.map(|decimals| raw_u128_to_human(position.tokens_owed0, decimals));
    let owed1 = decimals1.map(|decimals| raw_u128_to_human(position.tokens_owed1, decimals));
    let token1_usd = match (risk.token0_usd, price_token1_per_token0) {
        (Some(token0_usd), Some(price)) if price > 0.0 => Some(token0_usd / price),
        _ => None,
    };
    let amount0_usd = amount0
        .zip(risk.token0_usd)
        .map(|(amount, usd)| amount * usd);
    let amount1_usd = amount1.zip(token1_usd).map(|(amount, usd)| amount * usd);
    let position_value_usd = match (amount0_usd, amount1_usd) {
        (Some(amount0_usd), Some(amount1_usd)) => Some(amount0_usd + amount1_usd),
        _ => None,
    };
    let owed0_usd = owed0.zip(risk.token0_usd).map(|(amount, usd)| amount * usd);
    let owed1_usd = owed1.zip(token1_usd).map(|(amount, usd)| amount * usd);
    let owed_value_usd = match (owed0_usd, owed1_usd) {
        (Some(owed0_usd), Some(owed1_usd)) => Some(owed0_usd + owed1_usd),
        _ => None,
    };
    let risk_token_share = match (amount0, amount1, price_token1_per_token0) {
        (Some(amount0), Some(amount1), Some(price)) => Some(risk_token_inventory_share(
            amount0,
            amount1,
            price,
            risk.risk_token_side,
        )),
        _ => None,
    };
    let mut alerts = Vec::<String>::new();
    let mut kill_switch_reasons = Vec::<String>::new();
    if in_range == Some(false) {
        alerts.push("out_of_range".to_string());
        kill_switch_reasons.push("position_out_of_range".to_string());
    }
    if let (Some(lower), Some(upper)) = (distance_to_lower, distance_to_upper) {
        if lower.min(upper) <= risk.min_distance_to_edge_ticks {
            alerts.push("near_range_edge".to_string());
        }
    }
    if let Some(share) = risk_token_share {
        if share > risk.max_risk_token_share {
            alerts.push("risk_token_share_exceeded".to_string());
            kill_switch_reasons.push("risk_token_share_exceeded".to_string());
        }
    }
    if let (Some(owed_value), Some(limit)) = (owed_value_usd, risk.max_owed_value_usd) {
        if owed_value > limit {
            alerts.push("owed_value_above_collection_threshold".to_string());
        }
    }
    alerts.sort();
    alerts.dedup();
    kill_switch_reasons.sort();
    kill_switch_reasons.dedup();
    let kill_switch_triggered = !kill_switch_reasons.is_empty();
    let result = json!({
        "token_id": token_id,
        "npm": npm,
        "owner": owner,
        "pool": resolved_pool,
        "gauge": gauge,
        "appears_staked": appears_staked,
        "token0": position.token0,
        "token1": position.token1,
        "decimals0": decimals0,
        "decimals1": decimals1,
        "tick_spacing_position": position.tick_spacing,
        "tick_spacing_pool": tick_spacing_live,
        "tick_lower": position.tick_lower,
        "tick_upper": position.tick_upper,
        "range_width_ticks": range_width_ticks,
        "current_tick": current_tick,
        "sqrt_price_x96": sqrt_price_x96_hex,
        "pool_liquidity_raw": pool_liquidity_raw,
        "price_token1_per_token0": price_token1_per_token0,
        "in_range": in_range,
        "distance_to_lower_ticks": distance_to_lower,
        "distance_to_upper_ticks": distance_to_upper,
        "liquidity_raw": position.liquidity.to_string(),
        "amount0": amount0,
        "amount1": amount1,
        "token0_usd": risk.token0_usd,
        "token1_usd": token1_usd,
        "amount0_usd": amount0_usd,
        "amount1_usd": amount1_usd,
        "position_value_usd": position_value_usd,
        "tokens_owed0_raw": position.tokens_owed0.to_string(),
        "tokens_owed1_raw": position.tokens_owed1.to_string(),
        "tokens_owed0": owed0,
        "tokens_owed1": owed1,
        "tokens_owed0_usd": owed0_usd,
        "tokens_owed1_usd": owed1_usd,
        "owed_value_usd": owed_value_usd,
        "fee_growth_inside0_last_x128": position.fee_growth_inside0_last_x128,
        "fee_growth_inside1_last_x128": position.fee_growth_inside1_last_x128,
        "fee_bps": fee_bps,
        "risk_token_side": risk.risk_token_side.label(),
        "risk_token_share": risk_token_share,
        "max_risk_token_share": risk.max_risk_token_share,
        "min_distance_to_edge_ticks": risk.min_distance_to_edge_ticks,
        "max_owed_value_usd": risk.max_owed_value_usd,
        "alerts": alerts,
        "kill_switch_triggered": kill_switch_triggered,
        "kill_switch_reasons": kill_switch_reasons,
        "requires_signature": false,
        "broadcast": false
    });
    Ok(result)
}

async fn inspect_position(
    rpc_url: String,
    token_id: u64,
    pool_address: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let rpc = JsonRpcClient::new(rpc_url);
    let result = build_position_snapshot(
        &rpc,
        token_id,
        pool_address.as_deref(),
        PositionRiskOptions::default(),
    )
    .await?;

    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    println!("position #{token_id} (read-only)");
    println!("  npm        : {}", json_str(&result, "npm"));
    println!("  owner      : {}", result["owner"].as_str().unwrap_or(""));
    if let Some(pool) = result["pool"].as_str() {
        println!("  pool       : {pool}");
    } else {
        println!("  pool       : unresolved");
    }
    if let Some(gauge) = result["gauge"].as_str() {
        println!("  gauge      : {gauge}");
    }
    println!("  staked?    : {}", json_bool(&result, "appears_staked"));
    println!("  token0     : {}", result["token0"].as_str().unwrap_or(""));
    println!("  token1     : {}", result["token1"].as_str().unwrap_or(""));
    println!(
        "  range      : [{}, {}) width={} ticks spacing={}",
        json_i64(&result, "tick_lower"),
        json_i64(&result, "tick_upper"),
        json_i64(&result, "range_width_ticks"),
        json_i64(&result, "tick_spacing_position")
    );
    if let Some(tick) = result["current_tick"].as_i64() {
        println!(
            "  current    : tick={} in_range={} dist_lower={} dist_upper={}",
            tick,
            json_optional_bool(&result, "in_range"),
            json_optional_i64(&result, "distance_to_lower_ticks"),
            json_optional_i64(&result, "distance_to_upper_ticks")
        );
    }
    if let Some(fee) = result["fee_bps"].as_f64() {
        println!("  fee_bps    : {fee:.2}");
    }
    println!("  liquidity  : {}", json_str(&result, "liquidity_raw"));
    println!("  owed0_raw  : {}", json_str(&result, "tokens_owed0_raw"));
    println!("  owed1_raw  : {}", json_str(&result, "tokens_owed1_raw"));
    println!("  broadcast  : false");

    Ok(())
}

struct PositionMonitorArgs {
    rpc_url: String,
    token_id: u64,
    pool_address: Option<String>,
    output: PathBuf,
    poll_seconds: u64,
    iterations: u64,
    risk: PositionRiskOptions,
    format: OutputFormat,
}

async fn monitor_position(args: PositionMonitorArgs) -> Result<()> {
    if let Some(parent) = args
        .output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating monitor output directory {}", parent.display()))?;
    }
    let rpc = JsonRpcClient::new(args.rpc_url);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.output)
        .with_context(|| format!("opening monitor output {}", args.output.display()))?;

    let mut iteration = 0_u64;
    loop {
        if args.iterations != 0 && iteration >= args.iterations {
            break;
        }
        iteration += 1;
        let mut snapshot =
            build_position_snapshot(&rpc, args.token_id, args.pool_address.as_deref(), args.risk)
                .await?;
        if let Some(object) = snapshot.as_object_mut() {
            object.insert("monitor_sequence".to_string(), json!(iteration));
            object.insert(
                "observed_at_unix".to_string(),
                json!(current_unix_timestamp()),
            );
            object.insert(
                "monitor_output".to_string(),
                json!(args.output.display().to_string()),
            );
        }
        writeln!(file, "{}", serde_json::to_string(&snapshot)?)?;
        file.flush()?;

        match args.format {
            OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&snapshot)?),
            OutputFormat::Table => println!(
                "snapshot #{iteration} token_id={} tick={} in_range={} value_usd={} risk_share={} alerts={} kill_switch={} -> {}",
                args.token_id,
                json_optional_i64(&snapshot, "current_tick"),
                json_optional_bool(&snapshot, "in_range"),
                json_optional_f64(&snapshot, "position_value_usd"),
                json_optional_pct(&snapshot, "risk_token_share"),
                json_string_array(&snapshot, "alerts"),
                json_optional_bool(&snapshot, "kill_switch_triggered"),
                args.output.display()
            ),
        }

        if args.iterations != 0 && iteration >= args.iterations {
            break;
        }
        tokio::time::sleep(Duration::from_secs(args.poll_seconds)).await;
    }

    Ok(())
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

    // Read the pool's tokens so we can build real, signable calldata.
    let read_addr = |hex: &str| -> String {
        let w = hex.trim_start_matches("0x");
        format!("0x{}", &w[w.len().saturating_sub(40)..])
    };
    let token0 = read_addr(&rpc.eth_call(&args.pool_address, "0x0dfe1681").await?);
    let token1 = read_addr(&rpc.eth_call(&args.pool_address, "0xd21220a7").await?);
    // Owner/recipient + deadline baked into the (unsigned) calldata.
    let recipient = args.recipient.as_str();
    let deadline = current_unix_timestamp() + 1200;
    let c = BASE_SLIPSTREAM_GAUGES_V3;
    let npm_position = match args.token_id {
        Some(token_id) => {
            Some(read_npm_position(&rpc, c.nonfungible_position_manager, token_id).await?)
        }
        None => None,
    };

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
    // Current inventory + liquidity: from the old band if rebalancing, else all token0.
    let mut current_liquidity = 0.0_f64;
    let mut current_liquidity_raw: Option<u128> = None;
    let current_lower = args
        .current_lower
        .or_else(|| npm_position.as_ref().map(|position| position.tick_lower));
    let current_upper = args
        .current_upper
        .or_else(|| npm_position.as_ref().map(|position| position.tick_upper));
    let (c0, c1) = match (current_lower, current_upper) {
        (Some(cl), Some(cu)) => {
            let (mut a0, mut a1, liq) = autopool_backtest::cl_mint_amounts(
                args.decimals0,
                args.decimals1,
                cl,
                cu,
                sqrt_x96,
                capital_token0,
            );
            if let Some(position) = npm_position {
                current_liquidity = position.liquidity as f64;
                current_liquidity_raw = Some(position.liquidity);
                if liq > 0.0 {
                    let scale = current_liquidity / liq;
                    a0 *= scale;
                    a1 *= scale;
                }
            } else {
                current_liquidity = liq;
                current_liquidity_raw = Some(liq.round() as u128);
            }
            (a0, a1)
        }
        _ => (capital_token0, 0.0),
    };

    let price_human = {
        let p_raw = (sqrt_x96 / 79_228_162_514_264_337_593_543_950_336.0).powi(2);
        p_raw * 10f64.powi(args.decimals0 as i32 - args.decimals1 as i32)
    };
    let slip = args.slippage_bps / 10_000.0;
    let target_ratio_1_per_0 = if t0 > 0.0 { t1 / t0 } else { f64::INFINITY };

    // Slipstream/v3 deployment contracts (verify the pool's deployment matches).
    let rebalancing = current_lower.is_some() && current_upper.is_some();
    let mut actions: Vec<serde_json::Value> = Vec::new();

    if rebalancing && args.staked {
        actions.push(json!({"step":"unstake","contract":c.gauge_factory,"call":"gauge.withdraw(tokenId)","note":"emissions require staking; must unstake before modifying"}));
    }
    if rebalancing {
        actions.push(json!({"step":"collect","contract":c.nonfungible_position_manager,"call":"collect(tokenId, max, max)","expects":"pre-decrease fees out"}));
        actions.push(json!({"step":"decreaseLiquidity","contract":c.nonfungible_position_manager,"call":"decreaseLiquidity(tokenId, liquidity, amount0Min, amount1Min, deadline)","amount0_out_est":c0,"amount1_out_est":c1,"amount0Min":c0*(1.0-slip),"amount1Min":c1*(1.0-slip)}));
        actions.push(json!({"step":"collectWithdrawn","contract":c.nonfungible_position_manager,"call":"collect(tokenId, max, max)","expects":"principal + fees owed after decreaseLiquidity"}));
        actions.push(
            json!({"step":"burn","contract":c.nonfungible_position_manager,"call":"burn(tokenId)","note":"optional cleanup after the executable rebalance; not included in the main NPM multicall"}),
        );
    }

    // Swap to reach the target ratio, simulated against the pool's real in-range
    // liquidity (so expected_out and price impact reflect actual depth, not just mid).
    let pool_liquidity = state.liquidity.parse::<f64>().unwrap_or(0.0);
    let fee_fraction = fee_bps / 10_000.0;
    let mut swap_impact_bps = 0.0_f64;
    let (mut mint0, mut mint1) = fit_to_range_ratio(c0, c1, target_ratio_1_per_0);
    if let Some(swap_plan) = solve_inventory_swap(
        sqrt_x96,
        pool_liquidity,
        fee_fraction,
        args.decimals0,
        args.decimals1,
        c0,
        c1,
        target_ratio_1_per_0,
    ) {
        let (tin, tout, dec_in, dec_out) = if swap_plan.zero_for_one {
            (&token0, &token1, args.decimals0, args.decimals1)
        } else {
            (&token1, &token0, args.decimals1, args.decimals0)
        };
        let amount_in = swap_plan.amount_in;
        let amount_in_raw = amount_in * 10f64.powi(dec_in as i32);
        let amount_in_u128 = amount_in_raw.round() as u128;
        let expected_out = swap_plan.expected_out;
        swap_impact_bps = swap_plan.price_impact_bps;
        let amount_out_min = (expected_out * (1.0 - slip)).max(0.0);
        let amount_out_min_raw = (amount_out_min * 10f64.powi(dec_out as i32)).round() as u128;
        let (safe0, safe1) = if swap_plan.zero_for_one {
            (c0 - amount_in, c1 + amount_out_min)
        } else {
            (c0 + amount_out_min, c1 - amount_in)
        };
        (mint0, mint1) = fit_to_range_ratio(
            safe0 * (1.0 - slip),
            safe1 * (1.0 - slip),
            target_ratio_1_per_0,
        );
        // Best-effort real on-chain quote via the Quoter (read-only eth_call).
        let onchain_out = if args.skip_quoter {
            json!("skipped")
        } else {
            match rpc
                .quote_exact_input_single(c.mixed_quoter, tin, tout, spacing, amount_in_u128)
                .await
            {
                Ok(out_raw) => json!(out_raw as f64 / 10f64.powi(dec_out as i32)),
                Err(e) => json!(format!("quoter unavailable: {e}")),
            }
        };
        let calldata = autopool_evm::encode_exact_input_single(
            tin,
            tout,
            spacing,
            recipient,
            deadline,
            amount_in_u128,
            amount_out_min_raw,
        );
        actions.push(json!({
            "step":"swap","contract":c.swap_router,
            "call":format!("exactInputSingle({} -> {})", tin, tout),
            "amount_in":amount_in,"expected_out_sim":expected_out,"onchain_quote_out":onchain_out,
            "amountOutMin":amount_out_min,"price_impact_bps":swap_plan.price_impact_bps,
            "calldata":calldata
        }));
    }
    let mint_calldata = autopool_evm::encode_mint(
        &token0,
        &token1,
        spacing,
        lower,
        upper,
        (mint0 * 10f64.powi(args.decimals0 as i32)).round() as u128,
        (mint1 * 10f64.powi(args.decimals1 as i32)).round() as u128,
        (mint0 * (1.0 - slip) * 10f64.powi(args.decimals0 as i32)).round() as u128,
        (mint1 * (1.0 - slip) * 10f64.powi(args.decimals1 as i32)).round() as u128,
        recipient,
        deadline,
    );
    actions.push(json!({"step":"mint","contract":c.nonfungible_position_manager,"call":"mint(...)","tickLower":lower,"tickUpper":upper,"amount0Desired":mint0,"amount1Desired":mint1,"amount0Min":mint0*(1.0-slip),"amount1Min":mint1*(1.0-slip),"calldata":mint_calldata.clone()}));
    if args.staked {
        actions.push(json!({"step":"stake","contract":c.gauge_factory,"call":"gauge.deposit(tokenId)","note":"stake the new NFT to earn emissions"}));
    }

    // Bundle the executable NPM-side calls into one real multicall calldata. For
    // rebalancing, decreaseLiquidity only records owed tokens; the second collect
    // transfers withdrawn principal into the wallet before the new mint pulls it.
    // Burning the empty old NFT is a separate cleanup because Slipstream's NPM can
    // reject burn in the same path even after collect has transferred principal.
    let mut npm_calls: Vec<String> = Vec::new();
    if rebalancing {
        if let Some(tid) = args.token_id {
            let tid = tid as u128;
            npm_calls.push(autopool_evm::encode_collect(tid, recipient));
            npm_calls.push(autopool_evm::encode_decrease_liquidity(
                tid,
                current_liquidity_raw.unwrap_or_else(|| current_liquidity.round() as u128),
                (c0 * (1.0 - slip) * 10f64.powi(args.decimals0 as i32)).round() as u128,
                (c1 * (1.0 - slip) * 10f64.powi(args.decimals1 as i32)).round() as u128,
                deadline,
            ));
            npm_calls.push(autopool_evm::encode_collect(tid, recipient));
        }
    }
    npm_calls.push(mint_calldata);
    let npm_multicall = autopool_evm::abi::encode_multicall(&npm_calls);

    // Risk gates.
    let gas_usd = net.estimated_rebalance_gas_usd.unwrap_or(0.0);
    let risk_token_share =
        risk_token_inventory_share(mint0, mint1, price_human, args.risk_token_side);
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
        risk_token_share <= args.max_risk_token_share,
        format!(
            "risk-token ({}) share {:.0}% <= {:.0}%",
            args.risk_token_side.label(),
            risk_token_share * 100.0,
            args.max_risk_token_share * 100.0
        ),
    ));
    gates.push((
        "slippage_bounded".into(),
        args.slippage_bps <= 100.0,
        format!("max slippage {:.0} bps", args.slippage_bps),
    ));
    gates.push((
        "swap_price_impact".into(),
        swap_impact_bps <= args.slippage_bps,
        format!(
            "rebalance swap impact {:.1} bps <= slippage {:.0} bps",
            swap_impact_bps, args.slippage_bps
        ),
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
        "risk_token_side": args.risk_token_side,
        "max_risk_token_share": args.max_risk_token_share,
        "risk_token_share": risk_token_share,
        "target_range": {"lower": lower, "upper": upper, "liquidity_est": target_liq},
        "actions": actions,
        "npm_multicall": {"contract": c.nonfungible_position_manager, "calls": npm_calls.len(), "calldata": npm_multicall},
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
        "target band: [{lower}, {upper}]  desired token0={mint0:.6} token1={mint1:.6}  ({})",
        if rebalancing {
            "rebalance"
        } else {
            "fresh mint"
        }
    );
    println!(
        "risk inventory: {} share {:.1}% (limit {:.1}%)",
        args.risk_token_side.label(),
        risk_token_share * 100.0,
        args.max_risk_token_share * 100.0
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
        "decision: {} (proposal only; validate executable calldata with scripts/fork-sim-rebalance.sh before any guarded signing path)",
        if all_pass {
            "GATES PASS — would propose"
        } else {
            "REJECTED by risk gates"
        }
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

fn percentile_f64(values: &[f64], percentile: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let index = (sorted.len() - 1) as f64 * percentile / 100.0;
    let lower = index.floor() as usize;
    let upper = index.ceil() as usize;
    if lower == upper {
        Some(sorted[lower])
    } else {
        let weight_upper = index - lower as f64;
        let weight_lower = 1.0 - weight_upper;
        Some(sorted[lower] * weight_lower + sorted[upper] * weight_upper)
    }
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

fn optional_pct(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.0}%"))
        .unwrap_or_else(|| "-".to_string())
}

fn json_str(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or("-")
        .to_string()
}

fn json_bool(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|value| value.as_bool())
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn json_i64(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|value| value.as_i64())
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn json_optional_i64(value: &serde_json::Value, key: &str) -> String {
    json_i64(value, key)
}

fn json_optional_bool(value: &serde_json::Value, key: &str) -> String {
    json_bool(value, key)
}

fn json_optional_f64(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|value| value.as_f64())
        .map(|value| format!("{value:.4}"))
        .unwrap_or_else(|| "-".to_string())
}

fn json_optional_pct(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|value| value.as_f64())
        .map(|value| format!("{:.1}%", value * 100.0))
        .unwrap_or_else(|| "-".to_string())
}

fn json_string_array(value: &serde_json::Value, key: &str) -> String {
    let values = value
        .get(key)
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if values.is_empty() {
        "-".to_string()
    } else {
        values.join(",")
    }
}

fn current_unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_token_inventory_share_respects_token_side() {
        let token0 = 2.0;
        let token1 = 400.0;
        let price_token1_per_token0 = 200.0;

        let token0_share = risk_token_inventory_share(
            token0,
            token1,
            price_token1_per_token0,
            RiskTokenSide::Token0,
        );
        let token1_share = risk_token_inventory_share(
            token0,
            token1,
            price_token1_per_token0,
            RiskTokenSide::Token1,
        );

        assert!((token0_share - 0.5).abs() < 1e-9);
        assert!((token1_share - 0.5).abs() < 1e-9);
        assert!((token0_share + token1_share - 1.0).abs() < 1e-9);
    }

    #[test]
    fn risk_token_inventory_share_rejects_invalid_price() {
        assert_eq!(
            risk_token_inventory_share(1.0, 1.0, 0.0, RiskTokenSide::Token1),
            0.0
        );
    }

    #[test]
    fn hot_pool_formula_apr_matches_fee_times_turnover() {
        let apr = formula_fee_apr_from_volume_tvl(2.0, 100.0);
        assert!((apr - 730.0).abs() < 1e-9);
        let required = required_volume_tvl_for_apr(2_000.0, 100.0).unwrap();
        assert!((required - 5.47945205479452).abs() < 1e-9);
    }

    #[test]
    fn hot_pool_spec_inverts_when_stable_is_token1() {
        let candidate = hot_pool_candidate_fixture("raydium", "clmm", Some(60), None, vec![]);
        let spec = replay_spec_from_candidate(&candidate, &hot_pool_replay_model(&candidate));

        assert_eq!(spec.replay_model, "clmm_tick_replay");
        assert!(spec.invert_for_numeraire);
        assert_eq!(spec.token0_usd, Some(1.0));
        assert_eq!(spec.risk_token_side, "token1");
    }

    #[test]
    fn hot_pool_plan_blocks_meteora_dlmm_from_tick_replay() {
        let candidate = hot_pool_candidate_fixture(
            "meteora",
            "dlmm",
            None,
            Some(20),
            vec!["meteora_daily_ratio_disagrees_with_apy".to_string()],
        );
        let model = hot_pool_replay_model(&candidate);
        let requirements = hot_pool_data_requirements(&candidate, &model, 0, &[]);
        let status = hot_pool_experiment_status(&candidate, &model, 0, &requirements, &[]);

        assert_eq!(model, "dlmm_bin_replay");
        assert_eq!(status, "blocked_needs_bin_replay");
        assert!(requirements.contains(&"dlmm_bin_replay_engine".to_string()));
    }

    #[test]
    fn proxy_replay_caps_inventory_drawdown() {
        let candidate = solana_pool_candidate_fixture(117.0, Some(1_852.0));
        let drawdown = inventory_drawdown_proxy_pct(Some(1_852.0), 10.0, &candidate);
        assert_eq!(drawdown, 100.0);
    }

    #[test]
    fn proxy_replay_estimates_narrow_hot_pool_edge() {
        let candidate = solana_pool_candidate_fixture(218.8, Some(26.6));
        let args = proxy_args_fixture();
        let scenario = solana_proxy_scenario(&candidate, 2.5, Some(26.6), &args);

        assert!(scenario.estimated_concentration > 4.0);
        assert!(scenario.net_fee_apr_proxy > 900.0);
        assert_eq!(scenario.risk_grade, "medium");
        assert_eq!(scenario.verdict, "candidate_replay");
    }

    #[test]
    fn decodes_raydium_clmm_swap_event() {
        let event = decode_raydium_clmm_swap_event(
            "QMbN6CYIceL5cEcIwJ0doJ606WjQpCy4c1fKQWkhV+rJ9GWEcQ0Beqx+qMoro1HpabFjovKAi6Xg2hc3AOJa/INrzLINjURgJ+pZMQTN1cdziJMBFtZsa/jPU5cifiYm349rv58fllzmMP246IpMm+Wt1jXLWP5a94u9i6w4S5ssqbnw+uUHH97IKB0AAAAAAAAAAAAAAAAHhCcHAAAAAAAAAAAAAAAAAMHWr0ETYI1+AAAAAAAAAACpeeHOew0AAAAAAAAAAAAA9Mj//wAAAAAAAAAAeVMHAAAAAAA=",
            "HnhpJPJgBG2KwniMTNW8cVBHvk1hFog3RC3kjnyc23tD",
        )
        .expect("valid Raydium swap event");

        assert_eq!(
            event.pool_state,
            "HnhpJPJgBG2KwniMTNW8cVBHvk1hFog3RC3kjnyc23tD"
        );
        assert_eq!(event.amount_0, 489_212_126);
        assert_eq!(event.amount_1, 120_030_215);
        assert!(!event.zero_for_one);
        assert_eq!(event.sqrt_price_x64, 9_119_050_456_317_810_369);
        assert_eq!(event.liquidity, 14_825_403_021_737);
        assert_eq!(event.tick, -14_092);
        assert_eq!(event.trade_fee_1, 480_121);

        let preview = raydium_event_to_swap_obs_preview(&event);
        assert_eq!(preview.amount0, -489_212_126.0);
        assert_eq!(preview.amount1, 120_030_215.0);
        assert_eq!(preview.tick, -14_092);
    }

    #[test]
    fn raydium_matching_skips_routed_swap_from_other_pool() {
        let invocations = vec![
            SolanaProgramSwapInvocation {
                instruction: "Swap".to_string(),
                program_data_base64: vec![
                    "QMbN6CYIceLQlIab3eXILQlyo9aqQY3bkiekql1sLmkJya8CAmKcP9laNTE8ekSIS9fREtHki/5hop//6Ko66Xbj3oFZpS1ro73UDi422zGrqgUtijMcRl2LF39ZXfIplXkzkOUqkdC2YHy19tWHdGMRTa1dw1pJQJO2QjC99moyLrIur/RE5J24MxsAAAAAAAAAAAAAAABvxaoGAAAAAAAAAAAAAAAAALsOzY6MMMl+AAAAAAAAAAAoj3WFLQAAAAAAAAAAAAAAGcn//wAAAAAAAAAAxq4AAAAAAAA=".to_string(),
                ],
            },
            SolanaProgramSwapInvocation {
                instruction: "SwapV2".to_string(),
                program_data_base64: vec![
                    "QMbN6CYIceL5cEcIwJ0doJ606WjQpCy4c1fKQWkhV+rJ9GWEcQ0Beqx+qMoro1HpabFjovKAi6Xg2hc3AOJa/INrzLINjURgJ+pZMQTN1cdziJMBFtZsa/jPU5cifiYm349rv58fllzmMP246IpMm+Wt1jXLWP5a94u9i6w4S5ssqbnw+uUHH97IKB0AAAAAAAAAAAAAAAAHhCcHAAAAAAAAAAAAAAAAAMHWr0ETYI1+AAAAAAAAAACpeeHOew0AAAAAAAAAAAAA9Mj//wAAAAAAAAAAeVMHAAAAAAA=".to_string(),
                ],
            },
        ];
        let matches = matching_raydium_clmm_swap_events(
            &invocations,
            "HnhpJPJgBG2KwniMTNW8cVBHvk1hFog3RC3kjnyc23tD",
        );

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].0, "SwapV2");
        assert_eq!(
            matches[0].2.pool_state,
            "HnhpJPJgBG2KwniMTNW8cVBHvk1hFog3RC3kjnyc23tD"
        );
        assert_eq!(matches[0].2.tick, -14_092);
    }

    #[test]
    fn decodes_orca_whirlpool_traded_event() {
        let event = decode_orca_whirlpool_traded_event(
            "4cpJr5MroJagiLqrLe0uP/OPax1CHnJdDCxAWO8Yb0V1+iIh9IY5iQHs+XQLw0dZFAcAAAAAAAAAW1DyDBJGWRQHAAAAAAAAAMhwCAAAAAAACVumAQAAAAAAAAAAAAAAAAAAAAAAAAAAAwMAAAAAAABzAAAAAAAAAA==",
            "BofA2ViUSudPBTUms2KRuG6AHNeMawjNfwqTJDgx5BKW",
        )
        .expect("valid Orca Whirlpool traded event");

        assert_eq!(
            event.whirlpool,
            "BofA2ViUSudPBTUms2KRuG6AHNeMawjNfwqTJDgx5BKW"
        );
        assert!(event.a_to_b);
        assert_eq!(event.pre_sqrt_price, 130_593_490_572_689_078_764);
        assert_eq!(event.post_sqrt_price, 130_593_488_712_993_230_939);
        assert_eq!(event.input_amount, 553_160);
        assert_eq!(event.output_amount, 27_679_497);

        let preview = orca_event_to_swap_obs_preview(&event, 267_836_504_483_179.0);
        assert_eq!(preview.amount0, 553_160.0);
        assert_eq!(preview.amount1, -27_679_497.0);
        assert_eq!(preview.tick, 39_145);
        assert_eq!(preview.liquidity, 267_836_504_483_179.0);
    }

    #[test]
    fn sample_goal_can_require_normalized_rows() {
        let args = SampleSolanaPoolSwapsArgs {
            rpc_url: "http://localhost".to_string(),
            pool_address: "pool".to_string(),
            program_id: RAYDIUM_CLMM_PROGRAM_ID.to_string(),
            token0_mint: "token0".to_string(),
            token1_mint: "token1".to_string(),
            active_liquidity: None,
            limit: 10,
            signature_scan_limit: 100,
            max_signature_pages: 2,
            before_signature: None,
            min_normalized_swaps: Some(2),
            request_sleep_ms: 0,
            output: None,
            normalized_output: None,
            format: OutputFormat::Table,
        };
        let mut samples = Vec::new();
        samples.push(solana_sample_fixture(Vec::new()));
        samples.push(solana_sample_fixture(vec![SolanaSwapObsPreview {
            amount0: 1.0,
            amount1: -1.0,
            sqrt_price_x96: 1.0,
            liquidity: 1.0,
            tick: 1,
        }]));
        assert!(!sample_goal_reached(&args, &samples));
        samples.push(solana_sample_fixture(vec![SolanaSwapObsPreview {
            amount0: 2.0,
            amount1: -2.0,
            sqrt_price_x96: 2.0,
            liquidity: 2.0,
            tick: 2,
        }]));
        assert!(sample_goal_reached(&args, &samples));
    }

    #[test]
    fn classifies_window_regimes_from_tick_path() {
        let range = swaps_for_ticks(&[10, 11, 10, 9, 10, 10]);
        let volatile_range = swaps_for_ticks(&[0, 50, -45, 55, -50, 0]);
        let trend_up = swaps_for_ticks(&(0..40).map(|index| index * 10).collect::<Vec<_>>());
        let trend_down = swaps_for_ticks(&(0..40).map(|index| -index * 10).collect::<Vec<_>>());

        assert_eq!(classify_window_regime(&range).label, "range");
        assert_eq!(
            classify_window_regime(&volatile_range).label,
            "volatile_range"
        );
        assert_eq!(classify_window_regime(&trend_up).label, "trend_up_risk");
        assert_eq!(
            classify_window_regime(&trend_down).label,
            "trend_down_money"
        );
    }

    #[test]
    fn lagged_regime_rule_uses_prior_window_to_select_hedge() {
        let rule = RegimeHedgeRule {
            range_hedge_fraction: 0.75,
            volatile_hedge_fraction: 0.75,
            trend_money_hedge_fraction: 0.25,
            trend_risk_hedge_fraction: 1.0,
        };
        let rows = vec![
            (0.25, hedge_window_row(0, "range", 1.0, -1.0, 100.0, 4.0)),
            (0.75, hedge_window_row(0, "range", 2.0, -2.0, 200.0, 2.0)),
            (
                0.25,
                hedge_window_row(1, "trend_down_money", 3.0, -3.0, 300.0, 3.0),
            ),
            (
                0.75,
                hedge_window_row(1, "trend_down_money", 9.0, 7.0, 900.0, 1.0),
            ),
        ];

        let summary = summarize_lagged_regime_rule_rows(&rows, &rule);

        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].windows, 1);
        assert_eq!(summary[0].skipped_windows, 1);
        assert_eq!(summary[0].mean_net_pnl_usd, 9.0);
        assert_eq!(summary[0].mean_net_vs_hold_usd, 7.0);
        assert_eq!(summary[0].p05_net_apr_pct, Some(900.0));
    }

    #[test]
    fn lagged_policy_switch_uses_prior_window_to_select_policy() {
        let rule = RegimePolicyRule {
            range_policy: RegimeSwitchPolicy::DeltaHedged,
            volatile_policy: RegimeSwitchPolicy::HedgedWide,
            trend_money_policy: RegimeSwitchPolicy::HedgedWide,
            trend_risk_policy: RegimeSwitchPolicy::HedgedWide,
        };
        let rows = vec![
            policy_window_row(0, "range", "delta_hedged", 1.0, -1.0, 100.0, 4.0),
            policy_window_row(0, "range", "hedged_wide", 2.0, -2.0, 200.0, 2.0),
            policy_window_row(1, "trend_down_money", "delta_hedged", 9.0, 7.0, 900.0, 1.0),
            policy_window_row(1, "trend_down_money", "hedged_wide", 3.0, -3.0, 300.0, 3.0),
        ];

        let summary = summarize_lagged_policy_switch_rows(&rows, &rule);

        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].windows, 1);
        assert_eq!(summary[0].skipped_windows, 1);
        assert_eq!(summary[0].mean_net_pnl_usd, 9.0);
        assert_eq!(summary[0].mean_net_vs_hold_usd, 7.0);
        assert_eq!(summary[0].p05_net_apr_pct, Some(900.0));
    }

    #[test]
    fn normalized_swap_key_dedupes_across_local_log_indices() {
        let mut left = swap_obs_fixture(10, 1, 100);
        let mut right = left;
        right.log_index = 99;

        assert_eq!(normalized_swap_key(&left), normalized_swap_key(&right));

        left.amount0 = 2.0;
        assert_ne!(normalized_swap_key(&left), normalized_swap_key(&right));
    }

    #[test]
    fn parses_promotion_window_configs() {
        let configs =
            parse_promotion_window_configs(&["25:10:4".to_string(), "80:25:3".to_string()])
                .expect("valid configs");

        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].window_swaps, 25);
        assert_eq!(configs[0].step_swaps, 10);
        assert_eq!(configs[0].min_windows, 4);
        assert_eq!(configs[1].window_swaps, 80);
    }

    #[test]
    fn promotion_window_row_requires_left_tail_and_hold_edge() {
        let thresholds = PromotionGateThresholds {
            min_p05_net_apr_pct: 500.0,
            min_mean_vs_hold_usd: 0.0,
            min_win_rate_vs_hold_pct: 60.0,
            max_drawdown_pct: 0.05,
        };
        let config = PromotionWindowConfig {
            window_swaps: 40,
            step_swaps: 15,
            min_windows: 4,
        };
        let passing = promotion_window_row(
            config,
            hedge_rule_row_fixture(4, 75.0, 1.0, 600.0, 20.0),
            &thresholds,
            500.0,
        );
        let failing = promotion_window_row(
            config,
            hedge_rule_row_fixture(4, 75.0, -1.0, 600.0, 20.0),
            &thresholds,
            500.0,
        );

        assert!(passing.passed);
        assert!(!failing.passed);
        assert!(
            failing
                .reasons
                .iter()
                .any(|reason| reason.contains("mean_vs_hold"))
        );
    }

    #[test]
    fn promotion_window_row_from_summary_gates_defensive_policy() {
        let thresholds = PromotionGateThresholds {
            min_p05_net_apr_pct: 500.0,
            min_mean_vs_hold_usd: 0.0,
            min_win_rate_vs_hold_pct: 60.0,
            max_drawdown_pct: 0.05,
        };
        let config = PromotionWindowConfig {
            window_swaps: 15,
            step_swaps: 5,
            min_windows: 3,
        };
        let passing = promotion_window_row_from_summary(
            config,
            window_summary_fixture("hedged_wide", 4, 75.0, 1.0, 650.0, 20.0),
            &thresholds,
            500.0,
        );
        let failing = promotion_window_row_from_summary(
            config,
            window_summary_fixture("hedged_wide", 4, 75.0, 1.0, 106.0, 20.0),
            &thresholds,
            500.0,
        );

        assert!(passing.passed);
        assert!(!failing.passed);
        assert!(
            failing
                .reasons
                .iter()
                .any(|reason| reason.contains("p05_apr"))
        );
    }

    fn hedge_rule_row_fixture(
        windows: usize,
        win_rate_vs_hold_pct: f64,
        mean_net_vs_hold_usd: f64,
        p05_net_apr_pct: f64,
        worst_max_drawdown_usd: f64,
    ) -> HedgeGridRuleRow {
        HedgeGridRuleRow {
            rule: "lagged_regime_rule".to_string(),
            hedge_map: "range=1.00".to_string(),
            windows,
            skipped_windows: 1,
            win_rate_vs_hold_pct,
            mean_net_pnl_usd: 1.0,
            mean_net_vs_hold_usd,
            mean_net_apr_pct: Some(700.0),
            p05_net_apr_pct: Some(p05_net_apr_pct),
            worst_max_drawdown_usd,
        }
    }

    fn window_summary_fixture(
        policy: &str,
        windows: usize,
        win_rate_vs_hold_pct: f64,
        mean_net_vs_hold_usd: f64,
        p05_net_apr_pct: f64,
        worst_max_drawdown_usd: f64,
    ) -> WindowPolicySummary {
        WindowPolicySummary {
            policy: policy.to_string(),
            windows,
            win_rate_vs_hold_pct,
            mean_net_pnl_usd: 1.0,
            mean_net_vs_hold_usd,
            mean_fee_minus_lvr_usd: 1.0,
            mean_net_apr_pct: Some(700.0),
            p05_net_apr_pct: Some(p05_net_apr_pct),
            p50_net_apr_pct: Some(700.0),
            p95_net_apr_pct: Some(900.0),
            mean_fee_lvr_apr_pct: Some(500.0),
            worst_max_drawdown_usd,
            mean_rebalances: 0.0,
        }
    }

    fn swap_obs_fixture(block: u64, log_index: u64, tick: i32) -> autopool_backtest::SwapObs {
        autopool_backtest::SwapObs {
            block,
            log_index,
            amount0: 1.0,
            amount1: -1.0,
            sqrt_price_x96: 1.0,
            liquidity: 1.0,
            tick,
        }
    }

    fn hedge_window_row(
        window_index: usize,
        regime: &str,
        net_pnl_usd: f64,
        net_vs_hold_usd: f64,
        net_apr_pct: f64,
        max_drawdown_usd: f64,
    ) -> WindowPolicyRow {
        policy_window_row(
            window_index,
            regime,
            "hedged_narrow",
            net_pnl_usd,
            net_vs_hold_usd,
            net_apr_pct,
            max_drawdown_usd,
        )
    }

    fn policy_window_row(
        window_index: usize,
        regime: &str,
        policy: &str,
        net_pnl_usd: f64,
        net_vs_hold_usd: f64,
        net_apr_pct: f64,
        max_drawdown_usd: f64,
    ) -> WindowPolicyRow {
        WindowPolicyRow {
            window_index,
            swap_start: window_index,
            swap_end_exclusive: window_index + 1,
            swaps: 1,
            block_first: window_index as u64,
            block_last: window_index as u64 + 1,
            tick_first: 0,
            tick_last: 0,
            tick_delta: 0,
            trend_strength: 0.0,
            regime: regime.to_string(),
            minutes: 1.0,
            policy: policy.to_string(),
            net_pnl_usd,
            net_vs_hold_usd,
            fee_minus_lvr_usd: 0.0,
            net_apr_pct: Some(net_apr_pct),
            fee_lvr_apr_pct: None,
            max_drawdown_usd,
            rebalances: 0,
        }
    }

    fn swaps_for_ticks(ticks: &[i32]) -> Vec<autopool_backtest::SwapObs> {
        ticks
            .iter()
            .enumerate()
            .map(|(index, tick)| autopool_backtest::SwapObs {
                block: index as u64,
                log_index: 0,
                amount0: 1.0,
                amount1: -1.0,
                sqrt_price_x96: 1.0,
                liquidity: 1.0,
                tick: *tick,
            })
            .collect()
    }

    fn solana_sample_fixture(
        normalized_swap_previews: Vec<SolanaSwapObsPreview>,
    ) -> SolanaPoolSwapSample {
        SolanaPoolSwapSample {
            signature: "sig".to_string(),
            slot: 1,
            block_time: None,
            instruction: "SwapV2".to_string(),
            token0_mint: "token0".to_string(),
            token1_mint: "token1".to_string(),
            token0_pool_delta_raw: 1,
            token1_pool_delta_raw: -1,
            token0_decimals: Some(6),
            token1_decimals: Some(6),
            token0_in: true,
            token1_in: false,
            program_data_count: 1,
            program_data_base64: vec!["data".to_string()],
            raydium_clmm_events: Vec::new(),
            orca_whirlpool_events: Vec::new(),
            normalized_swap_previews,
        }
    }

    fn hot_pool_candidate_fixture(
        venue: &str,
        pool_kind: &str,
        tick_spacing: Option<i32>,
        bin_step: Option<i32>,
        warnings: Vec<String>,
    ) -> HotPoolCandidateRow {
        HotPoolCandidateRow {
            venue: venue.to_string(),
            pool_kind: pool_kind.to_string(),
            symbol: "CARDS-USDC".to_string(),
            pool_address: "HnhpJPJgBG2KwniMTNW8cVBHvk1hFog3RC3kjnyc23tD".to_string(),
            tokens: vec![
                HotPoolTokenRow {
                    address: "CARDS".to_string(),
                    symbol: "CARDS".to_string(),
                    name: Some("Collector Crypt".to_string()),
                    decimals: Some(6),
                    verified: None,
                },
                HotPoolTokenRow {
                    address: "USDC".to_string(),
                    symbol: "USDC".to_string(),
                    name: Some("USD Coin".to_string()),
                    decimals: Some(6),
                    verified: None,
                },
            ],
            fee_bps: Some(40.0),
            tick_spacing,
            bin_step,
            tvl_usd: 3_600_000.0,
            volume_usd_24h: 5_500_000.0,
            volume_tvl_24h: 1.5,
            fee_apr_24h: 219.0,
            fee_apr_7d: Some(180.0),
            reward_apr_pct: Some(0.0),
            total_apr: Some(219.0),
            formula_fee_apr_24h: Some(219.0),
            reported_to_formula_apr: Some(1.0),
            current_price: Some(1.0),
            price_min_24h: Some(0.9),
            price_max_24h: Some(1.1),
            price_range_24h_pct: Some(22.2),
            price_change_24h_pct: Some(5.0),
            current_tick: Some(0),
            active_liquidity: Some("1000000".to_string()),
            updated_slot: Some(0),
            verified: true,
            target_fee_apr: 2_000.0,
            required_volume_tvl_for_target: Some(13.7),
            target_progress: Some(0.11),
            hot_score: 700.0,
            autoresearch_status: "needs_validation".to_string(),
            experiment_priority: "P1_replay_queue".to_string(),
            next_step: "replay_baselines".to_string(),
            warnings,
        }
    }

    fn solana_pool_candidate_fixture(
        fee_apr_24h: f64,
        price_range_24h_pct: Option<f64>,
    ) -> SolanaPoolCandidate {
        SolanaPoolCandidate {
            venue: SolanaVenue::RaydiumClmm,
            protocol: autopool_core::DexProtocol::Raydium,
            pool_kind: "clmm".to_string(),
            pool_address: "HnhpJPJgBG2KwniMTNW8cVBHvk1hFog3RC3kjnyc23tD".to_string(),
            symbol: "CARDS-USDC".to_string(),
            tokens: vec![
                SolanaToken {
                    address: "CARDS".to_string(),
                    symbol: "CARDS".to_string(),
                    name: Some("Collector Crypt".to_string()),
                    decimals: Some(6),
                    verified: None,
                },
                SolanaToken {
                    address: "USDC".to_string(),
                    symbol: "USDC".to_string(),
                    name: Some("USD Coin".to_string()),
                    decimals: Some(6),
                    verified: None,
                },
            ],
            fee_bps: Some(40.0),
            tick_spacing: Some(60),
            bin_step: None,
            tvl_usd: 3_600_000.0,
            volume_usd_24h: Some(5_500_000.0),
            volume_usd_7d: Some(45_000_000.0),
            fees_usd_24h: Some(22_000.0),
            fees_usd_7d: Some(180_000.0),
            fee_apr_24h: Some(fee_apr_24h),
            fee_apr_7d: Some(149.0),
            reward_apr: Some(0.0),
            total_apr: Some(fee_apr_24h),
            current_price: Some(0.25),
            price_min_24h: Some(0.20),
            price_max_24h: Some(0.25),
            price_range_24h_pct,
            price_change_24h_pct: None,
            current_tick: None,
            active_liquidity: None,
            updated_slot: None,
            verified: true,
            warnings: Vec::new(),
            deployability_score: 0.0,
        }
    }

    fn proxy_args_fixture() -> SolanaProxyReplayArgs {
        SolanaProxyReplayArgs {
            venues: Vec::new(),
            min_tvl_usd: 50_000.0,
            min_volume_usd_24h: 25_000.0,
            min_fee_apr: 100.0,
            max_fee_apr: 5_000.0,
            min_volume_tvl_24h: 0.5,
            page_size: 120,
            limit: 20,
            capital_usd: 10_000.0,
            half_width_pct: vec![2.5, 5.0, 10.0, 20.0],
            max_concentration: 12.0,
            rebalance_slippage_bps: 5.0,
            rebalance_tx_cost_usd: 0.002,
            max_rebalances_per_day: 12,
            output: None,
            format: OutputFormat::Table,
        }
    }
}
