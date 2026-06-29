//! Meteora-style DLMM bin replay primitives.
//!
//! This module is intentionally separate from the v3/Whirlpool tick replay engine:
//! DLMM liquidity is discrete by bin, so a v3 `sqrtPriceX96` position model is the
//! wrong abstraction. The current implementation is a bounded replay skeleton for
//! normalized bin observations; real deployment-quality scoring still needs decoded
//! Meteora swap events plus historical bin-liquidity snapshots.

use serde::{Deserialize, Serialize};

const SECONDS_PER_YEAR: f64 = 31_557_600.0;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DlmmReplayConfig {
    pub capital_usd: f64,
    pub fee_bps: f64,
    pub bin_step_bps: f64,
    pub rebalance_cost_usd: f64,
    pub rebalance_slippage_bps: f64,
    pub block_seconds: f64,
}

impl Default for DlmmReplayConfig {
    fn default() -> Self {
        Self {
            capital_usd: 10_000.0,
            fee_bps: 20.0,
            bin_step_bps: 20.0,
            rebalance_cost_usd: 0.002,
            rebalance_slippage_bps: 5.0,
            block_seconds: 0.4,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DlmmBinObs {
    pub block: u64,
    pub active_bin_id: i32,
    /// Active-bin liquidity in USD-equivalent units. This must come from decoded
    /// bin-array account state, not the pool-wide TVL.
    pub active_liquidity_usd: f64,
    /// Swap input notional in USD-equivalent units.
    pub amount_in_usd: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum DlmmRangeMode {
    HoldInventory,
    StaticRange { half_width_bins: i32 },
    CenteredRebalance { half_width_bins: i32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlmmPolicyReport {
    pub policy: String,
    pub capital_usd: f64,
    pub final_value_usd: f64,
    pub fee_income_usd: f64,
    pub net_pnl_usd: f64,
    pub net_apr_pct: Option<f64>,
    pub rebalance_cost_usd: f64,
    pub rebalances: u32,
    pub bins_in_range: u64,
    pub bins_total: u64,
    pub time_in_range_pct: f64,
    pub final_bin_id: i32,
    pub max_drawdown_usd: f64,
}

pub fn dlmm_bin_price_ratio(from_bin: i32, to_bin: i32, bin_step_bps: f64) -> f64 {
    let base = 1.0 + bin_step_bps / 10_000.0;
    base.powi(to_bin - from_bin)
}

pub fn run_dlmm_bin_policy(
    observations: &[DlmmBinObs],
    cfg: &DlmmReplayConfig,
    mode: DlmmRangeMode,
    policy: &str,
) -> Option<DlmmPolicyReport> {
    let first = observations.first()?;
    if matches!(mode, DlmmRangeMode::HoldInventory) {
        return Some(run_dlmm_hold(observations, cfg, policy));
    }

    let half_width = match mode {
        DlmmRangeMode::HoldInventory => 0,
        DlmmRangeMode::StaticRange { half_width_bins }
        | DlmmRangeMode::CenteredRebalance { half_width_bins } => half_width_bins.max(0),
    };
    let can_rebalance = matches!(mode, DlmmRangeMode::CenteredRebalance { .. });
    let mut entry_bin = first.active_bin_id;
    let mut lower = entry_bin - half_width;
    let mut upper = entry_bin + half_width;
    let mut deployed_usd = cfg.capital_usd;
    let mut fees_usd = 0.0_f64;
    let mut costs_usd = 0.0_f64;
    let mut rebalances = 0u32;
    let mut bins_in_range = 0u64;
    let mut peak_equity = cfg.capital_usd;
    let mut max_drawdown = 0.0_f64;

    for obs in observations {
        let in_range = (lower..=upper).contains(&obs.active_bin_id);
        if in_range {
            bins_in_range += 1;
            fees_usd += dlmm_fee_share_usd(deployed_usd, cfg, obs);
        } else if can_rebalance {
            let position_value =
                dlmm_inventory_value_usd(deployed_usd, entry_bin, obs.active_bin_id, cfg);
            let notional_cost = position_value.max(0.0) * cfg.rebalance_slippage_bps / 10_000.0
                + cfg.rebalance_cost_usd;
            costs_usd += notional_cost;
            deployed_usd = position_value.max(0.0);
            entry_bin = obs.active_bin_id;
            lower = entry_bin - half_width;
            upper = entry_bin + half_width;
            rebalances += 1;
        }

        let equity = dlmm_inventory_value_usd(deployed_usd, entry_bin, obs.active_bin_id, cfg)
            + fees_usd
            - costs_usd;
        peak_equity = peak_equity.max(equity);
        max_drawdown = max_drawdown.max(peak_equity - equity);
    }

    let last = observations.last().unwrap();
    let final_value_usd =
        dlmm_inventory_value_usd(deployed_usd, entry_bin, last.active_bin_id, cfg) + fees_usd;
    let net_pnl_usd = final_value_usd - cfg.capital_usd - costs_usd;
    Some(DlmmPolicyReport {
        policy: policy.to_string(),
        capital_usd: cfg.capital_usd,
        final_value_usd,
        fee_income_usd: fees_usd,
        net_pnl_usd,
        net_apr_pct: dlmm_annualized_apr(net_pnl_usd, cfg.capital_usd, observations, cfg),
        rebalance_cost_usd: costs_usd,
        rebalances,
        bins_in_range,
        bins_total: observations.len() as u64,
        time_in_range_pct: 100.0 * bins_in_range as f64 / observations.len() as f64,
        final_bin_id: last.active_bin_id,
        max_drawdown_usd: max_drawdown,
    })
}

fn run_dlmm_hold(
    observations: &[DlmmBinObs],
    cfg: &DlmmReplayConfig,
    policy: &str,
) -> DlmmPolicyReport {
    let first = observations.first().unwrap();
    let last = observations.last().unwrap();
    let final_value_usd = dlmm_inventory_value_usd(
        cfg.capital_usd,
        first.active_bin_id,
        last.active_bin_id,
        cfg,
    );
    let net_pnl_usd = final_value_usd - cfg.capital_usd;
    DlmmPolicyReport {
        policy: policy.to_string(),
        capital_usd: cfg.capital_usd,
        final_value_usd,
        fee_income_usd: 0.0,
        net_pnl_usd,
        net_apr_pct: dlmm_annualized_apr(net_pnl_usd, cfg.capital_usd, observations, cfg),
        rebalance_cost_usd: 0.0,
        rebalances: 0,
        bins_in_range: 0,
        bins_total: observations.len() as u64,
        time_in_range_pct: 0.0,
        final_bin_id: last.active_bin_id,
        max_drawdown_usd: dlmm_hold_max_drawdown(observations, cfg),
    }
}

fn dlmm_fee_share_usd(capital_usd: f64, cfg: &DlmmReplayConfig, obs: &DlmmBinObs) -> f64 {
    if obs.amount_in_usd <= 0.0 || obs.active_liquidity_usd <= 0.0 || capital_usd <= 0.0 {
        return 0.0;
    }
    let share = capital_usd / (obs.active_liquidity_usd + capital_usd);
    obs.amount_in_usd * cfg.fee_bps / 10_000.0 * share
}

fn dlmm_inventory_value_usd(
    capital_usd: f64,
    entry_bin: i32,
    current_bin: i32,
    cfg: &DlmmReplayConfig,
) -> f64 {
    let price_ratio = dlmm_bin_price_ratio(entry_bin, current_bin, cfg.bin_step_bps);
    0.5 * capital_usd + 0.5 * capital_usd * price_ratio
}

fn dlmm_annualized_apr(
    net_pnl_usd: f64,
    capital_usd: f64,
    observations: &[DlmmBinObs],
    cfg: &DlmmReplayConfig,
) -> Option<f64> {
    if observations.len() < 2 || capital_usd <= 0.0 {
        return None;
    }
    let seconds = observations
        .last()
        .unwrap()
        .block
        .saturating_sub(observations.first().unwrap().block) as f64
        * cfg.block_seconds;
    if seconds <= 0.0 {
        return None;
    }
    Some(100.0 * net_pnl_usd / capital_usd * SECONDS_PER_YEAR / seconds)
}

fn dlmm_hold_max_drawdown(observations: &[DlmmBinObs], cfg: &DlmmReplayConfig) -> f64 {
    let first = observations.first().unwrap();
    let mut peak = cfg.capital_usd;
    let mut drawdown = 0.0_f64;
    for obs in observations {
        let equity =
            dlmm_inventory_value_usd(cfg.capital_usd, first.active_bin_id, obs.active_bin_id, cfg);
        peak = peak.max(equity);
        drawdown = drawdown.max(peak - equity);
    }
    drawdown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> DlmmReplayConfig {
        DlmmReplayConfig {
            capital_usd: 10_000.0,
            fee_bps: 20.0,
            bin_step_bps: 20.0,
            rebalance_cost_usd: 0.002,
            rebalance_slippage_bps: 5.0,
            block_seconds: 0.4,
        }
    }

    fn obs(path: &[i32]) -> Vec<DlmmBinObs> {
        path.iter()
            .enumerate()
            .map(|(i, active_bin_id)| DlmmBinObs {
                block: 1000 + i as u64,
                active_bin_id: *active_bin_id,
                active_liquidity_usd: 40_000.0,
                amount_in_usd: 3_000.0,
            })
            .collect()
    }

    #[test]
    fn dlmm_bin_price_ratio_uses_step_compounding() {
        let r = dlmm_bin_price_ratio(100, 103, 20.0);
        let expected = 1.002_f64.powi(3);
        assert!((r - expected).abs() < 1e-12);
        assert!((dlmm_bin_price_ratio(103, 100, 20.0) * r - 1.0).abs() < 1e-12);
    }

    #[test]
    fn centered_dlmm_rebalance_preserves_fee_capture_on_traveling_flow() {
        let stream = obs(&[100, 101, 102, 103, 104, 105, 106, 107, 108]);
        let mut cfg = cfg();
        // Isolate fee capture from directional inventory PnL; real bin steps can make
        // a static out-of-range position win on beta, which is not this unit's claim.
        cfg.bin_step_bps = 0.0;
        cfg.rebalance_cost_usd = 0.0;
        cfg.rebalance_slippage_bps = 0.0;
        let static_range = run_dlmm_bin_policy(
            &stream,
            &cfg,
            DlmmRangeMode::StaticRange { half_width_bins: 1 },
            "static_bin",
        )
        .unwrap();
        let recentered = run_dlmm_bin_policy(
            &stream,
            &cfg,
            DlmmRangeMode::CenteredRebalance { half_width_bins: 1 },
            "recentered_bin",
        )
        .unwrap();

        assert!(recentered.rebalances > 0);
        assert!(recentered.bins_in_range > static_range.bins_in_range);
        assert!(recentered.fee_income_usd > static_range.fee_income_usd);
        assert!(recentered.net_pnl_usd > static_range.net_pnl_usd);
    }

    #[test]
    fn dlmm_replay_returns_none_for_empty_stream() {
        assert!(
            run_dlmm_bin_policy(
                &[],
                &cfg(),
                DlmmRangeMode::StaticRange { half_width_bins: 1 },
                "empty",
            )
            .is_none()
        );
    }
}
