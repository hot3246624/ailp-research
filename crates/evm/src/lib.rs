use async_trait::async_trait;
use autopool_core::{CoreError, PoolKey, PoolMarketState, PositionState, RangeSpec, Usd};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvmError {
    #[error("unsupported protocol for EVM adapter")]
    UnsupportedProtocol,
    #[error("provider error: {0}")]
    Provider(String),
    #[error("rpc error: {0}")]
    Rpc(String),
    #[error("invalid rpc response: {0}")]
    InvalidRpcResponse(String),
}

impl From<EvmError> for CoreError {
    fn from(value: EvmError) -> Self {
        CoreError::Message(value.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmChainConfig {
    pub chain_id: u64,
    pub name: String,
    pub rpc_url_env: String,
    pub private_relay_url_env: Option<String>,
    pub max_gas_usd: Usd,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSample {
    pub chain_id: u64,
    pub block_number: u64,
    pub gas_price_wei: u128,
    pub gas_price_gwei: f64,
    pub rebalance_gas_units: u64,
    pub estimated_rebalance_gas_eth: f64,
    pub estimated_rebalance_gas_usd: Option<Usd>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClPoolState {
    pub pool_address: String,
    pub current_tick: i32,
    pub sqrt_price_x96_hex: String,
    pub liquidity: String,
    pub tick_spacing: i32,
    pub fee_growth_global0_x128_hex: String,
    pub fee_growth_global1_x128_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EthLog {
    pub address: String,
    pub topics: Vec<String>,
    pub data: String,
    #[serde(rename = "blockNumber")]
    pub block_number: Option<String>,
    #[serde(rename = "transactionHash")]
    pub transaction_hash: Option<String>,
    #[serde(rename = "logIndex")]
    pub log_index: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolEventSummary {
    pub pool_address: String,
    pub from_block: u64,
    pub to_block: u64,
    pub swap_count: usize,
    pub mint_count: usize,
    pub burn_count: usize,
    pub collect_count: usize,
    pub latest_swap_block: Option<u64>,
    pub latest_swap_tick: Option<i32>,
}

pub const SWAP_TOPIC: &str = "0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67";
pub const MINT_TOPIC: &str = "0x7a53080ba414158be7ec69b987b5fb7d07dee101fe85488f0853ae16239d0bde";
pub const BURN_TOPIC: &str = "0x0c396cd989a39f4459b5fa1aed6a9a8dcdbc45908acfd67e028cd568da98982c";
pub const COLLECT_TOPIC: &str =
    "0x70935338e69775456a85ddef226c395fb668b63fa0115f5f20610b388e6ca9c0";

#[derive(Debug, Clone)]
pub struct JsonRpcClient {
    http: reqwest::Client,
    rpc_url: String,
}

impl JsonRpcClient {
    pub fn new(rpc_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            rpc_url: rpc_url.into(),
        }
    }

    pub async fn sample_network(
        &self,
        chain_id: u64,
        rebalance_gas_units: u64,
        eth_usd: Option<Usd>,
    ) -> Result<NetworkSample, EvmError> {
        let block_number_hex = self.call("eth_blockNumber", json!([])).await?;
        let gas_price_hex = self.call("eth_gasPrice", json!([])).await?;
        let block_number = parse_hex_u64(&block_number_hex)?;
        let gas_price_wei = parse_hex_u128(&gas_price_hex)?;
        let gas_price_gwei = gas_price_wei as f64 / 1e9;
        let estimated_rebalance_gas_eth = gas_price_wei as f64 * rebalance_gas_units as f64 / 1e18;
        let estimated_rebalance_gas_usd = eth_usd.map(|price| estimated_rebalance_gas_eth * price);

        Ok(NetworkSample {
            chain_id,
            block_number,
            gas_price_wei,
            gas_price_gwei,
            rebalance_gas_units,
            estimated_rebalance_gas_eth,
            estimated_rebalance_gas_usd,
        })
    }

    pub async fn latest_block_number(&self) -> Result<u64, EvmError> {
        let block_number_hex = self.call("eth_blockNumber", json!([])).await?;
        parse_hex_u64(&block_number_hex)
    }

    pub async fn eth_call(&self, to: &str, data: &str) -> Result<String, EvmError> {
        self.call(
            "eth_call",
            json!([
                {
                    "to": to,
                    "data": data,
                },
                "latest"
            ]),
        )
        .await
    }

    pub async fn get_cl_pool(
        &self,
        factory: &str,
        token_a: &str,
        token_b: &str,
        tick_spacing: i32,
    ) -> Result<Option<String>, EvmError> {
        let calldata = encode_get_pool_call(token_a, token_b, tick_spacing)?;
        let result = self.eth_call(factory, &calldata).await?;
        Ok(decode_address_result(&result))
    }

    pub async fn read_cl_pool_state(&self, pool_address: &str) -> Result<ClPoolState, EvmError> {
        let slot0 = self.eth_call(pool_address, "0x3850c7bd").await?;
        let liquidity = self.eth_call(pool_address, "0x1a686502").await?;
        let tick_spacing = self.eth_call(pool_address, "0xd0c93a7c").await?;
        let fee_growth0 = self.eth_call(pool_address, "0xf3058399").await?;
        let fee_growth1 = self.eth_call(pool_address, "0x46141319").await?;
        let words = decode_words(&slot0)?;

        Ok(ClPoolState {
            pool_address: pool_address.to_string(),
            current_tick: decode_i24_word(words.get(1).ok_or_else(|| {
                EvmError::InvalidRpcResponse("slot0 missing tick word".to_string())
            })?)?,
            sqrt_price_x96_hex: words
                .first()
                .ok_or_else(|| {
                    EvmError::InvalidRpcResponse("slot0 missing sqrt price word".to_string())
                })?
                .to_string(),
            liquidity: parse_hex_u128(&liquidity)?.to_string(),
            tick_spacing: decode_i24_result(&tick_spacing)?,
            fee_growth_global0_x128_hex: normalize_hex_word(&fee_growth0)?,
            fee_growth_global1_x128_hex: normalize_hex_word(&fee_growth1)?,
        })
    }

    pub async fn get_logs(
        &self,
        address: &str,
        from_block: u64,
        to_block: u64,
        topic0: &str,
    ) -> Result<Vec<EthLog>, EvmError> {
        let response = self
            .call_value(
                "eth_getLogs",
                json!([{
                    "address": address,
                    "fromBlock": format!("0x{from_block:x}"),
                    "toBlock": format!("0x{to_block:x}"),
                    "topics": [topic0],
                }]),
            )
            .await?;

        serde_json::from_value::<Vec<EthLog>>(response)
            .map_err(|err| EvmError::InvalidRpcResponse(err.to_string()))
    }

    pub async fn get_logs_chunked(
        &self,
        address: &str,
        from_block: u64,
        to_block: u64,
        topic0: &str,
        max_blocks_per_request: u64,
    ) -> Result<Vec<EthLog>, EvmError> {
        let chunk_size = max_blocks_per_request.max(1);
        let mut cursor = from_block;
        let mut logs = Vec::new();

        while cursor <= to_block {
            let chunk_end = cursor
                .saturating_add(chunk_size.saturating_sub(1))
                .min(to_block);
            logs.extend(self.get_logs(address, cursor, chunk_end, topic0).await?);
            if chunk_end == u64::MAX {
                break;
            }
            cursor = chunk_end + 1;
        }

        Ok(logs)
    }

    pub async fn pool_event_summary(
        &self,
        pool_address: &str,
        from_block: u64,
        to_block: u64,
    ) -> Result<PoolEventSummary, EvmError> {
        let swaps = self
            .get_logs(pool_address, from_block, to_block, SWAP_TOPIC)
            .await?;
        let mints = self
            .get_logs(pool_address, from_block, to_block, MINT_TOPIC)
            .await?;
        let burns = self
            .get_logs(pool_address, from_block, to_block, BURN_TOPIC)
            .await?;
        let collects = self
            .get_logs(pool_address, from_block, to_block, COLLECT_TOPIC)
            .await?;
        let latest_swap = swaps
            .iter()
            .filter_map(|log| {
                let block = log
                    .block_number
                    .as_deref()
                    .and_then(|value| parse_hex_u64(value).ok())?;
                let tick = decode_swap_tick(&log.data).ok()?;
                Some((block, tick))
            })
            .max_by_key(|(block, _)| *block);

        Ok(PoolEventSummary {
            pool_address: pool_address.to_string(),
            from_block,
            to_block,
            swap_count: swaps.len(),
            mint_count: mints.len(),
            burn_count: burns.len(),
            collect_count: collects.len(),
            latest_swap_block: latest_swap.map(|(block, _)| block),
            latest_swap_tick: latest_swap.map(|(_, tick)| tick),
        })
    }

    pub async fn pool_event_summary_chunked(
        &self,
        pool_address: &str,
        from_block: u64,
        to_block: u64,
        max_blocks_per_request: u64,
    ) -> Result<PoolEventSummary, EvmError> {
        let swaps = self
            .get_logs_chunked(
                pool_address,
                from_block,
                to_block,
                SWAP_TOPIC,
                max_blocks_per_request,
            )
            .await?;
        let mints = self
            .get_logs_chunked(
                pool_address,
                from_block,
                to_block,
                MINT_TOPIC,
                max_blocks_per_request,
            )
            .await?;
        let burns = self
            .get_logs_chunked(
                pool_address,
                from_block,
                to_block,
                BURN_TOPIC,
                max_blocks_per_request,
            )
            .await?;
        let collects = self
            .get_logs_chunked(
                pool_address,
                from_block,
                to_block,
                COLLECT_TOPIC,
                max_blocks_per_request,
            )
            .await?;
        let latest_swap = swaps
            .iter()
            .filter_map(|log| {
                let block = log
                    .block_number
                    .as_deref()
                    .and_then(|value| parse_hex_u64(value).ok())?;
                let tick = decode_swap_tick(&log.data).ok()?;
                Some((block, tick))
            })
            .max_by_key(|(block, _)| *block);

        Ok(PoolEventSummary {
            pool_address: pool_address.to_string(),
            from_block,
            to_block,
            swap_count: swaps.len(),
            mint_count: mints.len(),
            burn_count: burns.len(),
            collect_count: collects.len(),
            latest_swap_block: latest_swap.map(|(block, _)| block),
            latest_swap_tick: latest_swap.map(|(_, tick)| tick),
        })
    }

    async fn call(&self, method: &str, params: Value) -> Result<String, EvmError> {
        let response = self.call_value(method, params).await?;

        response
            .as_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| EvmError::InvalidRpcResponse(response.to_string()))
    }

    async fn call_value(&self, method: &str, params: Value) -> Result<Value, EvmError> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        // Free RPC tiers rate-limit (HTTP 429) and occasionally 503. Retry with
        // exponential backoff so the indexers and scans coexist on one endpoint.
        const MAX_ATTEMPTS: u32 = 6;
        let mut last_err = EvmError::Provider("no attempts".to_string());
        for attempt in 0..MAX_ATTEMPTS {
            if attempt > 0 {
                let backoff_ms = 250u64 * (1u64 << (attempt - 1).min(5));
                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
            }

            let response = match self.http.post(&self.rpc_url).json(&body).send().await {
                Ok(response) => response,
                Err(err) => {
                    last_err = EvmError::Provider(err.to_string());
                    continue;
                }
            };
            let status = response.status();
            let body_text = match response.text().await {
                Ok(text) => text,
                Err(err) => {
                    last_err = EvmError::Provider(err.to_string());
                    continue;
                }
            };

            if status.as_u16() == 429 || status.is_server_error() {
                last_err = EvmError::Provider(format!("{status}: {body_text}"));
                continue;
            }
            if !status.is_success() {
                return Err(EvmError::Provider(format!("{status}: {body_text}")));
            }

            let response = serde_json::from_str::<Value>(&body_text)
                .map_err(|err| EvmError::InvalidRpcResponse(err.to_string()))?;
            if let Some(error) = response.get("error") {
                // -32005 / "capacity" style errors are rate limits; retry those too.
                let text = error.to_string();
                if text.contains("rate") || text.contains("capacity") || text.contains("-32005") {
                    last_err = EvmError::Rpc(text);
                    continue;
                }
                return Err(EvmError::Rpc(text));
            }

            return response
                .get("result")
                .cloned()
                .ok_or_else(|| EvmError::InvalidRpcResponse(response.to_string()));
        }

        Err(last_err)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvmPoolKind {
    UniswapV3,
    UniswapV4,
    AerodromeSlipstream,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TxAction {
    CollectFees {
        pool: PoolKey,
    },
    BurnLiquidity {
        pool: PoolKey,
        range: RangeSpec,
    },
    SwapExactInput {
        token_in: String,
        token_out: String,
        amount_in: String,
    },
    MintLiquidity {
        pool: PoolKey,
        range: RangeSpec,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxPlan {
    pub chain_id: u64,
    pub actions: Vec<TxAction>,
    pub max_gas_usd: Usd,
    pub max_slippage_bps: f64,
    pub requires_signature: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationResult {
    pub success: bool,
    pub gas_used: Option<u64>,
    pub gas_cost_usd: Option<Usd>,
    pub error: Option<String>,
}

#[async_trait]
pub trait EvmPoolReader {
    async fn read_pool_state(&self, pool: &PoolKey) -> Result<PoolMarketState, EvmError>;
    async fn read_positions(&self, owner: &str) -> Result<Vec<PositionState>, EvmError>;
}

#[async_trait]
pub trait EvmTransactionSimulator {
    async fn simulate(&self, plan: &TxPlan) -> Result<SimulationResult, EvmError>;
}

#[async_trait]
pub trait EvmExecutor {
    async fn submit(&self, plan: &TxPlan) -> Result<String, EvmError>;
}

fn parse_hex_u64(value: &str) -> Result<u64, EvmError> {
    u64::from_str_radix(value.trim_start_matches("0x"), 16)
        .map_err(|err| EvmError::InvalidRpcResponse(err.to_string()))
}

fn parse_hex_u128(value: &str) -> Result<u128, EvmError> {
    u128::from_str_radix(value.trim_start_matches("0x"), 16)
        .map_err(|err| EvmError::InvalidRpcResponse(err.to_string()))
}

fn encode_get_pool_call(
    token_a: &str,
    token_b: &str,
    tick_spacing: i32,
) -> Result<String, EvmError> {
    Ok(format!(
        "0x28af8d0b{}{}{}",
        encode_address(token_a)?,
        encode_address(token_b)?,
        encode_positive_int24(tick_spacing)?
    ))
}

fn encode_address(address: &str) -> Result<String, EvmError> {
    let stripped = address.trim_start_matches("0x");
    if stripped.len() != 40 || !stripped.chars().all(|value| value.is_ascii_hexdigit()) {
        return Err(EvmError::InvalidRpcResponse(format!(
            "invalid address {address}"
        )));
    }
    Ok(format!("{stripped:0>64}").to_ascii_lowercase())
}

fn encode_positive_int24(value: i32) -> Result<String, EvmError> {
    if !(0..=0x7f_ffff).contains(&value) {
        return Err(EvmError::InvalidRpcResponse(format!(
            "tick spacing is not a positive int24: {value}"
        )));
    }
    Ok(format!("{:064x}", value as u32))
}

fn decode_address_result(result: &str) -> Option<String> {
    let word = normalize_hex_word(result).ok()?;
    let address = &word[24..64];
    if address.chars().all(|character| character == '0') {
        None
    } else {
        Some(format!("0x{address}"))
    }
}

fn decode_i24_result(result: &str) -> Result<i32, EvmError> {
    let word = normalize_hex_word(result)?;
    decode_i24_word(&word)
}

fn decode_i24_word(word: &str) -> Result<i32, EvmError> {
    let lower = &word[word.len() - 6..];
    let raw = i32::from_str_radix(lower, 16)
        .map_err(|err| EvmError::InvalidRpcResponse(err.to_string()))?;
    Ok(if raw & 0x80_0000 != 0 {
        raw - 0x100_0000
    } else {
        raw
    })
}

fn decode_words(result: &str) -> Result<Vec<String>, EvmError> {
    let stripped = result.trim_start_matches("0x");
    if stripped.len() % 64 != 0 {
        return Err(EvmError::InvalidRpcResponse(format!(
            "ABI result length is not word-aligned: {result}"
        )));
    }
    Ok(stripped
        .as_bytes()
        .chunks(64)
        .map(|chunk| String::from_utf8_lossy(chunk).to_string())
        .collect())
}

fn normalize_hex_word(result: &str) -> Result<String, EvmError> {
    let stripped = result.trim_start_matches("0x");
    if stripped.len() > 64 || !stripped.chars().all(|value| value.is_ascii_hexdigit()) {
        return Err(EvmError::InvalidRpcResponse(format!(
            "invalid ABI word: {result}"
        )));
    }
    Ok(format!("{stripped:0>64}").to_ascii_lowercase())
}

fn decode_swap_tick(data: &str) -> Result<i32, EvmError> {
    let words = decode_words(data)?;
    decode_i24_word(
        words
            .get(4)
            .ok_or_else(|| EvmError::InvalidRpcResponse("swap data missing tick".to_string()))?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rpc_hex_values() {
        assert_eq!(parse_hex_u64("0x10").unwrap(), 16);
        assert_eq!(parse_hex_u128("0x3b9aca00").unwrap(), 1_000_000_000);
    }

    #[test]
    fn encodes_get_pool_call() {
        let call = encode_get_pool_call(
            "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "0xfde4C96c8593536E31F229EA8f37b2ADa2699bb2",
            1,
        )
        .unwrap();
        assert!(call.starts_with("0x28af8d0b"));
        assert_eq!(call.len(), 202);
    }

    #[test]
    fn decodes_abi_words() {
        assert_eq!(decode_i24_result("0x15").unwrap(), 21);
        assert_eq!(
            decode_i24_word("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")
                .unwrap(),
            -1
        );
        assert_eq!(
            decode_words("0x0000000000000000000000000000000000000000000000000000000000000001")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn decodes_swap_tick_from_data() {
        let data = concat!(
            "0x",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
        );
        assert_eq!(decode_swap_tick(data).unwrap(), -1);
    }
}
