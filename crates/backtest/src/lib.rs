use autopool_core::{
    OptimizationInput, PnlAttribution, PoolMarketState, PoolReplayFrame, RangeOptimizer,
    RebalanceDecision,
};
use serde::{Deserialize, Serialize};

pub mod replay;

pub use replay::{
    ExecConfig, FoldResult, MultiPathReport, PolicyDistribution, PolicyReport, RangeMode,
    ReplayConfig, Scenario, SwapObs, WalkForwardConfig, WalkForwardReport, decode_swap_obs,
    multi_path_eval, run_baseline_battery, run_baseline_battery_with, run_single_policy,
    scenario_swaps, walk_forward,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestEvent {
    pub timestamp_unix: i64,
    pub market: PoolMarketState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestStep {
    pub event: BacktestEvent,
    pub decision: RebalanceDecision,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BacktestResult {
    pub steps: Vec<BacktestStep>,
    pub pnl: PnlAttribution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BaselinePolicy {
    HoldInventory,
    PassiveWideRange,
    FixedWidthRebalance,
    VolatilityScaledRebalance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineReplay {
    pub policy: BaselinePolicy,
    pub frames: Vec<PoolReplayFrame>,
    pub pnl: PnlAttribution,
}

pub fn default_pilot_baselines() -> Vec<BaselinePolicy> {
    vec![
        BaselinePolicy::HoldInventory,
        BaselinePolicy::PassiveWideRange,
        BaselinePolicy::FixedWidthRebalance,
        BaselinePolicy::VolatilityScaledRebalance,
    ]
}

pub struct ReplayBacktester<O> {
    optimizer: O,
}

impl<O> ReplayBacktester<O>
where
    O: RangeOptimizer,
{
    pub fn new(optimizer: O) -> Self {
        Self { optimizer }
    }

    pub fn replay(&self, inputs: Vec<(BacktestEvent, OptimizationInput)>) -> BacktestResult {
        let steps = inputs
            .into_iter()
            .map(|(event, input)| BacktestStep {
                event,
                decision: self.optimizer.decide(&input),
            })
            .collect();

        BacktestResult {
            steps,
            ..BacktestResult::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use autopool_core::PnlAttribution;

    #[test]
    fn pnl_attribution_subtracts_costs() {
        let pnl = PnlAttribution {
            fee_income_usd: 100.0,
            reward_income_usd: 10.0,
            inventory_pnl_usd: -15.0,
            impermanent_loss_usd: 20.0,
            gas_cost_usd: 5.0,
            swap_slippage_usd: 3.0,
            mev_loss_usd: 2.0,
            out_of_range_opportunity_cost_usd: 1.0,
        };

        assert_eq!(pnl.net_pnl_usd(), 64.0);
    }
}
