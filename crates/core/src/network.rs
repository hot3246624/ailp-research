use serde::{Deserialize, Serialize};

use crate::{Bps, Usd};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkHealth {
    Tradable,
    Degraded,
    Halted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkRegime {
    pub chain_id: u64,
    pub health: NetworkHealth,
    pub block_time_seconds: Option<f64>,
    pub expected_finality_seconds: Option<f64>,
    pub reorg_risk_bps: Option<Bps>,
    pub rpc_latency_ms: Option<f64>,
    pub rpc_disagreement_bps: Option<Bps>,
    pub private_route_available: bool,
    pub estimated_mev_loss_bps: Option<Bps>,
    pub gas_usd_collect: Usd,
    pub gas_usd_rebalance: Usd,
    pub gas_usd_exit: Usd,
}

impl NetworkRegime {
    pub fn tradable(chain_id: u64) -> Self {
        Self {
            chain_id,
            health: NetworkHealth::Tradable,
            block_time_seconds: None,
            expected_finality_seconds: None,
            reorg_risk_bps: None,
            rpc_latency_ms: None,
            rpc_disagreement_bps: None,
            private_route_available: false,
            estimated_mev_loss_bps: None,
            gas_usd_collect: 0.0,
            gas_usd_rebalance: 0.0,
            gas_usd_exit: 0.0,
        }
    }

    pub fn is_tradable(&self) -> bool {
        self.health == NetworkHealth::Tradable
    }
}
