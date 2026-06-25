use autopool_core::{Chain, DexProtocol, ExposureKind, IlRisk, PoolKey, Usd, YieldSnapshot};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

const DEFAULT_BASE_URL: &str = "https://yields.llama.fi";

#[derive(Debug, Error)]
pub enum DefiLlamaError {
    #[error("invalid base url: {0}")]
    InvalidBaseUrl(#[from] url::ParseError),
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
}

#[derive(Debug, Clone)]
pub struct DefiLlamaClient {
    http: reqwest::Client,
    base_url: Url,
}

impl Default for DefiLlamaClient {
    fn default() -> Self {
        Self::new(DEFAULT_BASE_URL).expect("default DeFiLlama base URL is valid")
    }
}

impl DefiLlamaClient {
    pub fn new(base_url: &str) -> Result<Self, DefiLlamaError> {
        Ok(Self {
            http: reqwest::Client::new(),
            base_url: Url::parse(base_url)?,
        })
    }

    pub async fn fetch_pools(&self) -> Result<Vec<DefiLlamaPool>, DefiLlamaError> {
        let url = self.base_url.join("/pools")?;
        let response = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json::<DefiLlamaPoolsResponse>()
            .await?;
        Ok(response.data)
    }

    pub async fn fetch_snapshots(
        &self,
        filter: &PoolFilter,
    ) -> Result<Vec<YieldSnapshot>, DefiLlamaError> {
        let pools = self.fetch_pools().await?;
        Ok(pools
            .into_iter()
            .filter(|pool| filter.matches(pool))
            .map(|pool| pool.into_snapshot())
            .collect())
    }
}

#[derive(Debug, Clone, Default)]
pub struct PoolFilter {
    pub chains: Vec<String>,
    pub projects: Vec<String>,
    pub min_tvl_usd: Option<Usd>,
    pub lp_only: bool,
    pub dex_only: bool,
    pub exclude_outliers: bool,
}

impl PoolFilter {
    pub fn matches(&self, pool: &DefiLlamaPool) -> bool {
        if !self.chains.is_empty()
            && !self
                .chains
                .iter()
                .any(|chain| chain.eq_ignore_ascii_case(&pool.chain))
        {
            return false;
        }

        if !self.projects.is_empty()
            && !self
                .projects
                .iter()
                .any(|project| project.eq_ignore_ascii_case(&pool.project))
        {
            return false;
        }

        if let Some(min_tvl_usd) = self.min_tvl_usd {
            if pool.tvl_usd.unwrap_or(0.0) < min_tvl_usd {
                return false;
            }
        }

        if self.lp_only
            && !(pool.il_risk.as_deref() == Some("yes")
                || pool.exposure.as_deref() == Some("multi"))
        {
            return false;
        }

        if self.dex_only && !is_known_dex_project(&pool.project) {
            return false;
        }

        if self.exclude_outliers && pool.outlier.unwrap_or(false) {
            return false;
        }

        true
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefiLlamaPoolsResponse {
    pub data: Vec<DefiLlamaPool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefiLlamaPrediction {
    #[serde(rename = "predictedClass")]
    pub predicted_class: Option<String>,
    #[serde(rename = "predictedProbability")]
    pub predicted_probability: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefiLlamaPool {
    pub chain: String,
    pub project: String,
    pub symbol: String,
    #[serde(rename = "tvlUsd")]
    pub tvl_usd: Option<Usd>,
    #[serde(default)]
    pub apy: Option<f64>,
    #[serde(rename = "apyBase")]
    pub apy_base: Option<f64>,
    #[serde(rename = "apyReward")]
    pub apy_reward: Option<f64>,
    #[serde(rename = "volumeUsd1d")]
    pub volume_usd_1d: Option<Usd>,
    #[serde(rename = "volumeUsd7d")]
    pub volume_usd_7d: Option<Usd>,
    #[serde(rename = "ilRisk")]
    pub il_risk: Option<String>,
    pub exposure: Option<String>,
    pub stablecoin: Option<bool>,
    pub outlier: Option<bool>,
    pub mu: Option<f64>,
    pub sigma: Option<f64>,
    pub predictions: Option<DefiLlamaPrediction>,
    #[serde(rename = "poolMeta")]
    pub pool_meta: Option<String>,
    #[serde(rename = "underlyingTokens")]
    pub underlying_tokens: Option<Vec<String>>,
    pub pool: String,
}

impl DefiLlamaPool {
    pub fn into_snapshot(self) -> YieldSnapshot {
        let prediction = self.predictions.clone();
        YieldSnapshot {
            source: "defillama".to_string(),
            pool: PoolKey {
                chain: chain_from_defillama(&self.chain),
                protocol: protocol_from_project(&self.project),
                source_id: self.pool.clone(),
                address: address_from_pool_id(&self.pool),
                symbol: self.symbol.clone(),
                fee_tier_bps: fee_tier_bps_from_meta(self.pool_meta.as_deref()),
                tick_spacing: tick_spacing_from_meta(self.pool_meta.as_deref()),
            },
            tvl_usd: self.tvl_usd,
            apy: self.apy,
            apy_base: self.apy_base,
            apy_reward: self.apy_reward,
            volume_usd_1d: self.volume_usd_1d,
            volume_usd_7d: self.volume_usd_7d,
            il_risk: il_risk_from_str(self.il_risk.as_deref()),
            exposure: exposure_from_str(self.exposure.as_deref()),
            stablecoin: self.stablecoin,
            outlier: self.outlier.unwrap_or(false),
            mu: self.mu,
            sigma: self.sigma,
            predicted_class: prediction
                .as_ref()
                .and_then(|value| value.predicted_class.clone()),
            predicted_probability: prediction.and_then(|value| value.predicted_probability),
            underlying_tokens: self.underlying_tokens.unwrap_or_default(),
        }
    }
}

fn chain_from_defillama(name: &str) -> Chain {
    match name {
        "Ethereum" => Chain::evm(1, name),
        "Optimism" | "OP Mainnet" => Chain::evm(10, name),
        "BSC" => Chain::evm(56, name),
        "Polygon" => Chain::evm(137, name),
        "Base" => Chain::evm(8453, name),
        "Arbitrum" => Chain::evm(42161, name),
        "Avalanche" => Chain::evm(43114, name),
        _ => Chain::Other {
            name: name.to_string(),
        },
    }
}

fn protocol_from_project(project: &str) -> DexProtocol {
    match project {
        "uniswap-v3" => DexProtocol::UniswapV3,
        "uniswap-v4" => DexProtocol::UniswapV4,
        "aerodrome-v1" | "aerodrome-slipstream" => DexProtocol::Aerodrome,
        "curve-dex" => DexProtocol::Curve,
        "balancer-v3" | "balancer-v2" => DexProtocol::Balancer,
        "sushiswap" => DexProtocol::SushiSwap,
        _ => DexProtocol::Unknown(project.to_string()),
    }
}

fn is_known_dex_project(project: &str) -> bool {
    matches!(
        project,
        "uniswap-v3"
            | "uniswap-v4"
            | "aerodrome-v1"
            | "aerodrome-slipstream"
            | "curve-dex"
            | "balancer-v3"
            | "balancer-v2"
            | "sushiswap"
    )
}

fn il_risk_from_str(value: Option<&str>) -> IlRisk {
    match value {
        Some("no") => IlRisk::None,
        Some("yes") => IlRisk::Yes,
        _ => IlRisk::Unknown,
    }
}

fn exposure_from_str(value: Option<&str>) -> ExposureKind {
    match value {
        Some("single") => ExposureKind::Single,
        Some("multi") => ExposureKind::Multi,
        _ => ExposureKind::Unknown,
    }
}

fn address_from_pool_id(pool_id: &str) -> Option<String> {
    pool_id
        .split(|character: char| !character.is_ascii_hexdigit() && character != 'x')
        .find(|part| part.starts_with("0x") && part.len() == 42)
        .map(|part| part.to_ascii_lowercase())
}

fn fee_tier_bps_from_meta(pool_meta: Option<&str>) -> Option<f64> {
    let meta = pool_meta?
        .split_whitespace()
        .rev()
        .find_map(|part| part.strip_suffix('%'))?;
    let percent = meta.parse::<f64>().ok()?;
    Some(percent * 100.0)
}

fn tick_spacing_from_meta(pool_meta: Option<&str>) -> Option<i32> {
    let meta = pool_meta?.trim();
    let cl_prefix = meta.split_whitespace().next()?;
    cl_prefix.strip_prefix("CL")?.parse::<i32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_fee_tier_from_percent_meta() {
        assert_eq!(fee_tier_bps_from_meta(Some("0.3%")), Some(30.0));
        assert_eq!(fee_tier_bps_from_meta(Some("0.05%")), Some(5.0));
        assert_eq!(fee_tier_bps_from_meta(Some("CL100 - 0.05%")), Some(5.0));
        assert_eq!(fee_tier_bps_from_meta(Some("CL50 - 0.0079%")), Some(0.79));
    }

    #[test]
    fn extracts_tick_spacing_from_cl_meta() {
        assert_eq!(tick_spacing_from_meta(Some("CL50 - 0.0079%")), Some(50));
        assert_eq!(tick_spacing_from_meta(Some("CL1 - 0.0009%")), Some(1));
        assert_eq!(tick_spacing_from_meta(Some("0.3%")), None);
    }
}
