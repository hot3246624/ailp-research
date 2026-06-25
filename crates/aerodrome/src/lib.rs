use autopool_core::{DexProtocol, YieldSnapshot};
use serde::{Deserialize, Serialize};

pub const BASE_CHAIN_ID: u64 = 8453;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlipstreamDeployment {
    Initial,
    GaugeCaps,
    GaugesV3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlipstreamContracts {
    pub deployment: SlipstreamDeployment,
    pub pool_factory: &'static str,
    pub nonfungible_position_manager: &'static str,
    pub swap_router: &'static str,
    pub mixed_quoter: &'static str,
    pub gauge_factory: &'static str,
}

// Current latest deployment listed by the official aerodrome-finance/slipstream
// repository under "Gauges V3 Deployment".
pub const BASE_SLIPSTREAM_GAUGES_V3: SlipstreamContracts = SlipstreamContracts {
    deployment: SlipstreamDeployment::GaugesV3,
    pool_factory: "0xf8f2eB4940CFE7d13603DDDD87f123820Fc061Ef",
    nonfungible_position_manager: "0xe1f8cd9AC4e4A65F54f38a5CdAfCA44f6dD68b53",
    swap_router: "0x698Cb2b6dd822994581fEa6eA4Fc755d1363A92F",
    mixed_quoter: "0xCd2A7D98e82D6107eac1828ce8DeAA6acB65b555",
    gauge_factory: "0x385293CaE378C813F16f0C1334d774AdDDf56AbB",
};

pub const BASE_SLIPSTREAM_GAUGE_CAPS: SlipstreamContracts = SlipstreamContracts {
    deployment: SlipstreamDeployment::GaugeCaps,
    pool_factory: "0xaDe65c38CD4849aDBA595a4323a8C7DdfE89716a",
    nonfungible_position_manager: "0xa990C6a764b73BF43cee5Bb40339c3322FB9D55F",
    swap_router: "0xcbBb8035cAc7D4B3Ca7aBb74cF7BdF900215Ce0D",
    mixed_quoter: "0x49540630A4d2CE67d54450D007D634F4c45B4f4f",
    gauge_factory: "0xD30677bd8dd15132F251Cb54CbDA552d2A05Fb08",
};

pub const BASE_SLIPSTREAM_INITIAL: SlipstreamContracts = SlipstreamContracts {
    deployment: SlipstreamDeployment::Initial,
    pool_factory: "0x5e7BB104d84c7CB9B682AaC2F3d509f5F406809A",
    nonfungible_position_manager: "0x827922686190790b37229fd06084350E74485b72",
    swap_router: "0xBE6D8f0d05cC4be24d5167a3eF062215bE6D18a5",
    mixed_quoter: "0x0A5aA5D3a4d28014f967Bf0f29EAA3FF9807D5c6",
    gauge_factory: "0xA4e46b4f701c62e14DF11B48dCe76A7d793CDf2B",
};

pub fn base_slipstream_factories_latest_first() -> [SlipstreamContracts; 3] {
    [
        BASE_SLIPSTREAM_GAUGES_V3,
        BASE_SLIPSTREAM_GAUGE_CAPS,
        BASE_SLIPSTREAM_INITIAL,
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairRiskClass {
    Stable,
    EthCorrelated,
    BtcCorrelated,
    NativeOrIncentive,
    LongTail,
    Unknown,
}

impl PairRiskClass {
    pub fn strategy_priority(self) -> u8 {
        match self {
            Self::Stable => 0,
            Self::EthCorrelated | Self::BtcCorrelated => 1,
            Self::NativeOrIncentive => 2,
            Self::LongTail => 3,
            Self::Unknown => 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlipstreamCandidate {
    pub symbol: String,
    pub source_pool_id: String,
    pub underlying_tokens: Vec<String>,
    pub tick_spacing: Option<i32>,
    pub tvl_usd: f64,
    pub volume_usd_1d: f64,
    pub apy: f64,
    pub apy_base: f64,
    pub apy_reward: f64,
    pub reward_share: f64,
    pub fee_tier_bps: Option<f64>,
    pub pair_risk: PairRiskClass,
    pub pilot_bucket: String,
}

impl SlipstreamCandidate {
    pub fn from_yield(snapshot: &YieldSnapshot) -> Option<Self> {
        if snapshot.pool.protocol != DexProtocol::Aerodrome
            || !snapshot.pool.chain.name().eq_ignore_ascii_case("Base")
        {
            return None;
        }

        let apy = snapshot.apy.unwrap_or(0.0);
        let apy_reward = snapshot.apy_reward.unwrap_or(0.0);
        let reward_share = if apy > 0.0 { apy_reward / apy } else { 0.0 };
        let pair_risk = classify_pair(&snapshot.pool.symbol);

        Some(Self {
            symbol: snapshot.pool.symbol.clone(),
            source_pool_id: snapshot.pool.source_id.clone(),
            underlying_tokens: snapshot.underlying_tokens.clone(),
            tick_spacing: snapshot.pool.tick_spacing,
            tvl_usd: snapshot.tvl_usd.unwrap_or(0.0),
            volume_usd_1d: snapshot.volume_usd_1d.unwrap_or(0.0),
            apy,
            apy_base: snapshot.apy_base.unwrap_or(0.0),
            apy_reward,
            reward_share,
            fee_tier_bps: snapshot.pool.fee_tier_bps,
            pair_risk,
            pilot_bucket: pilot_bucket(pair_risk, reward_share).to_string(),
        })
    }
}

pub fn build_pilot_universe(
    snapshots: &[YieldSnapshot],
    min_tvl_usd: f64,
    min_volume_usd_1d: f64,
    max_reward_share: f64,
) -> Vec<SlipstreamCandidate> {
    let mut candidates = snapshots
        .iter()
        .filter(|snapshot| !snapshot.outlier)
        .filter_map(SlipstreamCandidate::from_yield)
        .filter(|candidate| candidate.tvl_usd >= min_tvl_usd)
        .filter(|candidate| candidate.volume_usd_1d >= min_volume_usd_1d)
        .filter(|candidate| candidate.reward_share <= max_reward_share)
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| {
        left.pair_risk
            .strategy_priority()
            .cmp(&right.pair_risk.strategy_priority())
            .then_with(|| right.volume_usd_1d.total_cmp(&left.volume_usd_1d))
            .then_with(|| right.apy_base.total_cmp(&left.apy_base))
    });

    candidates
}

pub fn classify_pair(symbol: &str) -> PairRiskClass {
    let tokens = split_symbol(symbol);
    if tokens.len() < 2 {
        return PairRiskClass::Unknown;
    }

    if tokens.iter().all(|token| is_stable(token)) {
        return PairRiskClass::Stable;
    }

    if tokens.iter().all(|token| is_eth_correlated(token)) {
        return PairRiskClass::EthCorrelated;
    }

    if tokens.iter().all(|token| is_btc_correlated(token)) {
        return PairRiskClass::BtcCorrelated;
    }

    if tokens.iter().any(|token| is_native_or_incentive(token)) {
        return PairRiskClass::NativeOrIncentive;
    }

    PairRiskClass::LongTail
}

fn pilot_bucket(pair_risk: PairRiskClass, reward_share: f64) -> &'static str {
    if reward_share > 0.5 {
        return "reward-heavy-control";
    }

    match pair_risk {
        PairRiskClass::Stable => "stable-primary",
        PairRiskClass::EthCorrelated | PairRiskClass::BtcCorrelated => "correlated-control",
        PairRiskClass::NativeOrIncentive => "native-risk-control",
        PairRiskClass::LongTail => "long-tail-control",
        PairRiskClass::Unknown => "unknown",
    }
}

fn split_symbol(symbol: &str) -> Vec<String> {
    symbol
        .split(['-', '/', '_'])
        .map(|token| token.trim().to_ascii_uppercase())
        .filter(|token| !token.is_empty())
        .collect()
}

fn is_stable(token: &str) -> bool {
    matches!(
        token,
        "USDC" | "USDBC" | "USDT" | "DAI" | "EURC" | "EUSD" | "MSUSD" | "CRVUSD" | "LUSD"
    )
}

fn is_eth_correlated(token: &str) -> bool {
    matches!(
        token,
        "ETH" | "WETH" | "CBETH" | "WSTETH" | "MSETH" | "RETH"
    )
}

fn is_btc_correlated(token: &str) -> bool {
    matches!(token, "BTC" | "WBTC" | "CBBTC" | "CBLTC" | "TBTC")
}

fn is_native_or_incentive(token: &str) -> bool {
    matches!(token, "AERO" | "VELO" | "VIRTUAL" | "TOSHI" | "BRETT")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_pilot_pair_risk() {
        assert_eq!(classify_pair("EURC-USDC"), PairRiskClass::Stable);
        assert_eq!(classify_pair("WETH-MSETH"), PairRiskClass::EthCorrelated);
        assert_eq!(classify_pair("CBLTC-CBBTC"), PairRiskClass::BtcCorrelated);
        assert_eq!(classify_pair("USDC-AERO"), PairRiskClass::NativeOrIncentive);
        assert_eq!(classify_pair("WETH-LCAP"), PairRiskClass::LongTail);
    }
}
