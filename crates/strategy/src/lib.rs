use autopool_core::{
    DecisionKind, ExposureKind, IlRisk, OptimizationInput, RangeOptimizer, RangeSpec,
    RebalanceDecision, RiskLimits, Tick, YieldSnapshot,
};

#[derive(Debug, Clone)]
pub struct PoolScore {
    pub score: f64,
    pub accepted: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct WeightedRiskModel {
    pub limits: RiskLimits,
    pub reward_apy_penalty: f64,
    pub il_risk_penalty: f64,
    pub outlier_penalty: f64,
    pub sigma_penalty: f64,
}

impl Default for WeightedRiskModel {
    fn default() -> Self {
        Self {
            limits: RiskLimits::default(),
            reward_apy_penalty: 0.4,
            il_risk_penalty: 5.0,
            outlier_penalty: 100.0,
            sigma_penalty: 8.0,
        }
    }
}

impl WeightedRiskModel {
    pub fn score_yield(&self, snapshot: &YieldSnapshot) -> PoolScore {
        let mut reasons = Vec::new();
        let mut score = snapshot.apy.unwrap_or(0.0);
        let mut accepted = true;

        if snapshot.outlier {
            accepted = false;
            score -= self.outlier_penalty;
            reasons.push("rejected: DeFiLlama marks this pool as an APY outlier".to_string());
        }

        if snapshot.tvl_usd.unwrap_or(0.0) < self.limits.min_tvl_usd {
            accepted = false;
            reasons.push(format!(
                "rejected: TVL below ${:.0}",
                self.limits.min_tvl_usd
            ));
        }

        if snapshot.volume_usd_1d.unwrap_or(0.0) < self.limits.min_volume_usd_1d {
            reasons.push(format!(
                "penalty: 1d volume below ${:.0}",
                self.limits.min_volume_usd_1d
            ));
            score -= 10.0;
        }

        if snapshot.il_risk == IlRisk::Yes || snapshot.exposure == ExposureKind::Multi {
            score -= self.il_risk_penalty;
            reasons.push("penalty: multi-token LP inventory can become adverse".to_string());
        }

        let pair_penalty = pair_inventory_penalty_bps(&snapshot.pool.symbol);
        if pair_penalty > 0.0 {
            score -= pair_penalty;
            reasons.push(format!(
                "penalty: pair inventory risk class costs {:.1} score",
                pair_penalty
            ));
        }

        let reward_share = reward_share(snapshot);
        if reward_share > 0.5 {
            score -= snapshot.apy.unwrap_or(0.0) * self.reward_apy_penalty;
            reasons.push("penalty: APY is reward-heavy and needs liquidation modeling".to_string());
        }

        if let Some(sigma) = snapshot.sigma {
            score -= sigma * self.sigma_penalty;
        }

        if reasons.is_empty() {
            reasons.push("accepted by public-yield screen".to_string());
        }

        PoolScore {
            score,
            accepted,
            reasons,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConservativeRangeOptimizer {
    pub min_width_ticks: Tick,
    pub max_width_ticks: Tick,
    pub edge_buffer_usd: f64,
}

impl Default for ConservativeRangeOptimizer {
    fn default() -> Self {
        Self {
            min_width_ticks: 120,
            max_width_ticks: 3_600,
            edge_buffer_usd: 20.0,
        }
    }
}

impl RangeOptimizer for ConservativeRangeOptimizer {
    fn decide(&self, input: &OptimizationInput) -> RebalanceDecision {
        if let Some(network) = &input.network {
            if !network.is_tradable() {
                return RebalanceDecision::reject("network regime is not tradable");
            }
        }

        if let Some(snapshot) = &input.yield_snapshot {
            if snapshot.outlier {
                return RebalanceDecision::reject("yield snapshot is marked as outlier");
            }

            if snapshot.tvl_usd.unwrap_or(0.0) < input.risk_limits.min_tvl_usd {
                return RebalanceDecision::reject("pool TVL is below risk limit");
            }
        }

        let target_capital_usd = input
            .current_position
            .as_ref()
            .map(|position| position.current_value_usd)
            .unwrap_or(input.risk_limits.max_pool_exposure_usd);

        let width = self.width_from_volatility(input.market.observed_volatility_bps);
        let target_range = RangeSpec {
            lower_tick: input.market.current_tick - width / 2,
            upper_tick: input.market.current_tick + width / 2,
            target_capital_usd,
        };

        match &input.current_position {
            None => RebalanceDecision {
                kind: DecisionKind::Rebalance,
                target_range: Some(target_range),
                expected_net_edge_usd: None,
                reasons: vec!["no open position; propose initial range".to_string()],
            },
            Some(position) if position.range.contains(input.market.current_tick) => {
                RebalanceDecision::hold("current position is still in range")
            }
            Some(position) => {
                let expected_edge = estimated_rebalance_edge_usd(input);
                if expected_edge > input.cost_model.gas_cost_usd + self.edge_buffer_usd {
                    RebalanceDecision {
                        kind: DecisionKind::Rebalance,
                        target_range: Some(target_range),
                        expected_net_edge_usd: Some(expected_edge - input.cost_model.gas_cost_usd),
                        reasons: vec![
                            "current position is out of range".to_string(),
                            "expected edge exceeds gas and buffer".to_string(),
                        ],
                    }
                } else if position.current_value_usd > input.risk_limits.max_pool_exposure_usd {
                    RebalanceDecision {
                        kind: DecisionKind::Exit,
                        target_range: None,
                        expected_net_edge_usd: Some(expected_edge),
                        reasons: vec!["position exceeds max pool exposure".to_string()],
                    }
                } else {
                    RebalanceDecision::hold(
                        "out of range, but estimated edge does not justify cost",
                    )
                }
            }
        }
    }
}

impl ConservativeRangeOptimizer {
    fn width_from_volatility(&self, volatility_bps: Option<f64>) -> Tick {
        let volatility_width = volatility_bps
            .map(|value| (value * 3.0).round() as Tick)
            .unwrap_or(self.min_width_ticks);
        volatility_width.clamp(self.min_width_ticks, self.max_width_ticks)
    }
}

fn reward_share(snapshot: &YieldSnapshot) -> f64 {
    let reward = snapshot.apy_reward.unwrap_or(0.0).max(0.0);
    let total = snapshot.apy.unwrap_or(0.0).max(0.0);
    if total == 0.0 { 0.0 } else { reward / total }
}

fn pair_inventory_penalty_bps(symbol: &str) -> f64 {
    let tokens = symbol
        .split(['-', '/', '_'])
        .map(|token| token.to_ascii_uppercase())
        .collect::<Vec<_>>();

    if tokens.len() < 2 {
        return 8.0;
    }

    if tokens.iter().all(|token| is_stable_token(token)) {
        return 0.0;
    }

    if tokens.iter().all(|token| is_eth_correlated_token(token))
        || tokens.iter().all(|token| is_btc_correlated_token(token))
    {
        return 2.0;
    }

    if tokens
        .iter()
        .any(|token| matches!(token.as_str(), "AERO" | "VIRTUAL" | "TOSHI" | "BRETT"))
    {
        return 12.0;
    }

    8.0
}

fn is_stable_token(token: &str) -> bool {
    matches!(
        token,
        "USDC" | "USDBC" | "USDT" | "DAI" | "EURC" | "EUSD" | "MSUSD" | "CRVUSD" | "LUSD"
    )
}

fn is_eth_correlated_token(token: &str) -> bool {
    matches!(
        token,
        "ETH" | "WETH" | "CBETH" | "WSTETH" | "MSETH" | "RETH"
    )
}

fn is_btc_correlated_token(token: &str) -> bool {
    matches!(token, "BTC" | "WBTC" | "CBBTC" | "CBLTC" | "TBTC")
}

fn estimated_rebalance_edge_usd(input: &OptimizationInput) -> f64 {
    let capital = input
        .current_position
        .as_ref()
        .map(|position| position.current_value_usd)
        .unwrap_or(input.risk_limits.max_pool_exposure_usd);
    let annual_fee_apr = input
        .market
        .fee_apr_estimate
        .or_else(|| {
            input
                .yield_snapshot
                .as_ref()
                .and_then(|value| value.apy_base)
        })
        .unwrap_or(0.0);

    // MVP assumption: rebalance edge is one day of recoverable fee APR.
    capital * annual_fee_apr / 100.0 / 365.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_inventory_penalty_prefers_correlated_pairs() {
        assert_eq!(pair_inventory_penalty_bps("EURC-USDC"), 0.0);
        assert_eq!(pair_inventory_penalty_bps("WETH-MSETH"), 2.0);
        assert_eq!(pair_inventory_penalty_bps("CBLTC-CBBTC"), 2.0);
        assert_eq!(pair_inventory_penalty_bps("USDC-AERO"), 12.0);
    }
}
