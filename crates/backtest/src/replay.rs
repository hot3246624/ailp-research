//! Concentrated-liquidity replay engine.
//!
//! Turns a stream of on-chain `Swap` observations into LP profit-and-loss for a
//! set of candidate range policies. The math follows Uniswap v3 / Aerodrome
//! Slipstream concentrated-liquidity mechanics:
//!
//! - a position holds constant liquidity `L` inside a tick range `[lower, upper]`;
//! - token amounts at any price are a closed-form function of `L`, the range, and
//!   the current `sqrt(price)`;
//! - while the price is inside the range the position earns a share of every
//!   swap's fee proportional to `L / (L_active + L)`.
//!
//! Everything is computed in token0 ("numeraire") units internally and only
//! converted to USD at report time via a single `token0_usd` anchor. This avoids
//! needing an independent USD feed for the volatile second asset: its value is
//! marked at the pool's own price, which is exactly where adverse inventory drift
//! shows up.

use serde::{Deserialize, Serialize};

/// Raw, decoded fields of a single Slipstream/UniswapV3 `Swap` event.
///
/// Amounts and liquidity are kept in raw integer units (as `f64`) because the
/// fee model only uses ratios and the final USD conversion divides by the token
/// decimal scale.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SwapObs {
    pub block: u64,
    pub log_index: u64,
    /// Signed token0 delta of the pool (raw units). Positive = token0 into pool.
    pub amount0: f64,
    /// Signed token1 delta of the pool (raw units). Positive = token1 into pool.
    pub amount1: f64,
    /// Pool `sqrtPriceX96` after the swap.
    pub sqrt_price_x96: f64,
    /// In-range pool liquidity reported by the swap (raw units).
    pub liquidity: f64,
    /// Pool tick after the swap.
    pub tick: i32,
}

impl SwapObs {
    /// Post-swap raw price (token1 per token0, raw integer units).
    pub fn price_raw(&self) -> f64 {
        let s = self.sqrt_price_x96 / TWO_POW_96;
        s * s
    }

    /// Gross input amount (raw) and whether the input token was token0.
    fn gross_input(&self) -> (f64, bool) {
        if self.amount0 > 0.0 {
            (self.amount0, true)
        } else {
            (self.amount1.max(0.0), false)
        }
    }
}

const TWO_POW_96: f64 = 79_228_162_514_264_337_593_543_950_336.0; // 2^96

/// Decode the 5 ABI words of a Slipstream/UniswapV3 `Swap` `data` blob into a
/// [`SwapObs`]. Returns `None` if the blob is malformed.
///
/// Layout: `amount0 (int256), amount1 (int256), sqrtPriceX96 (uint160),
/// liquidity (uint128), tick (int24)`.
pub fn decode_swap_obs(data: &str, block: u64, log_index: u64) -> Option<SwapObs> {
    let words = abi_words(data)?;
    if words.len() < 5 {
        return None;
    }
    Some(SwapObs {
        block,
        log_index,
        amount0: word_to_f64_signed(words[0]),
        amount1: word_to_f64_signed(words[1]),
        sqrt_price_x96: word_to_f64_unsigned(words[2]),
        liquidity: word_to_f64_unsigned(words[3]),
        tick: decode_i24_word(words[4]),
    })
}

fn abi_words(data: &str) -> Option<Vec<&str>> {
    let stripped = data.strip_prefix("0x").unwrap_or(data);
    if stripped.is_empty() || stripped.len() % 64 != 0 {
        return None;
    }
    Some(
        (0..stripped.len())
            .step_by(64)
            .map(|start| &stripped[start..start + 64])
            .collect(),
    )
}

fn word_to_f64_unsigned(word: &str) -> f64 {
    let mut acc = 0.0_f64;
    for ch in word.chars() {
        let digit = ch.to_digit(16).unwrap_or(0) as f64;
        acc = acc * 16.0 + digit;
    }
    acc
}

fn word_to_f64_signed(word: &str) -> f64 {
    // Two's-complement: top bit set means negative.
    let first = word.as_bytes().first().copied().unwrap_or(b'0');
    let negative = (first as char).to_digit(16).unwrap_or(0) >= 8;
    if !negative {
        return word_to_f64_unsigned(word);
    }
    // Compute the (small) magnitude directly at nibble level: ~word + 1. This
    // keeps precision; subtracting two ~2^256 floats would not.
    let mut magnitude = 0.0_f64;
    for ch in word.chars() {
        let digit = ch.to_digit(16).unwrap_or(0);
        magnitude = magnitude * 16.0 + (15 - digit) as f64;
    }
    -(magnitude + 1.0)
}

fn decode_i24_word(word: &str) -> i32 {
    let lower = &word[word.len().saturating_sub(6)..];
    let raw = i32::from_str_radix(lower, 16).unwrap_or(0);
    if raw & 0x80_0000 != 0 {
        raw - 0x100_0000
    } else {
        raw
    }
}

/// Raw `sqrt(1.0001^tick)` for a tick boundary.
fn sqrt_ratio_at_tick(tick: i32) -> f64 {
    1.0001_f64.powf(tick as f64 / 2.0)
}

/// Static economic context for a replay: token decimals, pool fee, and the USD
/// anchor for token0.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ReplayConfig {
    pub decimals0: u8,
    pub decimals1: u8,
    /// Pool fee as a fraction (e.g. 0.003 for 30 bps).
    pub fee_fraction: f64,
    /// USD value of one whole token0 (the numeraire anchor).
    pub token0_usd: f64,
    /// Capital deployed into the position, in USD.
    pub capital_usd: f64,
    /// Gas cost of one rebalance (collect+burn+swap+mint), in USD.
    pub rebalance_gas_usd: f64,
    /// Slippage charged on the fraction of capital re-swapped at each rebalance.
    pub rebalance_slippage_bps: f64,
    /// Fraction of position value assumed to be swapped to re-center inventory.
    pub rebalance_swap_fraction: f64,
}

impl ReplayConfig {
    fn scale0(&self) -> f64 {
        10f64.powi(self.decimals0 as i32)
    }
    fn scale1(&self) -> f64 {
        10f64.powi(self.decimals1 as i32)
    }
    /// Convert a raw price (token1/token0) into a human price.
    fn price_human(&self, price_raw: f64) -> f64 {
        price_raw * 10f64.powi(self.decimals0 as i32 - self.decimals1 as i32)
    }
    /// Value, in human token0 units, of raw token amounts marked at `price_raw`.
    fn value_token0(&self, amount0_raw: f64, amount1_raw: f64, price_raw: f64) -> f64 {
        let p_h = self.price_human(price_raw);
        let a0 = amount0_raw / self.scale0();
        let a1 = amount1_raw / self.scale1();
        a0 + if p_h > 0.0 { a1 / p_h } else { 0.0 }
    }
    fn token0_to_usd(&self, value_token0: f64) -> f64 {
        value_token0 * self.token0_usd
    }
    fn capital_token0(&self) -> f64 {
        self.capital_usd / self.token0_usd
    }
}

/// How a policy chooses (and maintains) its range.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum RangeMode {
    /// Hold the two tokens 50/50, never provide liquidity. Pure inventory baseline.
    HoldInventory,
    /// Set a wide range once at entry around the starting tick; never rebalance.
    StaticWide { half_width_ticks: i32 },
    /// Fixed half-width centered at the current tick; recenter when price exits.
    CenteredRebalance {
        half_width_ticks: i32,
        rebalance: bool,
    },
    /// Half-width scaled by realized tick volatility; recenter when price exits.
    VolatilityScaled {
        k: f64,
        floor_ticks: i32,
        cap_ticks: i32,
        window: usize,
    },
}

/// A concrete position: liquidity, range, entry price, and entry inventory.
#[derive(Debug, Clone, Copy)]
struct Position {
    liquidity: f64,
    sqrt_lower: f64,
    sqrt_upper: f64,
    amount0_entry: f64,
    amount1_entry: f64,
}

impl Position {
    /// sized to `capital_token0` human units of token0.
    fn open(cfg: &ReplayConfig, sqrt_price_raw: f64, lower_tick: i32, upper_tick: i32) -> Self {
        let sa = sqrt_ratio_at_tick(lower_tick);
        let sb = sqrt_ratio_at_tick(upper_tick);
        let sc = sqrt_price_raw.clamp(sa, sb);
        // Token amounts per unit of liquidity (raw integer convention).
        let a0_unit = (1.0 / sc - 1.0 / sb).max(0.0);
        let a1_unit = (sc - sa).max(0.0);
        let price_raw = sqrt_price_raw * sqrt_price_raw;
        let unit_value_token0 = cfg.value_token0(a0_unit, a1_unit, price_raw);
        let liquidity = if unit_value_token0 > 0.0 {
            cfg.capital_token0() / unit_value_token0
        } else {
            0.0
        };
        Self {
            liquidity,
            sqrt_lower: sa,
            sqrt_upper: sb,
            amount0_entry: liquidity * a0_unit,
            amount1_entry: liquidity * a1_unit,
        }
    }

    fn amounts_at(&self, sqrt_price_raw: f64) -> (f64, f64) {
        let sc = sqrt_price_raw.clamp(self.sqrt_lower, self.sqrt_upper);
        let a0 = self.liquidity * (1.0 / sc - 1.0 / self.sqrt_upper);
        let a1 = self.liquidity * (sc - self.sqrt_lower);
        (a0.max(0.0), a1.max(0.0))
    }

    fn in_range(&self, sqrt_price_raw: f64) -> bool {
        sqrt_price_raw >= self.sqrt_lower && sqrt_price_raw <= self.sqrt_upper
    }

    fn value_token0(&self, cfg: &ReplayConfig, sqrt_price_raw: f64) -> f64 {
        let (a0, a1) = self.amounts_at(sqrt_price_raw);
        cfg.value_token0(a0, a1, sqrt_price_raw * sqrt_price_raw)
    }
}

/// Per-policy replay outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyReport {
    pub policy: String,
    pub capital_usd: f64,
    pub final_value_usd: f64,
    pub fee_income_usd: f64,
    pub gas_cost_usd: f64,
    pub slippage_cost_usd: f64,
    /// Loss versus holding the position's entry inventory (excludes fees/costs).
    pub inventory_il_usd: f64,
    pub net_pnl_usd: f64,
    /// Net PnL minus the hold-50/50 baseline net PnL.
    pub net_vs_hold_usd: f64,
    pub rebalances: u32,
    pub swaps_in_range: u64,
    pub swaps_total: u64,
    pub time_in_range_pct: f64,
    pub final_tick: i32,
    pub avg_half_width_ticks: f64,
}

/// Simulate one range policy over the swap stream.
fn run_policy(swaps: &[SwapObs], cfg: &ReplayConfig, mode: RangeMode, name: &str) -> PolicyReport {
    let first = swaps.first().expect("non-empty swaps");
    let entry_sqrt = first.sqrt_price_x96 / TWO_POW_96;

    // Hold baseline: split capital 50/50 at entry and mark at the final price.
    if let RangeMode::HoldInventory = mode {
        return hold_report(swaps, cfg, entry_sqrt, name);
    }

    let mut half_width_sum = 0.0_f64;
    let mut half_width_count = 0u64;
    let mut tick_window: Vec<i32> = Vec::new();

    let initial_half = initial_half_width(&mode, &tick_window);
    half_width_sum += initial_half as f64;
    half_width_count += 1;
    let (mut lower, mut upper) = centered_range(first.tick, initial_half);
    let mut position = Position::open(cfg, entry_sqrt, lower, upper);

    let mut fees_token0 = 0.0_f64;
    let mut gas_usd = 0.0_f64;
    let mut slippage_usd = 0.0_f64;
    let mut rebalances = 0u32;
    let mut swaps_in_range = 0u64;

    // Price seen entering each swap = previous swap's post-price (entry for #0).
    let mut pre_sqrt = entry_sqrt;

    for swap in swaps {
        // Fee accrual uses the price at which the position was sitting when the
        // swap began (pre-swap price), which is where its liquidity was active.
        if position.in_range(pre_sqrt) {
            swaps_in_range += 1;
            let (gross_in, input_is_token0) = swap.gross_input();
            if gross_in > 0.0 && swap.liquidity > 0.0 {
                let share = position.liquidity / (swap.liquidity + position.liquidity);
                let fee_in_input_raw = share * cfg.fee_fraction * gross_in;
                let price_raw = swap.price_raw();
                fees_token0 += if input_is_token0 {
                    fee_in_input_raw / cfg.scale0()
                } else {
                    let p_h = cfg.price_human(price_raw);
                    if p_h > 0.0 {
                        (fee_in_input_raw / cfg.scale1()) / p_h
                    } else {
                        0.0
                    }
                };
            }
        }

        let post_sqrt = swap.sqrt_price_x96 / TWO_POW_96;
        tick_window.push(swap.tick);

        // Rebalance if the policy allows it and price left the range.
        if should_rebalance(&mode) && !position.in_range(post_sqrt) {
            // Realize: value continues in the position; pay costs and recenter.
            let pos_value_token0 = position.value_token0(cfg, post_sqrt);
            gas_usd += cfg.rebalance_gas_usd;
            let swap_notional_usd =
                cfg.token0_to_usd(pos_value_token0) * cfg.rebalance_swap_fraction;
            slippage_usd += swap_notional_usd * cfg.rebalance_slippage_bps / 10_000.0;
            rebalances += 1;

            let half = next_half_width(&mode, &tick_window);
            half_width_sum += half as f64;
            half_width_count += 1;
            let (nl, nu) = centered_range(swap.tick, half);
            lower = nl;
            upper = nu;
            // Re-open sized to the realized value net of this rebalance's costs.
            let net_value_token0 = (pos_value_token0
                - (cfg.rebalance_gas_usd
                    + swap_notional_usd * cfg.rebalance_slippage_bps / 10_000.0)
                    / cfg.token0_usd)
                .max(0.0);
            position = Position::open_with_capital(cfg, post_sqrt, lower, upper, net_value_token0);
        }

        pre_sqrt = post_sqrt;
    }

    let _ = (lower, upper);
    let final_sqrt = swaps.last().unwrap().sqrt_price_x96 / TWO_POW_96;
    let final_tick = swaps.last().unwrap().tick;

    let position_value_token0 = position.value_token0(cfg, final_sqrt);
    let final_value_usd = cfg.token0_to_usd(position_value_token0 + fees_token0);
    let fee_income_usd = cfg.token0_to_usd(fees_token0);

    // Inventory IL: hold the *current segment's* entry mix vs the LP amounts now.
    let hold_entry_value_token0 = cfg.value_token0(
        position.amount0_entry,
        position.amount1_entry,
        final_sqrt * final_sqrt,
    );
    let inventory_il_usd = cfg.token0_to_usd(hold_entry_value_token0 - position_value_token0);

    let net_pnl_usd = final_value_usd - cfg.capital_usd - gas_usd - slippage_usd;

    let hold_net = hold_net_pnl(swaps, cfg, entry_sqrt);
    let swaps_total = swaps.len() as u64;

    PolicyReport {
        policy: name.to_string(),
        capital_usd: cfg.capital_usd,
        final_value_usd,
        fee_income_usd,
        gas_cost_usd: gas_usd,
        slippage_cost_usd: slippage_usd,
        inventory_il_usd,
        net_pnl_usd,
        net_vs_hold_usd: net_pnl_usd - hold_net,
        rebalances,
        swaps_in_range,
        swaps_total,
        time_in_range_pct: if swaps_total > 0 {
            100.0 * swaps_in_range as f64 / swaps_total as f64
        } else {
            0.0
        },
        final_tick,
        avg_half_width_ticks: if half_width_count > 0 {
            half_width_sum / half_width_count as f64
        } else {
            0.0
        },
    }
}

impl Position {
    fn open_with_capital(
        cfg: &ReplayConfig,
        sqrt_price_raw: f64,
        lower_tick: i32,
        upper_tick: i32,
        capital_token0: f64,
    ) -> Self {
        let sa = sqrt_ratio_at_tick(lower_tick);
        let sb = sqrt_ratio_at_tick(upper_tick);
        let sc = sqrt_price_raw.clamp(sa, sb);
        let a0_unit = (1.0 / sc - 1.0 / sb).max(0.0);
        let a1_unit = (sc - sa).max(0.0);
        let price_raw = sqrt_price_raw * sqrt_price_raw;
        let unit_value_token0 = cfg.value_token0(a0_unit, a1_unit, price_raw);
        let liquidity = if unit_value_token0 > 0.0 {
            capital_token0 / unit_value_token0
        } else {
            0.0
        };
        Self {
            liquidity,
            sqrt_lower: sa,
            sqrt_upper: sb,
            amount0_entry: liquidity * a0_unit,
            amount1_entry: liquidity * a1_unit,
        }
    }
}

fn centered_range(center_tick: i32, half_width: i32) -> (i32, i32) {
    let half = half_width.max(1);
    (center_tick - half, center_tick + half)
}

fn initial_half_width(mode: &RangeMode, window: &[i32]) -> i32 {
    match mode {
        RangeMode::HoldInventory => 0,
        RangeMode::StaticWide { half_width_ticks } => *half_width_ticks,
        RangeMode::CenteredRebalance {
            half_width_ticks, ..
        } => *half_width_ticks,
        RangeMode::VolatilityScaled { floor_ticks, .. } => *floor_ticks.max(&1),
        #[allow(unreachable_patterns)]
        _ => {
            let _ = window;
            1
        }
    }
}

fn next_half_width(mode: &RangeMode, window: &[i32]) -> i32 {
    match mode {
        RangeMode::VolatilityScaled {
            k,
            floor_ticks,
            cap_ticks,
            window: w,
        } => {
            let vol = rolling_tick_vol(window, *w);
            ((k * vol).round() as i32).clamp(*floor_ticks, *cap_ticks)
        }
        RangeMode::CenteredRebalance {
            half_width_ticks, ..
        } => *half_width_ticks,
        RangeMode::StaticWide { half_width_ticks } => *half_width_ticks,
        RangeMode::HoldInventory => 0,
    }
}

fn should_rebalance(mode: &RangeMode) -> bool {
    matches!(
        mode,
        RangeMode::CenteredRebalance {
            rebalance: true,
            ..
        } | RangeMode::VolatilityScaled { .. }
    )
}

/// Standard deviation of consecutive tick changes over the last `window` ticks.
fn rolling_tick_vol(window: &[i32], n: usize) -> f64 {
    let n = n.max(2);
    let slice = if window.len() > n {
        &window[window.len() - n..]
    } else {
        window
    };
    if slice.len() < 2 {
        return 0.0;
    }
    let diffs: Vec<f64> = slice
        .windows(2)
        .map(|pair| (pair[1] - pair[0]) as f64)
        .collect();
    let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
    let var = diffs.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / diffs.len() as f64;
    var.sqrt()
}

fn hold_report(swaps: &[SwapObs], cfg: &ReplayConfig, entry_sqrt: f64, name: &str) -> PolicyReport {
    let final_sqrt = swaps.last().unwrap().sqrt_price_x96 / TWO_POW_96;
    let final_tick = swaps.last().unwrap().tick;
    let (a0, a1) = hold_5050_amounts(cfg, entry_sqrt);
    let final_value_usd = cfg.token0_to_usd(cfg.value_token0(a0, a1, final_sqrt * final_sqrt));
    let net_pnl_usd = final_value_usd - cfg.capital_usd;
    PolicyReport {
        policy: name.to_string(),
        capital_usd: cfg.capital_usd,
        final_value_usd,
        fee_income_usd: 0.0,
        gas_cost_usd: 0.0,
        slippage_cost_usd: 0.0,
        inventory_il_usd: 0.0,
        net_pnl_usd,
        net_vs_hold_usd: 0.0,
        rebalances: 0,
        swaps_in_range: 0,
        swaps_total: swaps.len() as u64,
        time_in_range_pct: 0.0,
        final_tick,
        avg_half_width_ticks: 0.0,
    }
}

/// Raw token amounts for a 50/50-by-value split of capital at entry price.
fn hold_5050_amounts(cfg: &ReplayConfig, entry_sqrt: f64) -> (f64, f64) {
    let price_raw = entry_sqrt * entry_sqrt;
    let p_h = cfg.price_human(price_raw);
    let half_usd = cfg.capital_usd / 2.0;
    // token0 leg: half the USD in token0.
    let a0_human = (half_usd / cfg.token0_usd).max(0.0);
    // token1 leg: half the USD worth of token1; token1_usd = token0_usd / p_h.
    let token1_usd = if p_h > 0.0 { cfg.token0_usd / p_h } else { 0.0 };
    let a1_human = if token1_usd > 0.0 {
        half_usd / token1_usd
    } else {
        0.0
    };
    (a0_human * cfg.scale0(), a1_human * cfg.scale1())
}

fn hold_net_pnl(swaps: &[SwapObs], cfg: &ReplayConfig, entry_sqrt: f64) -> f64 {
    let final_sqrt = swaps.last().unwrap().sqrt_price_x96 / TWO_POW_96;
    let (a0, a1) = hold_5050_amounts(cfg, entry_sqrt);
    cfg.token0_to_usd(cfg.value_token0(a0, a1, final_sqrt * final_sqrt)) - cfg.capital_usd
}

/// Run the standard baseline battery over a swap stream.
pub fn run_baseline_battery(swaps: &[SwapObs], cfg: &ReplayConfig) -> Vec<PolicyReport> {
    run_baseline_battery_with(swaps, cfg, 600, 6000, 1.5)
}

/// Run baselines with explicit narrow/wide half-widths and a vol multiplier.
pub fn run_baseline_battery_with(
    swaps: &[SwapObs],
    cfg: &ReplayConfig,
    narrow_half_width: i32,
    wide_half_width: i32,
    vol_k: f64,
) -> Vec<PolicyReport> {
    if swaps.is_empty() {
        return Vec::new();
    }
    vec![
        run_policy(swaps, cfg, RangeMode::HoldInventory, "hold_50_50"),
        run_policy(
            swaps,
            cfg,
            RangeMode::StaticWide {
                half_width_ticks: wide_half_width,
            },
            "passive_wide",
        ),
        run_policy(
            swaps,
            cfg,
            RangeMode::CenteredRebalance {
                half_width_ticks: narrow_half_width,
                rebalance: false,
            },
            "narrow_static",
        ),
        run_policy(
            swaps,
            cfg,
            RangeMode::CenteredRebalance {
                half_width_ticks: narrow_half_width,
                rebalance: true,
            },
            "narrow_rebalance",
        ),
        run_policy(
            swaps,
            cfg,
            RangeMode::VolatilityScaled {
                k: vol_k,
                floor_ticks: narrow_half_width / 2,
                cap_ticks: wide_half_width,
                window: 200,
            },
            "vol_scaled_rebalance",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ReplayConfig {
        ReplayConfig {
            decimals0: 18,
            decimals1: 18,
            fee_fraction: 0.003,
            token0_usd: 3300.0,
            capital_usd: 10_000.0,
            rebalance_gas_usd: 0.05,
            rebalance_slippage_bps: 5.0,
            rebalance_swap_fraction: 0.5,
        }
    }

    #[test]
    fn decodes_real_swap_word() {
        // From a real WETH-AERO swap: amount0 positive, amount1 negative, tick 80983.
        let data = concat!(
            "0x",
            "0000000000000000000000000000000000000000000000000c44307e19296880",
            "ffffffffffffffffffffffffffffffffffffffffffffff62c8758726b99a9885",
            "0000000000000000000000000000000000000039569350230507fbd1d71113b1",
            "000000000000000000000000000000000000000000039aeded48a3857e640b9b",
            "0000000000000000000000000000000000000000000000000000000000013c57"
        );
        let obs = decode_swap_obs(data, 47_815_885, 194).unwrap();
        assert_eq!(obs.tick, 80983);
        assert!(obs.amount0 > 0.0, "token0 into pool");
        assert!(obs.amount1 < 0.0, "token1 out of pool");
        // sqrt(1.0001^80983) ~= 57.3 -> price ~= 3286.
        let price = obs.price_raw();
        assert!(price > 3000.0 && price < 3600.0, "price was {price}");
    }

    #[test]
    fn entry_value_matches_capital() {
        let cfg = cfg();
        let entry_sqrt = sqrt_ratio_at_tick(80_000);
        let pos = Position::open(&cfg, entry_sqrt, 79_000, 81_000);
        let value_usd = cfg.token0_to_usd(pos.value_token0(&cfg, entry_sqrt));
        assert!((value_usd - cfg.capital_usd).abs() < 1.0, "got {value_usd}");
    }

    #[test]
    fn below_range_is_all_token0() {
        let cfg = cfg();
        let entry_sqrt = sqrt_ratio_at_tick(80_000);
        let pos = Position::open(&cfg, entry_sqrt, 79_000, 81_000);
        let below = sqrt_ratio_at_tick(78_000);
        let (a0, a1) = pos.amounts_at(below);
        assert!(a1 == 0.0, "all token0 below range, a1={a1}");
        assert!(a0 > 0.0);
    }

    #[test]
    fn synthetic_flat_price_earns_fees_no_il() {
        // Flat price: many swaps at the same tick. Narrow LP should earn fees and
        // have ~zero IL, and beat hold.
        let cfg = cfg();
        let sqrt = sqrt_ratio_at_tick(80_000);
        let x96 = sqrt * TWO_POW_96;
        let swaps: Vec<SwapObs> = (0..500)
            .map(|i| SwapObs {
                block: 1000 + i,
                log_index: 0,
                amount0: 1e18, // 1 token0 in each swap
                amount1: -3000e18,
                sqrt_price_x96: x96,
                liquidity: 1e24,
                tick: 80_000,
            })
            .collect();
        let reports = run_baseline_battery(&swaps, &cfg);
        let narrow = reports
            .iter()
            .find(|r| r.policy == "narrow_static")
            .unwrap();
        assert!(narrow.fee_income_usd > 0.0, "should earn fees");
        assert!(narrow.inventory_il_usd.abs() < 1.0, "flat price -> ~no IL");
        assert!(narrow.net_vs_hold_usd > 0.0, "fees should beat flat hold");
        assert_eq!(narrow.time_in_range_pct, 100.0);
    }

    #[test]
    fn rebalance_counts_when_price_trends() {
        let cfg = cfg();
        // Price ramps upward through ticks; narrow rebalancing must recenter.
        let swaps: Vec<SwapObs> = (0..400)
            .map(|i| {
                let tick = 80_000 + (i as i32) * 5;
                let sqrt = sqrt_ratio_at_tick(tick);
                SwapObs {
                    block: 1000 + i,
                    log_index: 0,
                    amount0: 1e18,
                    amount1: -3000e18,
                    sqrt_price_x96: sqrt * TWO_POW_96,
                    liquidity: 1e24,
                    tick,
                }
            })
            .collect();
        let reports = run_baseline_battery_with(&swaps, &cfg, 100, 6000, 1.5);
        let reb = reports
            .iter()
            .find(|r| r.policy == "narrow_rebalance")
            .unwrap();
        assert!(
            reb.rebalances > 0,
            "trending price should trigger rebalances"
        );
    }
}
