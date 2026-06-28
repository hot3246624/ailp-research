use autopool_core::{
    Bps, Chain, DexProtocol, ExposureKind, IlRisk, PoolKey, Tick, Usd, YieldSnapshot,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use url::Url;

const ORCA_BASE_URL: &str = "https://api.orca.so";
const RAYDIUM_BASE_URL: &str = "https://api-v3.raydium.io";
const METEORA_BASE_URL: &str = "https://dlmm.datapi.meteora.ag";

#[derive(Debug, Error)]
pub enum SolanaDiscoveryError {
    #[error("invalid url: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("unexpected api response: {0}")]
    ApiShape(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SolanaVenue {
    OrcaWhirlpool,
    RaydiumClmm,
    MeteoraDlmm,
}

impl SolanaVenue {
    pub fn label(self) -> &'static str {
        match self {
            Self::OrcaWhirlpool => "orca",
            Self::RaydiumClmm => "raydium",
            Self::MeteoraDlmm => "meteora",
        }
    }

    pub fn protocol(self) -> DexProtocol {
        match self {
            Self::OrcaWhirlpool => DexProtocol::Orca,
            Self::RaydiumClmm => DexProtocol::Raydium,
            Self::MeteoraDlmm => DexProtocol::Meteora,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryOptions {
    pub min_tvl_usd: Usd,
    pub page_size: usize,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            min_tvl_usd: 100_000.0,
            page_size: 100,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolanaToken {
    pub address: String,
    pub symbol: String,
    pub name: Option<String>,
    pub decimals: Option<u8>,
    pub verified: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolanaPoolCandidate {
    pub venue: SolanaVenue,
    pub protocol: DexProtocol,
    pub pool_kind: String,
    pub pool_address: String,
    pub symbol: String,
    pub tokens: Vec<SolanaToken>,
    pub fee_bps: Option<Bps>,
    pub tick_spacing: Option<Tick>,
    pub bin_step: Option<i32>,
    pub tvl_usd: Usd,
    pub volume_usd_24h: Option<Usd>,
    pub volume_usd_7d: Option<Usd>,
    pub fees_usd_24h: Option<Usd>,
    pub fees_usd_7d: Option<Usd>,
    pub fee_apr_24h: Option<f64>,
    pub fee_apr_7d: Option<f64>,
    pub reward_apr: Option<f64>,
    pub total_apr: Option<f64>,
    pub current_price: Option<f64>,
    pub current_tick: Option<Tick>,
    pub active_liquidity: Option<String>,
    pub updated_slot: Option<u64>,
    pub verified: bool,
    pub warnings: Vec<String>,
    pub deployability_score: f64,
}

impl SolanaPoolCandidate {
    pub fn into_yield_snapshot(self) -> YieldSnapshot {
        let apy_base = self.fee_apr_24h.or(self.fee_apr_7d);
        YieldSnapshot {
            source: self.venue.label().to_string(),
            pool: PoolKey {
                chain: Chain::Other {
                    name: "Solana".to_string(),
                },
                protocol: self.protocol,
                source_id: self.pool_address.clone(),
                address: Some(self.pool_address),
                symbol: self.symbol,
                fee_tier_bps: self.fee_bps,
                tick_spacing: self.tick_spacing,
            },
            tvl_usd: Some(self.tvl_usd),
            apy: self.total_apr.or(apy_base),
            apy_base,
            apy_reward: self.reward_apr,
            volume_usd_1d: self.volume_usd_24h,
            volume_usd_7d: self.volume_usd_7d,
            il_risk: IlRisk::Yes,
            exposure: ExposureKind::Multi,
            stablecoin: None,
            outlier: false,
            mu: None,
            sigma: None,
            predicted_class: None,
            predicted_probability: None,
            underlying_tokens: self
                .tokens
                .into_iter()
                .map(|token| token.address)
                .collect::<Vec<_>>(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SolanaDiscoveryClient {
    http: reqwest::Client,
    orca_base_url: Url,
    raydium_base_url: Url,
    meteora_base_url: Url,
}

impl Default for SolanaDiscoveryClient {
    fn default() -> Self {
        Self::new(ORCA_BASE_URL, RAYDIUM_BASE_URL, METEORA_BASE_URL)
            .expect("default Solana discovery URLs are valid")
    }
}

impl SolanaDiscoveryClient {
    pub fn new(
        orca_base_url: &str,
        raydium_base_url: &str,
        meteora_base_url: &str,
    ) -> Result<Self, SolanaDiscoveryError> {
        Ok(Self {
            http: reqwest::Client::new(),
            orca_base_url: Url::parse(orca_base_url)?,
            raydium_base_url: Url::parse(raydium_base_url)?,
            meteora_base_url: Url::parse(meteora_base_url)?,
        })
    }

    pub async fn discover_many(
        &self,
        venues: &[SolanaVenue],
        options: &DiscoveryOptions,
    ) -> Result<Vec<SolanaPoolCandidate>, SolanaDiscoveryError> {
        let mut rows = Vec::new();
        for venue in venues {
            rows.extend(self.discover(*venue, options).await?);
        }
        Ok(rows)
    }

    pub async fn discover(
        &self,
        venue: SolanaVenue,
        options: &DiscoveryOptions,
    ) -> Result<Vec<SolanaPoolCandidate>, SolanaDiscoveryError> {
        match venue {
            SolanaVenue::OrcaWhirlpool => self.fetch_orca(options).await,
            SolanaVenue::RaydiumClmm => self.fetch_raydium(options).await,
            SolanaVenue::MeteoraDlmm => self.fetch_meteora(options).await,
        }
    }

    async fn fetch_orca(
        &self,
        options: &DiscoveryOptions,
    ) -> Result<Vec<SolanaPoolCandidate>, SolanaDiscoveryError> {
        let mut url = self.orca_base_url.join("/v2/solana/pools")?;
        url.query_pairs_mut()
            .append_pair("minTvl", &format!("{:.0}", options.min_tvl_usd))
            .append_pair("sortBy", "tvl")
            .append_pair("sortDirection", "desc")
            .append_pair("size", &options.page_size.to_string())
            .append_pair("stats", "24h,7d");
        let response = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let pools = array_at(&response, &["data"])?;
        Ok(pools.iter().filter_map(parse_orca_pool).collect())
    }

    async fn fetch_raydium(
        &self,
        options: &DiscoveryOptions,
    ) -> Result<Vec<SolanaPoolCandidate>, SolanaDiscoveryError> {
        let mut url = self.raydium_base_url.join("/pools/info/list-v2")?;
        url.query_pairs_mut()
            .append_pair("poolType", "Concentrated")
            .append_pair("sortField", "liquidity")
            .append_pair("sortType", "desc")
            .append_pair("size", &options.page_size.to_string());
        let response = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let pools = array_at(&response, &["data", "data"])?;
        Ok(pools
            .iter()
            .filter_map(parse_raydium_pool)
            .filter(|pool| pool.tvl_usd >= options.min_tvl_usd)
            .collect())
    }

    async fn fetch_meteora(
        &self,
        options: &DiscoveryOptions,
    ) -> Result<Vec<SolanaPoolCandidate>, SolanaDiscoveryError> {
        let mut url = self.meteora_base_url.join("/pools")?;
        url.query_pairs_mut()
            .append_pair("page", "1")
            .append_pair("page_size", &options.page_size.to_string())
            .append_pair("sort_by", "tvl:desc")
            .append_pair("filter_by", "is_blacklisted=false");
        let response = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let pools = array_at(&response, &["data"])?;
        Ok(pools
            .iter()
            .filter_map(parse_meteora_pool)
            .filter(|pool| pool.tvl_usd >= options.min_tvl_usd)
            .collect())
    }
}

fn parse_orca_pool(pool: &Value) -> Option<SolanaPoolCandidate> {
    let pool_address = string_at(pool, &["address"])?;
    let token_a = token_from_value(
        value_at(pool, &["tokenA"])?,
        "address",
        "symbol",
        "name",
        "decimals",
        None,
    )?;
    let token_b = token_from_value(
        value_at(pool, &["tokenB"])?,
        "address",
        "symbol",
        "name",
        "decimals",
        None,
    )?;
    let tvl_usd = f64_at(pool, &["tvlUsdc"])?;
    let volume_usd_24h = f64_at(pool, &["stats", "24h", "volume"]);
    let volume_usd_7d = f64_at(pool, &["stats", "7d", "volume"]);
    let fees_usd_24h = f64_at(pool, &["stats", "24h", "fees"]);
    let fees_usd_7d = f64_at(pool, &["stats", "7d", "fees"]);
    let fee_apr_24h = f64_at(pool, &["stats", "24h", "yieldOverTvl"])
        .map(|value| annualize_period_yield(value, 1.0))
        .or_else(|| fee_apr_from_fees(fees_usd_24h, tvl_usd, 1.0));
    let fee_apr_7d = f64_at(pool, &["stats", "7d", "yieldOverTvl"])
        .map(|value| annualize_period_yield(value, 7.0))
        .or_else(|| fee_apr_from_fees(fees_usd_7d, tvl_usd, 7.0));
    let symbol = format!("{}-{}", token_a.symbol, token_b.symbol);
    let mut row = SolanaPoolCandidate {
        venue: SolanaVenue::OrcaWhirlpool,
        protocol: DexProtocol::Orca,
        pool_kind: "whirlpool".to_string(),
        pool_address,
        symbol,
        tokens: vec![token_a, token_b],
        fee_bps: f64_at(pool, &["feeRate"]).map(orca_fee_rate_to_bps),
        tick_spacing: i32_at(pool, &["tickSpacing"]),
        bin_step: None,
        tvl_usd,
        volume_usd_24h,
        volume_usd_7d,
        fees_usd_24h,
        fees_usd_7d,
        fee_apr_24h,
        fee_apr_7d,
        reward_apr: None,
        total_apr: fee_apr_24h,
        current_price: f64_at(pool, &["price"]),
        current_tick: i32_at(pool, &["tickCurrentIndex"]),
        active_liquidity: string_at(pool, &["liquidity"]),
        updated_slot: u64_at(pool, &["updatedSlot"]),
        verified: !bool_at(pool, &["hasWarning"]).unwrap_or(false),
        warnings: Vec::new(),
        deployability_score: 0.0,
    };
    finalize_candidate(&mut row);
    Some(row)
}

fn parse_raydium_pool(pool: &Value) -> Option<SolanaPoolCandidate> {
    let pool_address = string_at(pool, &["id"])?;
    let token_a = token_from_value(
        value_at(pool, &["mintA"])?,
        "address",
        "symbol",
        "name",
        "decimals",
        None,
    )?;
    let token_b = token_from_value(
        value_at(pool, &["mintB"])?,
        "address",
        "symbol",
        "name",
        "decimals",
        None,
    )?;
    let tvl_usd = f64_at(pool, &["tvl"])?;
    let volume_usd_24h = f64_at(pool, &["day", "volume"]);
    let volume_usd_7d = f64_at(pool, &["week", "volume"]);
    let fees_usd_24h = f64_at(pool, &["day", "volumeFee"]);
    let fees_usd_7d = f64_at(pool, &["week", "volumeFee"]);
    let reward_apr = sum_f64_array_at(pool, &["day", "rewardApr"]);
    let fee_apr_24h =
        f64_at(pool, &["day", "feeApr"]).or_else(|| fee_apr_from_fees(fees_usd_24h, tvl_usd, 1.0));
    let fee_apr_7d =
        f64_at(pool, &["week", "feeApr"]).or_else(|| fee_apr_from_fees(fees_usd_7d, tvl_usd, 7.0));
    let symbol = format!("{}-{}", token_a.symbol, token_b.symbol);
    let mut row = SolanaPoolCandidate {
        venue: SolanaVenue::RaydiumClmm,
        protocol: DexProtocol::Raydium,
        pool_kind: "clmm".to_string(),
        pool_address,
        symbol,
        tokens: vec![token_a, token_b],
        fee_bps: f64_at(pool, &["feeRate"]).map(fraction_fee_rate_to_bps),
        tick_spacing: i32_at(pool, &["config", "tickSpacing"]),
        bin_step: None,
        tvl_usd,
        volume_usd_24h,
        volume_usd_7d,
        fees_usd_24h,
        fees_usd_7d,
        fee_apr_24h,
        fee_apr_7d,
        reward_apr,
        total_apr: f64_at(pool, &["day", "apr"]),
        current_price: f64_at(pool, &["price"]),
        current_tick: None,
        active_liquidity: None,
        updated_slot: None,
        verified: true,
        warnings: Vec::new(),
        deployability_score: 0.0,
    };
    finalize_candidate(&mut row);
    Some(row)
}

fn parse_meteora_pool(pool: &Value) -> Option<SolanaPoolCandidate> {
    let pool_address = string_at(pool, &["address"])?;
    let token_a = token_from_value(
        value_at(pool, &["token_x"])?,
        "address",
        "symbol",
        "name",
        "decimals",
        Some("is_verified"),
    )?;
    let token_b = token_from_value(
        value_at(pool, &["token_y"])?,
        "address",
        "symbol",
        "name",
        "decimals",
        Some("is_verified"),
    )?;
    let tvl_usd = f64_at(pool, &["tvl"])?;
    let volume_usd_24h = f64_at(pool, &["volume", "24h"]);
    let fees_usd_24h = f64_at(pool, &["fees", "24h"]);
    let fee_apr_24h = f64_at(pool, &["apy"])
        .map(fractionish_to_percent)
        .or_else(|| {
            f64_at(pool, &["fee_tvl_ratio", "24h"]).map(|value| annualize_period_yield(value, 1.0))
        })
        .or_else(|| fee_apr_from_fees(fees_usd_24h, tvl_usd, 1.0));
    let fee_apr_from_daily_ratio = f64_at(pool, &["fee_tvl_ratio", "24h"])
        .map(|value| annualize_period_yield(value, 1.0))
        .or_else(|| fee_apr_from_fees(fees_usd_24h, tvl_usd, 1.0));
    let reward_apr = f64_at(pool, &["farm_apy"]).map(fractionish_to_percent);
    let symbol = string_at(pool, &["name"])
        .unwrap_or_else(|| format!("{}-{}", token_a.symbol, token_b.symbol));
    let verified = !bool_at(pool, &["is_blacklisted"]).unwrap_or(false)
        && token_a.verified.unwrap_or(true)
        && token_b.verified.unwrap_or(true);
    let mut row = SolanaPoolCandidate {
        venue: SolanaVenue::MeteoraDlmm,
        protocol: DexProtocol::Meteora,
        pool_kind: "dlmm".to_string(),
        pool_address,
        symbol,
        tokens: vec![token_a, token_b],
        fee_bps: f64_at(pool, &["pool_config", "base_fee_pct"]).map(percent_to_bps),
        tick_spacing: None,
        bin_step: i32_at(pool, &["pool_config", "bin_step"]),
        tvl_usd,
        volume_usd_24h,
        volume_usd_7d: None,
        fees_usd_24h,
        fees_usd_7d: None,
        fee_apr_24h,
        fee_apr_7d: None,
        reward_apr,
        total_apr: fee_apr_24h,
        current_price: f64_at(pool, &["current_price"]),
        current_tick: None,
        active_liquidity: None,
        updated_slot: None,
        verified,
        warnings: Vec::new(),
        deployability_score: 0.0,
    };
    if let (Some(api_apy), Some(daily_ratio_apr)) = (fee_apr_24h, fee_apr_from_daily_ratio) {
        if daily_ratio_apr > api_apy * 5.0 && daily_ratio_apr > 500.0 {
            row.warnings
                .push("meteora_daily_ratio_disagrees_with_apy".to_string());
        }
    }
    finalize_candidate(&mut row);
    Some(row)
}

fn finalize_candidate(row: &mut SolanaPoolCandidate) {
    if row.fee_bps.is_none() {
        row.warnings.push("fee_missing".to_string());
    }
    if !row.verified {
        row.warnings.push("unverified_or_warning".to_string());
    }
    if let Some(volume) = row.volume_usd_24h {
        if row.tvl_usd > 0.0 && volume / row.tvl_usd > 3.0 {
            row.warnings.push("high_turnover".to_string());
        }
    }
    if row.fee_apr_24h.unwrap_or(0.0) > 1_000.0 {
        row.warnings.push("fee_apr_outlier".to_string());
    }
    if major_token_score(&row.tokens) < 0.5 {
        row.warnings.push("long_tail_inventory".to_string());
    }
    row.deployability_score = deployability_score(row);
}

fn deployability_score(row: &SolanaPoolCandidate) -> f64 {
    let fee_apr = row.fee_apr_24h.or(row.fee_apr_7d).unwrap_or(0.0).max(0.0);
    let volume = row.volume_usd_24h.unwrap_or(0.0).max(0.0);
    let liquidity_weight = (1.0 + row.tvl_usd.max(0.0) / 100_000.0).ln();
    let flow_weight = (1.0 + volume / 100_000.0).ln();
    let token_weight = 0.6 + major_token_score(&row.tokens);
    let verification_weight = if row.verified { 1.0 } else { 0.55 };
    fee_apr * liquidity_weight * flow_weight * token_weight * verification_weight
}

fn major_token_score(tokens: &[SolanaToken]) -> f64 {
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

fn token_from_value(
    value: &Value,
    address_key: &str,
    symbol_key: &str,
    name_key: &str,
    decimals_key: &str,
    verified_key: Option<&str>,
) -> Option<SolanaToken> {
    let decimals = u64_at(value, &[decimals_key]).and_then(|value| u8::try_from(value).ok());
    Some(SolanaToken {
        address: string_at(value, &[address_key])?,
        symbol: string_at(value, &[symbol_key])?,
        name: string_at(value, &[name_key]),
        decimals,
        verified: verified_key.and_then(|key| bool_at(value, &[key])),
    })
}

fn annualize_period_yield(period_yield_fraction: f64, days: f64) -> f64 {
    if days <= 0.0 {
        0.0
    } else {
        period_yield_fraction / days * 365.0 * 100.0
    }
}

fn fee_apr_from_fees(fees: Option<Usd>, tvl_usd: Usd, days: f64) -> Option<f64> {
    let fees = fees?;
    if tvl_usd <= 0.0 || days <= 0.0 {
        return None;
    }
    Some(fees / tvl_usd / days * 365.0 * 100.0)
}

fn orca_fee_rate_to_bps(raw: f64) -> Bps {
    raw / 100.0
}

fn fraction_fee_rate_to_bps(raw: f64) -> Bps {
    raw * 10_000.0
}

fn percent_to_bps(percent: f64) -> Bps {
    percent * 100.0
}

fn fractionish_to_percent(value: f64) -> f64 {
    if value.abs() <= 10.0 {
        value * 100.0
    } else {
        value
    }
}

fn array_at<'a>(value: &'a Value, path: &[&str]) -> Result<&'a Vec<Value>, SolanaDiscoveryError> {
    value_at(value, path)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            SolanaDiscoveryError::ApiShape(format!("missing array at {}", path.join(".")))
        })
}

fn value_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

fn string_at(value: &Value, path: &[&str]) -> Option<String> {
    let value = value_at(value, path)?;
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    if value.is_number() || value.is_boolean() {
        return Some(value.to_string());
    }
    None
}

fn f64_at(value: &Value, path: &[&str]) -> Option<f64> {
    let value = value_at(value, path)?;
    if let Some(number) = value.as_f64() {
        return Some(number);
    }
    value.as_str()?.parse::<f64>().ok()
}

fn i32_at(value: &Value, path: &[&str]) -> Option<i32> {
    let raw = i64_at(value, path)?;
    i32::try_from(raw).ok()
}

fn i64_at(value: &Value, path: &[&str]) -> Option<i64> {
    let value = value_at(value, path)?;
    if let Some(number) = value.as_i64() {
        return Some(number);
    }
    value.as_str()?.parse::<i64>().ok()
}

fn u64_at(value: &Value, path: &[&str]) -> Option<u64> {
    let value = value_at(value, path)?;
    if let Some(number) = value.as_u64() {
        return Some(number);
    }
    value.as_str()?.parse::<u64>().ok()
}

fn bool_at(value: &Value, path: &[&str]) -> Option<bool> {
    let value = value_at(value, path)?;
    if let Some(flag) = value.as_bool() {
        return Some(flag);
    }
    value.as_str()?.parse::<bool>().ok()
}

fn sum_f64_array_at(value: &Value, path: &[&str]) -> Option<f64> {
    let values = value_at(value, path)?.as_array()?;
    Some(values.iter().filter_map(Value::as_f64).sum())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_protocol_fee_units() {
        assert_eq!(orca_fee_rate_to_bps(400.0), 4.0);
        assert_eq!(fraction_fee_rate_to_bps(0.0001), 1.0);
        assert_eq!(percent_to_bps(0.1), 10.0);
    }

    #[test]
    fn annualizes_period_yield_as_percent_apr() {
        let apr = annualize_period_yield(0.001, 1.0);
        assert!((apr - 36.5).abs() < 1e-9);
        let seven_day_apr = annualize_period_yield(0.007, 7.0);
        assert!((seven_day_apr - 36.5).abs() < 1e-9);
    }

    #[test]
    fn reads_numeric_strings_and_numbers() {
        let value = serde_json::json!({
            "string": "123.45",
            "number": 456.0,
            "nested": { "flag": "true" }
        });
        assert_eq!(f64_at(&value, &["string"]), Some(123.45));
        assert_eq!(f64_at(&value, &["number"]), Some(456.0));
        assert_eq!(bool_at(&value, &["nested", "flag"]), Some(true));
    }
}
