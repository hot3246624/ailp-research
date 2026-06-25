use serde::{Deserialize, Serialize};

use crate::{Bps, PoolKey, Tick, Usd};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceSample {
    pub block_number: u64,
    pub timestamp_unix: i64,
    pub tick: Tick,
    pub price_token1_per_token0: f64,
    pub sqrt_price_x96: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapFlowSample {
    pub block_number: u64,
    pub timestamp_unix: i64,
    pub amount0: f64,
    pub amount1: f64,
    pub volume_usd: Option<Usd>,
    pub tick_after: Tick,
    pub price_impact_bps: Option<Bps>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeGrowthSample {
    pub block_number: u64,
    pub timestamp_unix: i64,
    pub fee_growth_global0_x128: String,
    pub fee_growth_global1_x128: String,
    pub fee_usd_estimate: Option<Usd>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidityBandSample {
    pub lower_tick: Tick,
    pub upper_tick: Tick,
    pub liquidity_net: f64,
    pub liquidity_gross: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkCostSample {
    pub block_number: u64,
    pub timestamp_unix: i64,
    pub gas_price_gwei: f64,
    pub collect_cost_usd: Option<Usd>,
    pub rebalance_cost_usd: Option<Usd>,
    pub exit_cost_usd: Option<Usd>,
    pub mev_loss_bps_estimate: Option<Bps>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolReplayFrame {
    pub pool: PoolKey,
    pub price: PriceSample,
    pub active_liquidity: Option<f64>,
    pub nearby_liquidity: Vec<LiquidityBandSample>,
    pub recent_swaps: Vec<SwapFlowSample>,
    pub fee_growth: Option<FeeGrowthSample>,
    pub network_cost: Option<NetworkCostSample>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PnlAttribution {
    pub fee_income_usd: Usd,
    pub reward_income_usd: Usd,
    pub inventory_pnl_usd: Usd,
    pub impermanent_loss_usd: Usd,
    pub gas_cost_usd: Usd,
    pub swap_slippage_usd: Usd,
    pub mev_loss_usd: Usd,
    pub out_of_range_opportunity_cost_usd: Usd,
}

impl PnlAttribution {
    pub fn net_pnl_usd(&self) -> Usd {
        self.fee_income_usd + self.reward_income_usd + self.inventory_pnl_usd
            - self.impermanent_loss_usd
            - self.gas_cost_usd
            - self.swap_slippage_usd
            - self.mev_loss_usd
            - self.out_of_range_opportunity_cost_usd
    }
}
