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

/// Execution-realism context: you cannot rebalance instantly, and a short hedge
/// pays funding. `risk_asset_is_token1` says which leg is the volatile/"risk"
/// asset (token1 = AERO for WETH-AERO); the danger side is the one that converts
/// the position into that asset.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ExecConfig {
    /// Blocks between a trigger firing and the action executing. In a crash the
    /// delayed price is materially worse — this captures "you can't react instantly".
    pub action_delay_blocks: u64,
    /// Seconds per block (Base ~2s), used to convert block spans into funding time.
    pub block_seconds: f64,
    /// Funding cost (bps/day) charged on the short-hedge notional. Negative = earn.
    pub funding_bps_per_day: f64,
    /// Whether token1 is the volatile/risk asset (true for WETH-AERO).
    pub risk_asset_is_token1: bool,
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            action_delay_blocks: 0,
            block_seconds: 2.0,
            funding_bps_per_day: 0.0,
            risk_asset_is_token1: true,
        }
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
    /// Narrow band, but one-way: recenter on the safe side, and on the danger side
    /// (accumulating the risk asset) exit fully to the money leg and stand aside
    /// until price retraces. Caps the left tail instead of chasing it.
    HardExitStop { half_width_ticks: i32 },
    /// Narrow rebalancing band plus a static short hedge on the risk asset, sized
    /// to `hedge_fraction` of the entry risk-asset exposure.
    HedgedRebalance {
        half_width_ticks: i32,
        hedge_fraction: f64,
    },
    /// Regime-aware policy. Reads recent tick history each swap and:
    /// - ranging (low trend strength): hold a vol-scaled band, recenter only on
    ///   exit (tight when calm, wider when choppy — avoids the churn death spiral);
    /// - trending into the risk asset: exit to the money leg preemptively and stand
    ///   aside until the trend dies. Synthesizes static / vol-scaled / hard-exit.
    Adaptive {
        vol_k: f64,
        floor_ticks: i32,
        cap_ticks: i32,
        /// Trend strength (|net tick move| / (vol·√window)) above which the regime
        /// is judged trending. ~2 ≈ a 2-sigma directional move.
        trend_exit_threshold: f64,
        window: usize,
    },
}

/// Rolling regime estimate over the last `n` ticks: per-swap drift, tick-change
/// volatility, and a t-stat-like trend strength `|displacement| / (vol·√n)`.
struct Regime {
    drift_per_swap: f64,
    trend_strength: f64,
}

fn assess_regime(window: &[i32], n: usize) -> Regime {
    let n = n.max(2);
    let slice = if window.len() > n {
        &window[window.len() - n..]
    } else {
        window
    };
    if slice.len() < 2 {
        return Regime {
            drift_per_swap: 0.0,
            trend_strength: 0.0,
        };
    }
    let vol = rolling_tick_vol(slice, slice.len());
    let displacement = (slice[slice.len() - 1] - slice[0]) as f64;
    let drift_per_swap = displacement / slice.len() as f64;
    let denom = vol * (slice.len() as f64).sqrt();
    let trend_strength = if denom > 1e-9 {
        displacement.abs() / denom
    } else if displacement.abs() > 0.0 {
        f64::INFINITY
    } else {
        0.0
    };
    Regime {
        drift_per_swap,
        trend_strength,
    }
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
    // --- tail / execution metrics ---
    /// Largest peak-to-trough drop in mark-to-market equity over the path.
    pub max_drawdown_usd: f64,
    /// Lowest equity reached over the path.
    pub min_equity_usd: f64,
    /// Longest contiguous block span spent out-of-range holding the risk asset.
    pub max_one_sided_risk_blocks: u64,
    /// Fees earned while the position was in the risk-accumulation half of its
    /// range — "toxic" fees that come bundled with adverse selection.
    pub toxic_fee_usd: f64,
    /// PnL of the short hedge (positive when the risk asset fell).
    pub hedge_pnl_usd: f64,
    /// Funding paid on the hedge notional.
    pub funding_cost_usd: f64,
}

/// Find the execution price `delay` blocks after a trigger at `from_idx` — the
/// price you actually get given you cannot act instantly. Returns (sqrt, tick).
fn price_after_delay(
    swaps: &[SwapObs],
    from_idx: usize,
    from_block: u64,
    delay: u64,
) -> (f64, i32) {
    if delay == 0 {
        let s = &swaps[from_idx];
        return (s.sqrt_price_x96 / TWO_POW_96, s.tick);
    }
    let target = from_block.saturating_add(delay);
    for s in &swaps[from_idx..] {
        if s.block >= target {
            return (s.sqrt_price_x96 / TWO_POW_96, s.tick);
        }
    }
    let last = swaps.last().unwrap();
    (last.sqrt_price_x96 / TWO_POW_96, last.tick)
}

/// AERO (risk asset, token1) price expressed in token0 units = 1 / human price.
fn risk_price_token0(cfg: &ReplayConfig, sqrt_raw: f64) -> f64 {
    let p_h = cfg.price_human(sqrt_raw * sqrt_raw);
    if p_h > 0.0 { 1.0 / p_h } else { 0.0 }
}

/// Simulate one range policy over the swap stream, tracking execution latency,
/// an optional short hedge, one-way exits, and tail metrics.
fn run_policy(
    swaps: &[SwapObs],
    cfg: &ReplayConfig,
    exec: &ExecConfig,
    mode: RangeMode,
    name: &str,
) -> PolicyReport {
    let first = swaps.first().expect("non-empty swaps");
    let entry_sqrt = first.sqrt_price_x96 / TWO_POW_96;
    let hold_net = hold_net_pnl(swaps, cfg, entry_sqrt);

    if let RangeMode::HoldInventory = mode {
        return hold_report(swaps, cfg, entry_sqrt, name, hold_net);
    }

    let rebalance = should_rebalance(&mode);
    let hard_exit = matches!(mode, RangeMode::HardExitStop { .. });
    let adaptive = matches!(mode, RangeMode::Adaptive { .. });
    let (adaptive_threshold, adaptive_window) = match mode {
        RangeMode::Adaptive {
            trend_exit_threshold,
            window,
            ..
        } => (trend_exit_threshold, window),
        _ => (f64::INFINITY, 200),
    };
    let hedge_fraction = match mode {
        RangeMode::HedgedRebalance { hedge_fraction, .. } => hedge_fraction,
        _ => 0.0,
    };
    let hedged = hedge_fraction > 0.0;

    let mut half_width_sum = 0.0_f64;
    let mut half_width_count = 0u64;
    let mut tick_window: Vec<i32> = Vec::new();

    let initial_half = initial_half_width(&mode, &tick_window);
    half_width_sum += initial_half as f64;
    half_width_count += 1;
    let (mut lower, mut upper) = centered_range(first.tick, initial_half);
    let mut position: Option<Position> = Some(Position::open(cfg, entry_sqrt, lower, upper));
    let mut sidelined_token0 = 0.0_f64; // value parked in the money leg (token0)

    // Static short hedge on the risk asset (token1), sized to entry exposure.
    let entry_risk_price = risk_price_token0(cfg, entry_sqrt);
    let short_risk_human = if hedged {
        hedge_fraction * (position.as_ref().unwrap().amount1_entry / cfg.scale1())
    } else {
        0.0
    };

    let mut fees_token0 = 0.0_f64;
    let mut toxic_fee_token0 = 0.0_f64;
    let mut gas_usd = 0.0_f64;
    let mut slippage_usd = 0.0_f64;
    let mut funding_usd = 0.0_f64;
    let mut rebalances = 0u32;
    let mut swaps_in_range = 0u64;

    let mut peak_equity = cfg.capital_usd;
    let mut max_drawdown = 0.0_f64;
    let mut min_equity = cfg.capital_usd;
    let mut risk_run_start: Option<u64> = None;
    let mut max_one_sided_risk_blocks = 0u64;

    let mut pre_sqrt = entry_sqrt;
    let mut prev_block = first.block;

    for (i, swap) in swaps.iter().enumerate() {
        // Fee accrual at the pre-swap price (where liquidity was active).
        if let Some(pos) = &position {
            if pos.in_range(pre_sqrt) {
                swaps_in_range += 1;
                let (gross_in, input_is_token0) = swap.gross_input();
                if gross_in > 0.0 && swap.liquidity > 0.0 {
                    let share = pos.liquidity / (swap.liquidity + pos.liquidity);
                    let fee_in_input_raw = share * cfg.fee_fraction * gross_in;
                    let price_raw = swap.price_raw();
                    let fee_t0 = if input_is_token0 {
                        fee_in_input_raw / cfg.scale0()
                    } else {
                        let p_h = cfg.price_human(price_raw);
                        if p_h > 0.0 {
                            (fee_in_input_raw / cfg.scale1()) / p_h
                        } else {
                            0.0
                        }
                    };
                    fees_token0 += fee_t0;
                    // Toxic if sitting in the risk-accumulation (upper) half.
                    let mid = (pos.sqrt_lower * pos.sqrt_upper).sqrt();
                    if pre_sqrt > mid {
                        toxic_fee_token0 += fee_t0;
                    }
                }
            }
        }

        let post_sqrt = swap.sqrt_price_x96 / TWO_POW_96;
        tick_window.push(swap.tick);

        // Funding accrual on the short hedge notional.
        if hedged {
            let dt_days =
                swap.block.saturating_sub(prev_block) as f64 * exec.block_seconds / 86_400.0;
            let short_notional_usd =
                cfg.token0_to_usd(short_risk_human * risk_price_token0(cfg, post_sqrt));
            funding_usd += exec.funding_bps_per_day / 10_000.0 * short_notional_usd * dt_days;
        }

        // Regime-aware adaptive policy: act on a live trend read, even in range.
        if adaptive {
            let regime = assess_regime(&tick_window, adaptive_window);
            let trending = regime.trend_strength >= adaptive_threshold;
            // Risk asset is token1: the danger trend is the tick rising (drift > 0).
            let trending_danger = trending && regime.drift_per_swap > 0.0;
            // Benign trend = price drifting to the money side; safe to follow.
            let trending_benign = trending && regime.drift_per_swap < 0.0;
            if position.is_some() {
                let in_range = position.as_ref().unwrap().in_range(post_sqrt);
                if trending_danger {
                    // Preemptive exit to the money leg at the delayed price.
                    let (exec_sqrt, _) =
                        price_after_delay(swaps, i, swap.block, exec.action_delay_blocks);
                    let val = position.as_ref().unwrap().value_token0(cfg, exec_sqrt);
                    let notional_usd = cfg.token0_to_usd(val);
                    let slip = notional_usd * cfg.rebalance_slippage_bps / 10_000.0;
                    gas_usd += cfg.rebalance_gas_usd;
                    slippage_usd += slip;
                    rebalances += 1;
                    sidelined_token0 =
                        (val - (cfg.rebalance_gas_usd + slip) / cfg.token0_usd).max(0.0);
                    position = None;
                } else if trending_benign && !in_range {
                    // Follow a benign trend only; in a non-trending (chop) regime we
                    // HOLD and let the band mean-revert instead of chasing wiggles.
                    let (exec_sqrt, exec_tick) =
                        price_after_delay(swaps, i, swap.block, exec.action_delay_blocks);
                    let val = position.as_ref().unwrap().value_token0(cfg, exec_sqrt);
                    let notional_usd = cfg.token0_to_usd(val) * cfg.rebalance_swap_fraction;
                    let slip = notional_usd * cfg.rebalance_slippage_bps / 10_000.0;
                    gas_usd += cfg.rebalance_gas_usd;
                    slippage_usd += slip;
                    rebalances += 1;
                    let half = next_half_width(&mode, &tick_window);
                    half_width_sum += half as f64;
                    half_width_count += 1;
                    let (nl, nu) = centered_range(exec_tick, half);
                    lower = nl;
                    upper = nu;
                    let net = (val - (cfg.rebalance_gas_usd + slip) / cfg.token0_usd).max(0.0);
                    position = Some(Position::open_with_capital(
                        cfg, exec_sqrt, lower, upper, net,
                    ));
                }
            } else if !trending_danger {
                // Sidelined: re-enter once the danger trend dies.
                let half = next_half_width(&mode, &tick_window);
                half_width_sum += half as f64;
                half_width_count += 1;
                let (nl, nu) = centered_range(swap.tick, half);
                lower = nl;
                upper = nu;
                gas_usd += cfg.rebalance_gas_usd;
                rebalances += 1;
                position = Some(Position::open_with_capital(
                    cfg,
                    post_sqrt,
                    lower,
                    upper,
                    sidelined_token0,
                ));
                sidelined_token0 = 0.0;
            }
        } else if position.is_some() {
            // Position actions on a range breach.
            let (in_range, danger) = {
                let pos = position.as_ref().unwrap();
                (pos.in_range(post_sqrt), post_sqrt > pos.sqrt_upper)
            };
            if !in_range && hard_exit && danger {
                // One-way exit: liquidate to the money leg at the delayed price.
                let (exec_sqrt, _) =
                    price_after_delay(swaps, i, swap.block, exec.action_delay_blocks);
                let val = position.as_ref().unwrap().value_token0(cfg, exec_sqrt);
                let notional_usd = cfg.token0_to_usd(val);
                let slip = notional_usd * cfg.rebalance_slippage_bps / 10_000.0;
                gas_usd += cfg.rebalance_gas_usd;
                slippage_usd += slip;
                sidelined_token0 = (val - (cfg.rebalance_gas_usd + slip) / cfg.token0_usd).max(0.0);
                position = None;
            } else if !in_range && rebalance {
                let (exec_sqrt, exec_tick) =
                    price_after_delay(swaps, i, swap.block, exec.action_delay_blocks);
                let val = position.as_ref().unwrap().value_token0(cfg, exec_sqrt);
                let notional_usd = cfg.token0_to_usd(val) * cfg.rebalance_swap_fraction;
                let slip = notional_usd * cfg.rebalance_slippage_bps / 10_000.0;
                gas_usd += cfg.rebalance_gas_usd;
                slippage_usd += slip;
                rebalances += 1;
                let half = next_half_width(&mode, &tick_window);
                half_width_sum += half as f64;
                half_width_count += 1;
                let (nl, nu) = centered_range(exec_tick, half);
                lower = nl;
                upper = nu;
                let net = (val - (cfg.rebalance_gas_usd + slip) / cfg.token0_usd).max(0.0);
                position = Some(Position::open_with_capital(
                    cfg, exec_sqrt, lower, upper, net,
                ));
            }
        } else {
            // Sidelined: re-enter once price retraces to the old range center.
            let center_old = (lower + upper) / 2;
            if swap.tick <= center_old {
                let half = next_half_width(&mode, &tick_window);
                half_width_sum += half as f64;
                half_width_count += 1;
                let (nl, nu) = centered_range(swap.tick, half);
                lower = nl;
                upper = nu;
                gas_usd += cfg.rebalance_gas_usd;
                rebalances += 1;
                position = Some(Position::open_with_capital(
                    cfg,
                    post_sqrt,
                    lower,
                    upper,
                    sidelined_token0,
                ));
                sidelined_token0 = 0.0;
            }
        }

        // One-sided risk hold: active and above range = stuck holding the risk asset.
        let holding_risk = position
            .as_ref()
            .map(|pos| post_sqrt > pos.sqrt_upper)
            .unwrap_or(false);
        if holding_risk {
            let start = *risk_run_start.get_or_insert(swap.block);
            max_one_sided_risk_blocks =
                max_one_sided_risk_blocks.max(swap.block.saturating_sub(start));
        } else {
            risk_run_start = None;
        }

        // Mark-to-market equity for drawdown.
        let pos_val_t0 = match &position {
            Some(pos) => pos.value_token0(cfg, post_sqrt),
            None => sidelined_token0,
        };
        let hedge_pnl_t0 =
            short_risk_human * (entry_risk_price - risk_price_token0(cfg, post_sqrt));
        let equity_usd = cfg.token0_to_usd(pos_val_t0 + fees_token0 + hedge_pnl_t0)
            - gas_usd
            - slippage_usd
            - funding_usd;
        peak_equity = peak_equity.max(equity_usd);
        max_drawdown = max_drawdown.max(peak_equity - equity_usd);
        min_equity = min_equity.min(equity_usd);

        pre_sqrt = post_sqrt;
        prev_block = swap.block;
    }

    let final_sqrt = swaps.last().unwrap().sqrt_price_x96 / TWO_POW_96;
    let final_tick = swaps.last().unwrap().tick;

    let pos_val_t0 = match &position {
        Some(pos) => pos.value_token0(cfg, final_sqrt),
        None => sidelined_token0,
    };
    let hedge_pnl_t0 = short_risk_human * (entry_risk_price - risk_price_token0(cfg, final_sqrt));
    let hedge_pnl_usd = cfg.token0_to_usd(hedge_pnl_t0);
    let fee_income_usd = cfg.token0_to_usd(fees_token0);
    let final_value_usd = cfg.token0_to_usd(pos_val_t0 + fees_token0 + hedge_pnl_t0);

    let inventory_il_usd = match &position {
        Some(pos) => {
            let hold_v = cfg.value_token0(
                pos.amount0_entry,
                pos.amount1_entry,
                final_sqrt * final_sqrt,
            );
            cfg.token0_to_usd(hold_v - pos.value_token0(cfg, final_sqrt))
        }
        None => 0.0,
    };

    let net_pnl_usd = final_value_usd - cfg.capital_usd - gas_usd - slippage_usd - funding_usd;
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
        max_drawdown_usd: max_drawdown,
        min_equity_usd: min_equity,
        max_one_sided_risk_blocks,
        toxic_fee_usd: cfg.token0_to_usd(toxic_fee_token0),
        hedge_pnl_usd,
        funding_cost_usd: funding_usd,
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
    let _ = window;
    match mode {
        RangeMode::HoldInventory => 0,
        RangeMode::StaticWide { half_width_ticks } => *half_width_ticks,
        RangeMode::CenteredRebalance {
            half_width_ticks, ..
        } => *half_width_ticks,
        RangeMode::HardExitStop { half_width_ticks } => *half_width_ticks,
        RangeMode::HedgedRebalance {
            half_width_ticks, ..
        } => *half_width_ticks,
        RangeMode::VolatilityScaled { floor_ticks, .. } => *floor_ticks.max(&1),
        RangeMode::Adaptive { floor_ticks, .. } => *floor_ticks.max(&1),
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
        RangeMode::HardExitStop { half_width_ticks } => *half_width_ticks,
        RangeMode::HedgedRebalance {
            half_width_ticks, ..
        } => *half_width_ticks,
        RangeMode::StaticWide { half_width_ticks } => *half_width_ticks,
        RangeMode::Adaptive {
            vol_k,
            floor_ticks,
            cap_ticks,
            window: w,
            ..
        } => {
            let vol = rolling_tick_vol(window, *w);
            ((vol_k * vol).round() as i32).clamp(*floor_ticks, *cap_ticks)
        }
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
            | RangeMode::HardExitStop { .. }
            | RangeMode::HedgedRebalance { .. }
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

fn hold_report(
    swaps: &[SwapObs],
    cfg: &ReplayConfig,
    entry_sqrt: f64,
    name: &str,
    hold_net: f64,
) -> PolicyReport {
    let final_sqrt = swaps.last().unwrap().sqrt_price_x96 / TWO_POW_96;
    let final_tick = swaps.last().unwrap().tick;
    let (a0, a1) = hold_5050_amounts(cfg, entry_sqrt);

    // Mark the held inventory through the path for drawdown.
    let mut peak_equity = cfg.capital_usd;
    let mut max_drawdown = 0.0_f64;
    let mut min_equity = cfg.capital_usd;
    for swap in swaps {
        let post_sqrt = swap.sqrt_price_x96 / TWO_POW_96;
        let equity_usd = cfg.token0_to_usd(cfg.value_token0(a0, a1, post_sqrt * post_sqrt));
        peak_equity = peak_equity.max(equity_usd);
        max_drawdown = max_drawdown.max(peak_equity - equity_usd);
        min_equity = min_equity.min(equity_usd);
    }

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
        net_vs_hold_usd: net_pnl_usd - hold_net,
        rebalances: 0,
        swaps_in_range: 0,
        swaps_total: swaps.len() as u64,
        time_in_range_pct: 0.0,
        final_tick,
        avg_half_width_ticks: 0.0,
        max_drawdown_usd: max_drawdown,
        min_equity_usd: min_equity,
        max_one_sided_risk_blocks: 0,
        toxic_fee_usd: 0.0,
        hedge_pnl_usd: 0.0,
        funding_cost_usd: 0.0,
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

/// Run the standard baseline battery over a swap stream (default execution model).
pub fn run_baseline_battery(swaps: &[SwapObs], cfg: &ReplayConfig) -> Vec<PolicyReport> {
    // Threshold 6.0: calibrated on real (noisy) tick data, where 2.0 over-triggers.
    run_baseline_battery_with(swaps, cfg, &ExecConfig::default(), 600, 6000, 1.5, 1.0, 6.0)
}

/// Run baselines with explicit execution model, narrow/wide half-widths, a vol
/// multiplier, a hedge fraction, and the adaptive trend-exit threshold.
#[allow(clippy::too_many_arguments)]
pub fn run_baseline_battery_with(
    swaps: &[SwapObs],
    cfg: &ReplayConfig,
    exec: &ExecConfig,
    narrow_half_width: i32,
    wide_half_width: i32,
    vol_k: f64,
    hedge_fraction: f64,
    adaptive_trend_threshold: f64,
) -> Vec<PolicyReport> {
    if swaps.is_empty() {
        return Vec::new();
    }
    vec![
        run_policy(swaps, cfg, exec, RangeMode::HoldInventory, "hold_50_50"),
        run_policy(
            swaps,
            cfg,
            exec,
            RangeMode::StaticWide {
                half_width_ticks: wide_half_width,
            },
            "passive_wide",
        ),
        run_policy(
            swaps,
            cfg,
            exec,
            RangeMode::CenteredRebalance {
                half_width_ticks: narrow_half_width,
                rebalance: false,
            },
            "narrow_static",
        ),
        run_policy(
            swaps,
            cfg,
            exec,
            RangeMode::CenteredRebalance {
                half_width_ticks: narrow_half_width,
                rebalance: true,
            },
            "narrow_rebalance",
        ),
        run_policy(
            swaps,
            cfg,
            exec,
            RangeMode::VolatilityScaled {
                k: vol_k,
                floor_ticks: narrow_half_width / 2,
                cap_ticks: wide_half_width,
                window: 200,
            },
            "vol_scaled_rebalance",
        ),
        run_policy(
            swaps,
            cfg,
            exec,
            RangeMode::HardExitStop {
                half_width_ticks: narrow_half_width,
            },
            "hard_exit_stop",
        ),
        run_policy(
            swaps,
            cfg,
            exec,
            RangeMode::HedgedRebalance {
                half_width_ticks: narrow_half_width,
                hedge_fraction,
            },
            "hedged_narrow",
        ),
        run_policy(
            swaps,
            cfg,
            exec,
            RangeMode::Adaptive {
                vol_k,
                floor_ticks: narrow_half_width / 2,
                cap_ticks: wide_half_width,
                trend_exit_threshold: adaptive_trend_threshold,
                window: 200,
            },
            "adaptive_regime",
        ),
    ]
}

/// Synthetic price scenarios for stress-testing policies when real data lacks the
/// regime of interest (e.g. a crash). `Crash` = the risk asset (token1) collapses,
/// which raises the pool tick; `Pump` = the opposite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    Calm,
    Pump,
    Crash,
    Chop,
}

impl Scenario {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "calm" => Some(Self::Calm),
            "pump" => Some(Self::Pump),
            "crash" => Some(Self::Crash),
            "chop" => Some(Self::Chop),
            _ => None,
        }
    }
}

/// Build a deterministic synthetic swap stream for a scenario. `move_ticks` is the
/// net tick travel for trending scenarios / amplitude for chop.
pub fn scenario_swaps(
    kind: Scenario,
    start_tick: i32,
    n: usize,
    move_ticks: i32,
    swap_size_token0: f64,
    liquidity: f64,
) -> Vec<SwapObs> {
    let n = n.max(2);
    let mut rng: u64 = 0x9E37_79B9_7F4A_7C15; // fixed seed -> reproducible
    let mut next_noise = |amp: f64| -> f64 {
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let unit = ((rng >> 33) as f64 / (1u64 << 31) as f64) - 1.0; // [-1, 1)
        unit * amp
    };

    (0..n)
        .map(|i| {
            let frac = i as f64 / (n - 1) as f64;
            let tick = match kind {
                Scenario::Calm => start_tick + next_noise(40.0).round() as i32,
                Scenario::Pump => {
                    start_tick - (move_ticks as f64 * frac).round() as i32
                        + next_noise(30.0).round() as i32
                }
                Scenario::Crash => {
                    // Front-loaded collapse: most of the move in the first third.
                    let shape = (frac * 3.0).min(1.0);
                    start_tick
                        + (move_ticks as f64 * shape).round() as i32
                        + next_noise(30.0).round() as i32
                }
                Scenario::Chop => {
                    start_tick
                        + (move_ticks as f64 * (i as f64 * 0.3).sin()).round() as i32
                        + next_noise(50.0).round() as i32
                }
            };
            let sqrt = sqrt_ratio_at_tick(tick);
            // Alternate input side so both tokens see flow.
            let (amount0, amount1) = if i % 2 == 0 {
                (swap_size_token0, -swap_size_token0)
            } else {
                (-swap_size_token0, swap_size_token0)
            };
            SwapObs {
                block: 1000 + i as u64,
                log_index: 0,
                amount0,
                amount1,
                sqrt_price_x96: sqrt * TWO_POW_96,
                liquidity,
                tick,
            }
        })
        .collect()
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
        let reports = run_baseline_battery_with(
            &swaps,
            &cfg,
            &ExecConfig::default(),
            100,
            6000,
            1.5,
            1.0,
            2.0,
        );
        let reb = reports
            .iter()
            .find(|r| r.policy == "narrow_rebalance")
            .unwrap();
        assert!(
            reb.rebalances > 0,
            "trending price should trigger rebalances"
        );
    }

    #[test]
    fn crash_scenario_hard_exit_caps_tail_and_hedge_pays() {
        let cfg = cfg();
        let exec = ExecConfig {
            action_delay_blocks: 3,
            block_seconds: 2.0,
            funding_bps_per_day: 10.0,
            risk_asset_is_token1: true,
        };
        // AERO crashes: tick rises ~6000 (≈ +82% pool price, AERO ≈ -45%).
        let swaps = scenario_swaps(Scenario::Crash, 80_000, 1_500, 6_000, 1e18, 1e24);
        let reports = run_baseline_battery_with(&swaps, &cfg, &exec, 300, 6000, 1.5, 1.0, 2.0);
        let get = |name: &str| reports.iter().find(|r| r.policy == name).unwrap().clone();

        let narrow_static = get("narrow_static");
        let hard_exit = get("hard_exit_stop");
        let hedged = get("hedged_narrow");

        // One-way exit should lose less than passively holding the falling knife.
        assert!(
            hard_exit.net_pnl_usd > narrow_static.net_pnl_usd,
            "hard_exit {} should beat narrow_static {} in a crash",
            hard_exit.net_pnl_usd,
            narrow_static.net_pnl_usd
        );
        // And it should cap the drawdown.
        assert!(
            hard_exit.max_drawdown_usd < narrow_static.max_drawdown_usd,
            "hard_exit drawdown {} < narrow_static {}",
            hard_exit.max_drawdown_usd,
            narrow_static.max_drawdown_usd
        );
        // The short hedge pays off when the risk asset collapses.
        assert!(
            hedged.hedge_pnl_usd > 0.0,
            "hedge should profit in a crash, got {}",
            hedge_pnl(&hedged)
        );
        assert!(
            hedged.net_pnl_usd > narrow_static.net_pnl_usd,
            "hedged should beat unhedged narrow in a crash"
        );
        // Patiently holding a narrow band through the crash strands inventory.
        assert!(narrow_static.max_one_sided_risk_blocks > 0);
    }

    fn hedge_pnl(r: &PolicyReport) -> f64 {
        r.hedge_pnl_usd
    }

    #[test]
    fn adaptive_survives_crash_and_chop() {
        let cfg = cfg();
        let exec = ExecConfig {
            action_delay_blocks: 3,
            block_seconds: 2.0,
            funding_bps_per_day: 0.0,
            risk_asset_is_token1: true,
        };
        let pick = |reports: &[PolicyReport], name: &str| {
            reports.iter().find(|r| r.policy == name).unwrap().clone()
        };

        // Crash: adaptive should exit on the trend and beat patient narrow_static.
        let crash = scenario_swaps(Scenario::Crash, 80_000, 1_500, 6_000, 1e18, 1e24);
        let cr = run_baseline_battery_with(&crash, &cfg, &exec, 300, 6000, 1.5, 1.0, 2.0);
        let adaptive = pick(&cr, "adaptive_regime");
        let narrow_static = pick(&cr, "narrow_static");
        assert!(
            adaptive.net_pnl_usd > narrow_static.net_pnl_usd,
            "adaptive {} should beat narrow_static {} in a crash",
            adaptive.net_pnl_usd,
            narrow_static.net_pnl_usd
        );
        assert!(
            adaptive.max_drawdown_usd < narrow_static.max_drawdown_usd,
            "adaptive should cap drawdown in a crash"
        );

        // Chop: adaptive should not death-spiral like mechanical rebalancing.
        let chop = scenario_swaps(Scenario::Chop, 80_000, 1_500, 6_000, 1e18, 1e24);
        let ch = run_baseline_battery_with(&chop, &cfg, &exec, 300, 6000, 1.5, 1.0, 2.0);
        let adaptive_chop = pick(&ch, "adaptive_regime");
        let narrow_rebalance_chop = pick(&ch, "narrow_rebalance");
        assert!(
            adaptive_chop.net_pnl_usd > narrow_rebalance_chop.net_pnl_usd,
            "adaptive {} should avoid the chop death spiral vs narrow_rebalance {}",
            adaptive_chop.net_pnl_usd,
            narrow_rebalance_chop.net_pnl_usd
        );
    }
}
