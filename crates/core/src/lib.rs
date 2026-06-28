use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod history;
pub mod network;

pub use history::{
    FeeGrowthSample, LiquidityBandSample, NetworkCostSample, PnlAttribution, PoolReplayFrame,
    PriceSample, SwapFlowSample,
};
pub use network::{NetworkHealth, NetworkRegime};

pub type Usd = f64;
pub type Bps = f64;
pub type Tick = i32;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Chain {
    Evm { chain_id: u64, name: String },
    Other { name: String },
}

impl Chain {
    pub fn evm(chain_id: u64, name: impl Into<String>) -> Self {
        Self::Evm {
            chain_id,
            name: name.into(),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Evm { name, .. } | Self::Other { name } => name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TokenRef {
    pub chain: Chain,
    pub address: Option<String>,
    pub symbol: String,
    pub decimals: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DexProtocol {
    UniswapV3,
    UniswapV4,
    Aerodrome,
    Orca,
    Raydium,
    Meteora,
    Kamino,
    Curve,
    Balancer,
    SushiSwap,
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PoolKey {
    pub chain: Chain,
    pub protocol: DexProtocol,
    pub source_id: String,
    pub address: Option<String>,
    pub symbol: String,
    pub fee_tier_bps: Option<Bps>,
    pub tick_spacing: Option<Tick>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IlRisk {
    None,
    Yes,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExposureKind {
    Single,
    Multi,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YieldSnapshot {
    pub source: String,
    pub pool: PoolKey,
    pub tvl_usd: Option<Usd>,
    pub apy: Option<f64>,
    pub apy_base: Option<f64>,
    pub apy_reward: Option<f64>,
    pub volume_usd_1d: Option<Usd>,
    pub volume_usd_7d: Option<Usd>,
    pub il_risk: IlRisk,
    pub exposure: ExposureKind,
    pub stablecoin: Option<bool>,
    pub outlier: bool,
    pub mu: Option<f64>,
    pub sigma: Option<f64>,
    pub predicted_class: Option<String>,
    pub predicted_probability: Option<f64>,
    pub underlying_tokens: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolMarketState {
    pub pool: PoolKey,
    pub block_number: Option<u64>,
    pub current_tick: Tick,
    pub price_token1_per_token0: f64,
    pub active_liquidity: Option<f64>,
    pub observed_volatility_bps: Option<Bps>,
    pub fee_apr_estimate: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RangeSpec {
    pub lower_tick: Tick,
    pub upper_tick: Tick,
    pub target_capital_usd: Usd,
}

impl RangeSpec {
    pub fn width_ticks(&self) -> Tick {
        self.upper_tick - self.lower_tick
    }

    pub fn contains(&self, tick: Tick) -> bool {
        self.lower_tick <= tick && tick <= self.upper_tick
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionState {
    pub pool: PoolKey,
    pub range: RangeSpec,
    pub liquidity: f64,
    pub amount_token0: f64,
    pub amount_token1: f64,
    pub uncollected_fees_usd: Usd,
    pub entry_value_usd: Usd,
    pub current_value_usd: Usd,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostModel {
    pub gas_cost_usd: Usd,
    pub expected_swap_slippage_bps: Bps,
    pub mev_buffer_bps: Bps,
}

impl Default for CostModel {
    fn default() -> Self {
        Self {
            gas_cost_usd: 0.0,
            expected_swap_slippage_bps: 0.0,
            mev_buffer_bps: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskLimits {
    pub min_tvl_usd: Usd,
    pub min_volume_usd_1d: Usd,
    pub max_pool_exposure_usd: Usd,
    pub max_token_exposure_usd: Usd,
    pub max_one_sided_inventory_pct: f64,
    pub max_gas_to_edge_pct: f64,
    pub max_daily_rebalances_per_pool: u32,
}

impl Default for RiskLimits {
    fn default() -> Self {
        Self {
            min_tvl_usd: 1_000_000.0,
            min_volume_usd_1d: 100_000.0,
            max_pool_exposure_usd: 10_000.0,
            max_token_exposure_usd: 25_000.0,
            max_one_sided_inventory_pct: 0.8,
            max_gas_to_edge_pct: 0.15,
            max_daily_rebalances_per_pool: 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationInput {
    pub market: PoolMarketState,
    pub yield_snapshot: Option<YieldSnapshot>,
    pub current_position: Option<PositionState>,
    pub network: Option<NetworkRegime>,
    pub cost_model: CostModel,
    pub risk_limits: RiskLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyObjective {
    pub expected_fee_income_usd: Usd,
    pub expected_reward_income_usd: Usd,
    pub expected_inventory_pnl_usd: Usd,
    pub expected_il_cost_usd: Usd,
    pub expected_gas_cost_usd: Usd,
    pub expected_slippage_cost_usd: Usd,
    pub expected_mev_cost_usd: Usd,
    pub risk_penalty_usd: Usd,
}

impl StrategyObjective {
    pub fn expected_net_value_usd(&self) -> Usd {
        self.expected_fee_income_usd
            + self.expected_reward_income_usd
            + self.expected_inventory_pnl_usd
            - self.expected_il_cost_usd
            - self.expected_gas_cost_usd
            - self.expected_slippage_cost_usd
            - self.expected_mev_cost_usd
            - self.risk_penalty_usd
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DecisionKind {
    Hold,
    Rebalance,
    Exit,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebalanceDecision {
    pub kind: DecisionKind,
    pub target_range: Option<RangeSpec>,
    pub expected_net_edge_usd: Option<Usd>,
    pub reasons: Vec<String>,
}

impl RebalanceDecision {
    pub fn hold(reason: impl Into<String>) -> Self {
        Self {
            kind: DecisionKind::Hold,
            target_range: None,
            expected_net_edge_usd: None,
            reasons: vec![reason.into()],
        }
    }

    pub fn reject(reason: impl Into<String>) -> Self {
        Self {
            kind: DecisionKind::Reject,
            target_range: None,
            expected_net_edge_usd: None,
            reasons: vec![reason.into()],
        }
    }
}

#[async_trait]
pub trait YieldDataProvider {
    async fn latest_yields(&self) -> Result<Vec<YieldSnapshot>, CoreError>;
}

#[async_trait]
pub trait MarketDataProvider {
    async fn pool_state(&self, pool: &PoolKey) -> Result<PoolMarketState, CoreError>;
}

#[async_trait]
pub trait PositionProvider {
    async fn open_positions(&self) -> Result<Vec<PositionState>, CoreError>;
}

pub trait RangeOptimizer {
    fn decide(&self, input: &OptimizationInput) -> RebalanceDecision;
}
