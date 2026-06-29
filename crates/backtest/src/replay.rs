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

    /// Return this swap with token0/token1 roles swapped (price inverted). Lets a
    /// pool whose stable/numeraire leg is token1 (e.g. CTR-USDC, USDC = token1) be
    /// replayed with the stable as token0, so the engine's token0-numeraire and
    /// `risk_asset_is_token1` conventions hold. Decimals must be swapped by caller.
    pub fn inverted(self) -> SwapObs {
        let two_pow_192 = TWO_POW_96 * TWO_POW_96;
        SwapObs {
            amount0: self.amount1,
            amount1: self.amount0,
            sqrt_price_x96: if self.sqrt_price_x96 > 0.0 {
                two_pow_192 / self.sqrt_price_x96
            } else {
                0.0
            },
            tick: -self.tick,
            ..self
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
    /// Annual reward (gauge emission) APR earned on staked value while in range,
    /// as a fraction (e.g. 0.2249 for 22.49%). 0 disables reward income.
    pub reward_apr: f64,
    /// Haircut on reward income for liquidation cost / token risk (0.1 = keep 90%).
    pub reward_haircut: f64,
}

const SECONDS_PER_YEAR: f64 = 31_557_600.0;

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
    /// LP on/off gate. The meta-decision *whether to be an LP at all*: LP only wins
    /// in ranging regimes, so when a trend is detected in *either* direction the
    /// gate stands the position down to the money leg (conservative de-risk that
    /// protects against the down-crash; giving up an up-trend's upside is the price
    /// of not taking a directional bet). It resumes LPing once price ranges again.
    /// Hysteresis: stand down at `enter_threshold`, resume below `exit_threshold`.
    RegimeGated {
        vol_k: f64,
        floor_ticks: i32,
        cap_ticks: i32,
        enter_threshold: f64,
        exit_threshold: f64,
        window: usize,
    },
    /// Narrow rebalancing LP plus a *dynamic* short hedge that tracks the position's
    /// changing AERO delta (its token1 holding), rehedged when the delta drifts past
    /// `rehedge_band` of the entry exposure. Neutralizes inventory beta so the LP's
    /// fee−LVR alpha can be harvested in any regime (the static hedge cannot).
    DeltaHedged {
        half_width_ticks: i32,
        rehedge_band: f64,
    },
    /// Narrow dynamic-delta LP with an intra-window trend stop. It keeps the
    /// fee-harvesting shape of `DeltaHedged` in range/chop, but stands down to the
    /// money leg as soon as recent tick displacement becomes a strong trend.
    DeltaTrendStop {
        half_width_ticks: i32,
        rehedge_band: f64,
        enter_threshold: f64,
        exit_threshold: f64,
        window: usize,
    },
    /// The deployable shape: a wide, never-rebalanced band (low churn, harvests
    /// fees) plus a dynamic delta hedge (kills the inventory beta a wide band still
    /// carries). Combines passive-wide's positive expectancy with low variance.
    HedgedWide {
        half_width_ticks: i32,
        rehedge_band: f64,
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
    /// Loss-versus-rebalancing: value bled to arbitrageurs (≥0), the path-robust
    /// adverse-selection cost. Unlike IL-vs-hold it excludes price beta.
    pub lvr_usd: f64,
    /// Fee income minus LVR — the LP's pure edge (alpha) net of adverse selection.
    /// Positive means fees more than pay for the arbitrage bleed.
    pub fee_minus_lvr_usd: f64,
    /// Reward (gauge emission) income earned while staked in range, after haircut.
    pub reward_income_usd: f64,
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
    let gated = matches!(mode, RangeMode::RegimeGated { .. });
    let (gate_enter, gate_exit, gate_window) = match mode {
        RangeMode::RegimeGated {
            enter_threshold,
            exit_threshold,
            window,
            ..
        } => (enter_threshold, exit_threshold, window),
        _ => (f64::INFINITY, 0.0, 200),
    };
    let delta_trend_stop = matches!(mode, RangeMode::DeltaTrendStop { .. });
    let (trend_stop_enter, trend_stop_exit, trend_stop_window) = match mode {
        RangeMode::DeltaTrendStop {
            enter_threshold,
            exit_threshold,
            window,
            ..
        } => (enter_threshold, exit_threshold, window),
        _ => (f64::INFINITY, 0.0, 25),
    };
    let hedge_fraction = match mode {
        RangeMode::HedgedRebalance { hedge_fraction, .. } => hedge_fraction,
        _ => 0.0,
    };
    let hedged = hedge_fraction > 0.0;
    // Dynamic delta hedge applies to both DeltaHedged (narrow, rebalancing) and
    // HedgedWide (wide, static) — only the range behaviour differs.
    let delta_hedged = matches!(
        mode,
        RangeMode::DeltaHedged { .. }
            | RangeMode::DeltaTrendStop { .. }
            | RangeMode::HedgedWide { .. }
    );
    let rehedge_band = match mode {
        RangeMode::DeltaHedged { rehedge_band, .. }
        | RangeMode::DeltaTrendStop { rehedge_band, .. }
        | RangeMode::HedgedWide { rehedge_band, .. } => rehedge_band,
        _ => f64::INFINITY,
    };
    let any_hedge = hedged || delta_hedged;

    let mut half_width_sum = 0.0_f64;
    let mut half_width_count = 0u64;
    let mut tick_window: Vec<i32> = Vec::new();

    let initial_half = initial_half_width(&mode, &tick_window);
    half_width_sum += initial_half as f64;
    half_width_count += 1;
    let (mut lower, mut upper) = centered_range(first.tick, initial_half);
    let mut position: Option<Position> = Some(Position::open(cfg, entry_sqrt, lower, upper));
    let mut sidelined_token0 = 0.0_f64; // value parked in the money leg (token0)

    // Short hedge on the risk asset (token1). `hedged` = static (entry-sized, never
    // updated); `delta_hedged` = dynamic (tracks the position's current AERO delta).
    let entry_a1_human = position.as_ref().unwrap().amount1_entry / cfg.scale1();
    let mut short_aero = if hedged {
        hedge_fraction * entry_a1_human
    } else if delta_hedged {
        entry_a1_human
    } else {
        0.0
    };
    let mut hedge_pnl_token0 = 0.0_f64;
    let mut q_prev = risk_price_token0(cfg, entry_sqrt);

    let mut fees_token0 = 0.0_f64;
    let mut toxic_fee_token0 = 0.0_f64;
    let mut lvr_token0 = 0.0_f64;
    let mut reward_token0 = 0.0_f64;
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
                // Gauge emissions accrue per unit time on staked value while in range.
                if cfg.reward_apr > 0.0 {
                    let dt_secs = swap.block.saturating_sub(prev_block) as f64 * exec.block_seconds;
                    let staked_value = pos.value_token0(cfg, pre_sqrt);
                    reward_token0 += staked_value * cfg.reward_apr / SECONDS_PER_YEAR * dt_secs;
                }
            }
        }

        let post_sqrt = swap.sqrt_price_x96 / TWO_POW_96;
        tick_window.push(swap.tick);

        // Loss-versus-rebalancing: the active position's old holdings marked at the
        // new price minus the LP's actual new value = what it bled to arbitrageurs.
        if let Some(pos) = &position {
            let (x0, y0) = pos.amounts_at(pre_sqrt);
            let old_at_new = cfg.value_token0(x0, y0, post_sqrt * post_sqrt);
            let lp_at_new = pos.value_token0(cfg, post_sqrt);
            lvr_token0 += (old_at_new - lp_at_new).max(0.0);
        }

        // Short-hedge mark-to-market + funding (incremental, supports dynamic rehedge).
        // PnL uses the short held *during* this price move; rehedging happens after
        // the position actions below.
        if any_hedge {
            let q_now = risk_price_token0(cfg, post_sqrt);
            hedge_pnl_token0 += short_aero * (q_prev - q_now);
            q_prev = q_now;
            let dt_days =
                swap.block.saturating_sub(prev_block) as f64 * exec.block_seconds / 86_400.0;
            let short_notional_usd = cfg.token0_to_usd(short_aero * q_now);
            funding_usd += exec.funding_bps_per_day / 10_000.0 * short_notional_usd * dt_days;
        }

        // LP on/off gate: stand down to the money leg on any strong trend, resume
        // LPing when ranging. Whether to be an LP at all dominates the width choice.
        if gated {
            let regime = assess_regime(&tick_window, gate_window);
            let trending = regime.trend_strength >= gate_enter;
            let ranging = regime.trend_strength <= gate_exit;
            if position.is_some() {
                if trending {
                    let (exec_sqrt, _) =
                        price_after_delay(swaps, i, swap.block, exec.action_delay_blocks);
                    let val = position.as_ref().unwrap().value_token0(cfg, exec_sqrt);
                    let notional_usd = cfg.token0_to_usd(val) * cfg.rebalance_swap_fraction;
                    let slip = notional_usd * cfg.rebalance_slippage_bps / 10_000.0;
                    gas_usd += cfg.rebalance_gas_usd;
                    slippage_usd += slip;
                    rebalances += 1;
                    sidelined_token0 =
                        (val - (cfg.rebalance_gas_usd + slip) / cfg.token0_usd).max(0.0);
                    position = None;
                }
                // else: hold the band — no churn while neither trending nor ranging.
            } else if ranging {
                // Resume LPing: re-form the LP ratio from the money leg.
                let val = sidelined_token0;
                let notional_usd = cfg.token0_to_usd(val) * cfg.rebalance_swap_fraction;
                let slip = notional_usd * cfg.rebalance_slippage_bps / 10_000.0;
                gas_usd += cfg.rebalance_gas_usd;
                slippage_usd += slip;
                rebalances += 1;
                let half = next_half_width(&mode, &tick_window);
                half_width_sum += half as f64;
                half_width_count += 1;
                let (nl, nu) = centered_range(swap.tick, half);
                lower = nl;
                upper = nu;
                let net = (val - (cfg.rebalance_gas_usd + slip) / cfg.token0_usd).max(0.0);
                position = Some(Position::open_with_capital(
                    cfg, post_sqrt, lower, upper, net,
                ));
                sidelined_token0 = 0.0;
            }
        } else if delta_trend_stop {
            let regime = assess_regime(&tick_window, trend_stop_window);
            let trending = regime.trend_strength >= trend_stop_enter;
            let ranging = regime.trend_strength <= trend_stop_exit;
            if position.is_some() {
                let in_range = position.as_ref().unwrap().in_range(post_sqrt);
                if trending {
                    let (exec_sqrt, _) =
                        price_after_delay(swaps, i, swap.block, exec.action_delay_blocks);
                    let val = position.as_ref().unwrap().value_token0(cfg, exec_sqrt);
                    let notional_usd = cfg.token0_to_usd(val) * cfg.rebalance_swap_fraction;
                    let slip = notional_usd * cfg.rebalance_slippage_bps / 10_000.0;
                    gas_usd += cfg.rebalance_gas_usd;
                    slippage_usd += slip;
                    rebalances += 1;
                    sidelined_token0 =
                        (val - (cfg.rebalance_gas_usd + slip) / cfg.token0_usd).max(0.0);
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
            } else if ranging {
                let val = sidelined_token0;
                let notional_usd = cfg.token0_to_usd(val) * cfg.rebalance_swap_fraction;
                let slip = notional_usd * cfg.rebalance_slippage_bps / 10_000.0;
                gas_usd += cfg.rebalance_gas_usd;
                slippage_usd += slip;
                rebalances += 1;
                let half = next_half_width(&mode, &tick_window);
                half_width_sum += half as f64;
                half_width_count += 1;
                let (nl, nu) = centered_range(swap.tick, half);
                lower = nl;
                upper = nu;
                let net = (val - (cfg.rebalance_gas_usd + slip) / cfg.token0_usd).max(0.0);
                position = Some(Position::open_with_capital(
                    cfg, post_sqrt, lower, upper, net,
                ));
                sidelined_token0 = 0.0;
            }
        } else if adaptive {
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

        // Dynamic delta rehedge: keep the short equal to the position's current AERO
        // holding once it drifts past the band (relative to entry exposure).
        if delta_hedged {
            let target = position
                .as_ref()
                .map(|p| p.amounts_at(post_sqrt).1 / cfg.scale1())
                .unwrap_or(0.0);
            let drift = (target - short_aero).abs();
            let reference = entry_a1_human.max(1e-12);
            if drift > rehedge_band * reference || (target == 0.0 && short_aero > 0.0) {
                let rehedge_notional_usd = cfg.token0_to_usd(drift * q_prev);
                slippage_usd += rehedge_notional_usd * cfg.rebalance_slippage_bps / 10_000.0;
                short_aero = target;
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
        let reward_kept = reward_token0 * (1.0 - cfg.reward_haircut);
        let equity_usd = cfg
            .token0_to_usd(pos_val_t0 + fees_token0 + hedge_pnl_token0 + reward_kept)
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
    let hedge_pnl_usd = cfg.token0_to_usd(hedge_pnl_token0);
    let fee_income_usd = cfg.token0_to_usd(fees_token0);
    let reward_kept = reward_token0 * (1.0 - cfg.reward_haircut);
    let reward_income_usd = cfg.token0_to_usd(reward_kept);
    let lvr_usd = cfg.token0_to_usd(lvr_token0);
    let final_value_usd =
        cfg.token0_to_usd(pos_val_t0 + fees_token0 + hedge_pnl_token0 + reward_kept);

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
        lvr_usd,
        fee_minus_lvr_usd: fee_income_usd - lvr_usd,
        reward_income_usd,
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
        RangeMode::RegimeGated { floor_ticks, .. } => *floor_ticks.max(&1),
        RangeMode::DeltaHedged {
            half_width_ticks, ..
        } => *half_width_ticks,
        RangeMode::DeltaTrendStop {
            half_width_ticks, ..
        } => *half_width_ticks,
        RangeMode::HedgedWide {
            half_width_ticks, ..
        } => *half_width_ticks,
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
        RangeMode::RegimeGated {
            vol_k,
            floor_ticks,
            cap_ticks,
            window: w,
            ..
        } => {
            let vol = rolling_tick_vol(window, *w);
            ((vol_k * vol).round() as i32).clamp(*floor_ticks, *cap_ticks)
        }
        RangeMode::DeltaHedged {
            half_width_ticks, ..
        } => *half_width_ticks,
        RangeMode::DeltaTrendStop {
            half_width_ticks, ..
        } => *half_width_ticks,
        RangeMode::HedgedWide {
            half_width_ticks, ..
        } => *half_width_ticks,
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
            | RangeMode::DeltaHedged { .. }
            | RangeMode::DeltaTrendStop { .. }
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

/// Result of simulating a single-tick v3/Slipstream swap against live pool state.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SwapSim {
    /// Expected output amount (raw units of the other token).
    pub amount_out: f64,
    /// |price change| of the swap, in basis points.
    pub price_impact_bps: f64,
    pub sqrt_x96_after: f64,
}

/// Simulate an exact-input swap against the pool's current in-range liquidity using
/// the Uniswap-v3 closed form (assumes the swap stays within the active tick — true
/// for swaps small relative to in-range liquidity; it under-states impact for large
/// swaps that cross ticks). Inputs are raw units. `zero_for_one` = selling token0.
pub fn simulate_v3_swap(
    sqrt_x96: f64,
    liquidity: f64,
    fee_fraction: f64,
    amount_in: f64,
    zero_for_one: bool,
) -> SwapSim {
    let s = sqrt_x96 / TWO_POW_96;
    if liquidity <= 0.0 || s <= 0.0 || amount_in <= 0.0 {
        return SwapSim {
            amount_out: 0.0,
            price_impact_bps: 0.0,
            sqrt_x96_after: sqrt_x96,
        };
    }
    let ain = amount_in * (1.0 - fee_fraction);
    let (s_new, amount_out) = if zero_for_one {
        // token0 in → price falls: 1/s_new = 1/s + ain/L; token1 out.
        let s_new = 1.0 / (1.0 / s + ain / liquidity);
        (s_new, liquidity * (s - s_new))
    } else {
        // token1 in → price rises: s_new = s + ain/L; token0 out.
        let s_new = s + ain / liquidity;
        (s_new, liquidity * (1.0 / s - 1.0 / s_new))
    };
    let impact = ((s_new * s_new) / (s * s) - 1.0).abs() * 10_000.0;
    SwapSim {
        amount_out: amount_out.max(0.0),
        price_impact_bps: impact,
        sqrt_x96_after: s_new * TWO_POW_96,
    }
}

/// Token amounts (human units) and raw liquidity for opening a concentrated
/// position of `capital_token0` (human token0) across `[lower_tick, upper_tick]` at
/// `current_sqrt_x96`. The same v3 math used in the replay — exposed for the
/// execution planner so the dry-run and the backtest agree on inventory.
pub fn cl_mint_amounts(
    decimals0: u8,
    decimals1: u8,
    lower_tick: i32,
    upper_tick: i32,
    current_sqrt_x96: f64,
    capital_token0: f64,
) -> (f64, f64, f64) {
    let cfg = ReplayConfig {
        decimals0,
        decimals1,
        fee_fraction: 0.0,
        token0_usd: 1.0,
        capital_usd: capital_token0,
        rebalance_gas_usd: 0.0,
        rebalance_slippage_bps: 0.0,
        rebalance_swap_fraction: 0.5,
        reward_apr: 0.0,
        reward_haircut: 0.0,
    };
    let sqrt = current_sqrt_x96 / TWO_POW_96;
    let pos = Position::open_with_capital(&cfg, sqrt, lower_tick, upper_tick, capital_token0);
    let (a0, a1) = pos.amounts_at(sqrt);
    (a0 / cfg.scale0(), a1 / cfg.scale1(), pos.liquidity)
}

/// Token amounts (human units) for an existing concentrated-liquidity position
/// with known raw `liquidity` across `[lower_tick, upper_tick]` at
/// `current_sqrt_x96`.
pub fn cl_position_amounts(
    decimals0: u8,
    decimals1: u8,
    lower_tick: i32,
    upper_tick: i32,
    current_sqrt_x96: f64,
    liquidity: f64,
) -> (f64, f64) {
    let cfg = ReplayConfig {
        decimals0,
        decimals1,
        fee_fraction: 0.0,
        token0_usd: 1.0,
        capital_usd: 0.0,
        rebalance_gas_usd: 0.0,
        rebalance_slippage_bps: 0.0,
        rebalance_swap_fraction: 0.5,
        reward_apr: 0.0,
        reward_haircut: 0.0,
    };
    let sqrt = current_sqrt_x96 / TWO_POW_96;
    let pos = Position {
        liquidity,
        sqrt_lower: sqrt_ratio_at_tick(lower_tick),
        sqrt_upper: sqrt_ratio_at_tick(upper_tick),
        amount0_entry: 0.0,
        amount1_entry: 0.0,
    };
    let (a0, a1) = pos.amounts_at(sqrt);
    (a0 / cfg.scale0(), a1 / cfg.scale1())
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
        lvr_usd: 0.0,
        fee_minus_lvr_usd: 0.0,
        reward_income_usd: 0.0,
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
    let trend_stop_enter = (adaptive_trend_threshold / 2.0).max(2.0);
    let trend_stop_exit = (trend_stop_enter / 2.0).max(1.0);
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
        run_policy(
            swaps,
            cfg,
            exec,
            RangeMode::RegimeGated {
                vol_k,
                floor_ticks: narrow_half_width / 2,
                cap_ticks: wide_half_width,
                enter_threshold: adaptive_trend_threshold,
                exit_threshold: adaptive_trend_threshold / 2.0,
                window: 200,
            },
            "regime_gated",
        ),
        run_policy(
            swaps,
            cfg,
            exec,
            RangeMode::DeltaHedged {
                half_width_ticks: narrow_half_width,
                rehedge_band: 0.25,
            },
            "delta_hedged",
        ),
        run_policy(
            swaps,
            cfg,
            exec,
            RangeMode::DeltaTrendStop {
                half_width_ticks: narrow_half_width,
                rehedge_band: 0.25,
                enter_threshold: trend_stop_enter,
                exit_threshold: trend_stop_exit,
                window: 25,
            },
            "delta_trend_stop",
        ),
        run_policy(
            swaps,
            cfg,
            exec,
            RangeMode::HedgedWide {
                half_width_ticks: wide_half_width,
                rehedge_band: 0.25,
            },
            "hedged_wide",
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

/// Run a single policy over a swap stream, returning `None` for an empty stream.
pub fn run_single_policy(
    swaps: &[SwapObs],
    cfg: &ReplayConfig,
    exec: &ExecConfig,
    mode: RangeMode,
    name: &str,
) -> Option<PolicyReport> {
    if swaps.is_empty() {
        return None;
    }
    Some(run_policy(swaps, cfg, exec, mode, name))
}

/// Walk-forward calibration: roll a train window forward, pick the adaptive
/// parameters that maximize a risk-adjusted score on the (past) train window, then
/// apply them out-of-sample on the next test window. Parameters are never chosen
/// using the data they are scored on.
#[derive(Debug, Clone)]
pub struct WalkForwardConfig {
    pub train_swaps: usize,
    pub test_swaps: usize,
    pub thresholds: Vec<f64>,
    pub half_widths: Vec<i32>,
    pub vol_k: f64,
    pub cap_ticks: i32,
    pub window: usize,
    /// Objective on the train window is `net - drawdown_penalty * max_drawdown`.
    pub drawdown_penalty: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FoldResult {
    pub fold: usize,
    pub train_start: usize,
    pub train_end: usize,
    pub test_end: usize,
    pub chosen_threshold: f64,
    pub chosen_half_width: i32,
    pub test_net_usd: f64,
    pub test_max_drawdown_usd: f64,
    pub test_rebalances: u32,
    pub test_tick_span: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalkForwardReport {
    pub folds: Vec<FoldResult>,
    pub test_swaps_total: usize,
    /// Out-of-sample net of the walk-forward (per-fold calibrated) adaptive policy.
    pub oos_net_usd: f64,
    pub oos_max_drawdown_usd: f64,
    /// Same test segments, adaptive with a single fixed (median-grid) threshold.
    pub fixed_adaptive_net_usd: f64,
    /// Same test segments, a fixed narrow static band (median-grid width).
    pub static_net_usd: f64,
    /// Same test segments, hold 50/50.
    pub hold_net_usd: f64,
}

fn adaptive_mode(wf: &WalkForwardConfig, threshold: f64, half_width: i32) -> RangeMode {
    RangeMode::Adaptive {
        vol_k: wf.vol_k,
        floor_ticks: (half_width / 2).max(1),
        cap_ticks: wf.cap_ticks,
        trend_exit_threshold: threshold,
        window: wf.window,
    }
}

fn median<T: Copy + PartialOrd>(values: &[T]) -> T {
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

pub fn walk_forward(
    swaps: &[SwapObs],
    cfg: &ReplayConfig,
    exec: &ExecConfig,
    wf: &WalkForwardConfig,
) -> WalkForwardReport {
    let n = swaps.len();
    let fixed_threshold = if wf.thresholds.is_empty() {
        6.0
    } else {
        median(&wf.thresholds)
    };
    let fixed_half_width = if wf.half_widths.is_empty() {
        300
    } else {
        median(&wf.half_widths)
    };

    let mut folds = Vec::new();
    let mut oos_net = 0.0;
    let mut oos_dd = 0.0_f64;
    let mut fixed_net = 0.0;
    let mut static_net = 0.0;
    let mut hold_net = 0.0;
    let mut test_total = 0usize;

    let mut start = 0usize;
    let mut fold_idx = 0usize;
    while start + wf.train_swaps < n {
        let train_end = start + wf.train_swaps;
        let test_end = (train_end + wf.test_swaps).min(n);
        if test_end <= train_end {
            break;
        }
        let train = &swaps[start..train_end];
        let test = &swaps[train_end..test_end];

        // Calibrate on the train window only.
        let mut best: Option<(f64, f64, i32)> = None;
        for &thr in &wf.thresholds {
            for &hw in &wf.half_widths {
                if let Some(report) =
                    run_single_policy(train, cfg, exec, adaptive_mode(wf, thr, hw), "cal")
                {
                    let score = report.net_pnl_usd - wf.drawdown_penalty * report.max_drawdown_usd;
                    if best.map_or(true, |(s, _, _)| score > s) {
                        best = Some((score, thr, hw));
                    }
                }
            }
        }
        let (_, thr, hw) = best.unwrap_or((0.0, fixed_threshold, fixed_half_width));

        // Apply out-of-sample on the test window.
        let test_report =
            run_single_policy(test, cfg, exec, adaptive_mode(wf, thr, hw), "wf").unwrap();
        let fixed_report = run_single_policy(
            test,
            cfg,
            exec,
            adaptive_mode(wf, fixed_threshold, fixed_half_width),
            "fixed",
        )
        .unwrap();
        let static_report = run_single_policy(
            test,
            cfg,
            exec,
            RangeMode::CenteredRebalance {
                half_width_ticks: fixed_half_width,
                rebalance: false,
            },
            "static",
        )
        .unwrap();
        let hold_report =
            run_single_policy(test, cfg, exec, RangeMode::HoldInventory, "hold").unwrap();

        let tick_span = {
            let ticks = test.iter().map(|s| s.tick);
            let max = ticks.clone().max().unwrap_or(0);
            let min = ticks.min().unwrap_or(0);
            max - min
        };

        oos_net += test_report.net_pnl_usd;
        oos_dd = oos_dd.max(test_report.max_drawdown_usd);
        fixed_net += fixed_report.net_pnl_usd;
        static_net += static_report.net_pnl_usd;
        hold_net += hold_report.net_pnl_usd;
        test_total += test.len();

        folds.push(FoldResult {
            fold: fold_idx,
            train_start: start,
            train_end,
            test_end,
            chosen_threshold: thr,
            chosen_half_width: hw,
            test_net_usd: test_report.net_pnl_usd,
            test_max_drawdown_usd: test_report.max_drawdown_usd,
            test_rebalances: test_report.rebalances,
            test_tick_span: tick_span,
        });

        fold_idx += 1;
        start += wf.test_swaps;
    }

    WalkForwardReport {
        folds,
        test_swaps_total: test_total,
        oos_net_usd: oos_net,
        oos_max_drawdown_usd: oos_dd,
        fixed_adaptive_net_usd: fixed_net,
        static_net_usd: static_net,
        hold_net_usd: hold_net,
    }
}

/// Aggregate distribution of one policy's outcome across bootstrapped paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDistribution {
    pub policy: String,
    pub mean_net_usd: f64,
    pub std_net_usd: f64,
    pub p05_net_usd: f64,
    pub p50_net_usd: f64,
    pub p95_net_usd: f64,
    /// Fraction of paths where this policy's net beat hold's net.
    pub win_rate_vs_hold: f64,
    pub mean_fee_minus_lvr_usd: f64,
    pub mean_max_drawdown_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiPathReport {
    pub n_paths: usize,
    pub swaps_per_path: usize,
    pub block_len: usize,
    pub policies: Vec<PolicyDistribution>,
}

/// Moving-block bootstrap of a real swap stream: resample contiguous blocks of
/// (tick-increment, amounts, liquidity, block-gap), accumulate into a new tick
/// path, and rebuild swaps. Preserves local volatility/microstructure and the
/// direction-size coupling while randomizing the overall realized direction.
fn bootstrap_path(
    src: &[SwapObs],
    n: usize,
    block_len: usize,
    drift_adjust: f64,
    rng: &mut u64,
) -> Vec<SwapObs> {
    let m = src.len();
    let block_len = block_len.max(1);
    let mut out: Vec<SwapObs> = Vec::with_capacity(n);
    let mut cur_tick_f = src[0].tick as f64;
    let mut cur_block = src[0].block;
    while out.len() < n {
        *rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let bs = ((*rng >> 33) as usize) % m;
        for k in 0..block_len {
            let idx = bs + k;
            if idx >= m {
                break;
            }
            let s = &src[idx];
            let dtick = if idx == 0 {
                0
            } else {
                src[idx].tick - src[idx - 1].tick
            };
            let dblock = if idx == 0 {
                1
            } else {
                src[idx].block.saturating_sub(src[idx - 1].block)
            };
            // `drift_adjust` removes the source's mean per-swap drift so paths are
            // martingale (zero net direction) — the regime under which LP_net ≈
            // fee − LVR in expectation.
            cur_tick_f += dtick as f64 - drift_adjust;
            cur_block += dblock;
            let tick = cur_tick_f.round() as i32;
            let sqrt = sqrt_ratio_at_tick(tick);
            out.push(SwapObs {
                block: cur_block,
                log_index: 0,
                amount0: s.amount0,
                amount1: s.amount1,
                sqrt_price_x96: sqrt * TWO_POW_96,
                liquidity: s.liquidity,
                tick,
            });
            if out.len() >= n {
                break;
            }
        }
    }
    out
}

fn percentile(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * pct / 100.0).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Run the baseline battery over `n_paths` bootstrapped resamples of the real swap
/// stream and report each policy's outcome distribution. The point: a delta-hedged
/// LP should show a tight, positive net distribution (≈ fee − LVR) while an
/// unhedged LP shows a wide one (it swings with whichever direction each path took).
#[allow(clippy::too_many_arguments)]
pub fn multi_path_eval(
    swaps: &[SwapObs],
    cfg: &ReplayConfig,
    exec: &ExecConfig,
    narrow_half_width: i32,
    wide_half_width: i32,
    vol_k: f64,
    hedge_fraction: f64,
    adaptive_trend_threshold: f64,
    n_paths: usize,
    block_len: usize,
    seed: u64,
    demean: bool,
) -> MultiPathReport {
    let n = swaps.len();
    let mut rng = seed | 1;
    // Mean per-swap tick drift of the source; removed when `demean` so paths are
    // driftless (martingale) and isolate the LP economics from the directional bet.
    let drift_adjust = if demean && n > 1 {
        (swaps[n - 1].tick - swaps[0].tick) as f64 / (n - 1) as f64
    } else {
        0.0
    };

    let mut names: Vec<String> = Vec::new();
    let mut nets: Vec<Vec<f64>> = Vec::new();
    let mut fee_lvr: Vec<f64> = Vec::new();
    let mut dd: Vec<f64> = Vec::new();
    let mut wins: Vec<u64> = Vec::new();

    for _ in 0..n_paths {
        let path = bootstrap_path(swaps, n, block_len, drift_adjust, &mut rng);
        let reports = run_baseline_battery_with(
            &path,
            cfg,
            exec,
            narrow_half_width,
            wide_half_width,
            vol_k,
            hedge_fraction,
            adaptive_trend_threshold,
        );
        if names.is_empty() {
            names = reports.iter().map(|r| r.policy.clone()).collect();
            nets = vec![Vec::with_capacity(n_paths); reports.len()];
            fee_lvr = vec![0.0; reports.len()];
            dd = vec![0.0; reports.len()];
            wins = vec![0; reports.len()];
        }
        let hold_net = reports
            .iter()
            .find(|r| r.policy == "hold_50_50")
            .map(|r| r.net_pnl_usd)
            .unwrap_or(0.0);
        for (j, r) in reports.iter().enumerate() {
            nets[j].push(r.net_pnl_usd);
            fee_lvr[j] += r.fee_minus_lvr_usd;
            dd[j] += r.max_drawdown_usd;
            if r.net_pnl_usd > hold_net {
                wins[j] += 1;
            }
        }
    }

    let paths = n_paths.max(1) as f64;
    let policies = names
        .into_iter()
        .enumerate()
        .map(|(j, policy)| {
            let mut sorted = nets[j].clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let mean = nets[j].iter().sum::<f64>() / paths;
            let var = nets[j].iter().map(|x| (x - mean).powi(2)).sum::<f64>() / paths;
            PolicyDistribution {
                policy,
                mean_net_usd: mean,
                std_net_usd: var.sqrt(),
                p05_net_usd: percentile(&sorted, 5.0),
                p50_net_usd: percentile(&sorted, 50.0),
                p95_net_usd: percentile(&sorted, 95.0),
                win_rate_vs_hold: wins[j] as f64 / paths,
                mean_fee_minus_lvr_usd: fee_lvr[j] / paths,
                mean_max_drawdown_usd: dd[j] / paths,
            }
        })
        .collect();

    MultiPathReport {
        n_paths,
        swaps_per_path: n,
        block_len,
        policies,
    }
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
            reward_apr: 0.0,
            reward_haircut: 0.0,
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
    fn exported_position_amounts_match_mint_amounts() {
        let sqrt_x96 = sqrt_ratio_at_tick(80_000) * TWO_POW_96;
        let (mint0, mint1, liquidity) = cl_mint_amounts(18, 18, 79_000, 81_000, sqrt_x96, 10.0);
        let (amount0, amount1) = cl_position_amounts(18, 18, 79_000, 81_000, sqrt_x96, liquidity);
        assert!((amount0 - mint0).abs() < 1e-9);
        assert!((amount1 - mint1).abs() < 1e-6);
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
    fn walk_forward_beats_static_across_regime_shift() {
        let cfg = cfg();
        let exec = ExecConfig {
            action_delay_blocks: 3,
            block_seconds: 2.0,
            funding_bps_per_day: 0.0,
            risk_asset_is_token1: true,
        };
        // Calm period followed by a crash — parameters calibrated on the calm train
        // must still protect the out-of-sample crash test folds.
        let mut stream = scenario_swaps(Scenario::Calm, 80_000, 1_000, 6_000, 1e18, 1e24);
        let crash = scenario_swaps(Scenario::Crash, 80_000, 1_000, 6_000, 1e18, 1e24);
        stream.extend(crash);
        for (i, s) in stream.iter_mut().enumerate() {
            s.block = 1000 + i as u64;
        }

        let wf = WalkForwardConfig {
            train_swaps: 400,
            test_swaps: 200,
            thresholds: vec![2.0, 6.0, 10.0],
            half_widths: vec![300],
            vol_k: 1.5,
            cap_ticks: 6000,
            window: 200,
            drawdown_penalty: 0.5,
        };
        let report = walk_forward(&stream, &cfg, &exec, &wf);
        assert!(!report.folds.is_empty(), "should produce folds");
        assert!(
            report.oos_net_usd > report.static_net_usd,
            "walk-forward OOS {} should beat static {} across a regime shift",
            report.oos_net_usd,
            report.static_net_usd
        );
        assert!(
            report.oos_max_drawdown_usd < 2000.0,
            "walk-forward should cap OOS drawdown, got {}",
            report.oos_max_drawdown_usd
        );
    }

    #[test]
    fn v3_swap_sim_has_positive_impact_and_conserves_direction() {
        let sqrt_x96 = sqrt_ratio_at_tick(80_000) * TWO_POW_96;
        let liquidity = 1e24;
        // Sell token0 → get token1 out, price falls, positive impact.
        let sim = simulate_v3_swap(sqrt_x96, liquidity, 0.003, 1e18, true);
        assert!(sim.amount_out > 0.0);
        assert!(sim.price_impact_bps > 0.0);
        assert!(sim.sqrt_x96_after < sqrt_x96, "selling token0 lowers price");
        // A 10× larger swap moves price more (more impact).
        let big = simulate_v3_swap(sqrt_x96, liquidity, 0.003, 1e19, true);
        assert!(big.price_impact_bps > sim.price_impact_bps);
        // Thin liquidity ⇒ much larger impact for the same size.
        let thin = simulate_v3_swap(sqrt_x96, 1e22, 0.003, 1e18, true);
        assert!(thin.price_impact_bps > sim.price_impact_bps);
    }

    #[test]
    fn inverted_swap_flips_price_and_amounts() {
        let data = concat!(
            "0x",
            "0000000000000000000000000000000000000000000000000c44307e19296880",
            "ffffffffffffffffffffffffffffffffffffffffffffff62c8758726b99a9885",
            "0000000000000000000000000000000000000039569350230507fbd1d71113b1",
            "000000000000000000000000000000000000000000039aeded48a3857e640b9b",
            "0000000000000000000000000000000000000000000000000000000000013c57"
        );
        let obs = decode_swap_obs(data, 100, 1).unwrap();
        let inv = obs.inverted();
        assert_eq!(inv.tick, -obs.tick);
        assert_eq!(inv.amount0, obs.amount1);
        assert_eq!(inv.amount1, obs.amount0);
        // Inverted price is the reciprocal of the original.
        assert!((inv.price_raw() * obs.price_raw() - 1.0).abs() < 1e-6);
        // Double inversion restores the original price.
        assert!((inv.inverted().price_raw() - obs.price_raw()).abs() / obs.price_raw() < 1e-9);
    }

    #[test]
    fn hedged_wide_is_low_churn_low_variance() {
        let cfg = cfg();
        let exec = ExecConfig::default();
        // Direction-balanced base so the hedge has beta variance to remove.
        let mut base = scenario_swaps(Scenario::Pump, 80_000, 700, 4_000, 1e18, 1e24);
        let crash = scenario_swaps(Scenario::Crash, 80_000, 700, 4_000, 1e18, 1e24);
        base.extend(crash);
        for (i, s) in base.iter_mut().enumerate() {
            s.block = 1000 + i as u64;
        }
        let report = multi_path_eval(
            &base, &cfg, &exec, 300, 6000, 1.5, 1.0, 6.0, 80, 50, 7, false,
        );
        let pick = |name: &str| {
            report
                .policies
                .iter()
                .find(|p| p.policy == name)
                .unwrap()
                .clone()
        };
        let hedged_wide = pick("hedged_wide");
        let passive_wide = pick("passive_wide");
        // Wrapping the wide band in a delta hedge keeps its low churn but cuts the
        // directional variance.
        assert!(
            hedged_wide.std_net_usd < passive_wide.std_net_usd,
            "hedged_wide std {} should be below passive_wide {}",
            hedged_wide.std_net_usd,
            passive_wide.std_net_usd
        );
    }

    #[test]
    fn multi_path_delta_hedge_reduces_variance() {
        let cfg = cfg();
        let exec = ExecConfig::default();
        // Base with directional blocks in both directions (pump then crash) so that
        // bootstrap reorderings produce paths with varied net direction — that is
        // the beta variance a delta hedge is meant to remove.
        let mut base = scenario_swaps(Scenario::Pump, 80_000, 800, 4_000, 1e18, 1e24);
        let crash = scenario_swaps(Scenario::Crash, 80_000, 800, 4_000, 1e18, 1e24);
        base.extend(crash);
        for (i, s) in base.iter_mut().enumerate() {
            s.block = 1000 + i as u64;
        }
        let report = multi_path_eval(
            &base, &cfg, &exec, 300, 6000, 1.5, 1.0, 6.0, 80, 50, 42, false,
        );
        let pick = |name: &str| {
            report
                .policies
                .iter()
                .find(|p| p.policy == name)
                .unwrap()
                .clone()
        };
        let delta = pick("delta_hedged");
        let unhedged = pick("narrow_rebalance");
        // The whole point: hedging collapses the directional variance of net PnL.
        assert!(
            delta.std_net_usd < unhedged.std_net_usd,
            "delta-hedged net std {} should be below unhedged {}",
            delta.std_net_usd,
            unhedged.std_net_usd
        );
        assert_eq!(report.n_paths, 80);
    }

    #[test]
    fn dynamic_delta_hedge_offsets_crash_beta() {
        let cfg = cfg();
        let exec = ExecConfig {
            action_delay_blocks: 0,
            block_seconds: 2.0,
            funding_bps_per_day: 0.0, // isolate the hedge mechanism
            risk_asset_is_token1: true,
        };
        // A crash (AERO falls, tick rises): the unhedged LP accumulates the falling
        // asset; a dynamic short hedge profits and offsets that beta.
        let swaps = scenario_swaps(Scenario::Crash, 80_000, 1_200, 6_000, 1e18, 1e24);
        let reports = run_baseline_battery_with(&swaps, &cfg, &exec, 100, 6000, 1.5, 1.0, 6.0);
        let pick = |name: &str| reports.iter().find(|r| r.policy == name).unwrap().clone();
        let delta = pick("delta_hedged");
        let unhedged = pick("narrow_rebalance");
        assert!(
            delta.hedge_pnl_usd > 0.0,
            "dynamic short hedge should profit in a crash, got {}",
            delta.hedge_pnl_usd
        );
        assert!(
            delta.net_pnl_usd > unhedged.net_pnl_usd,
            "delta-hedged {} should beat unhedged narrow {} in a crash",
            delta.net_pnl_usd,
            unhedged.net_pnl_usd
        );
        assert!(
            delta.max_drawdown_usd < unhedged.max_drawdown_usd,
            "hedge should cap drawdown in a crash"
        );
    }

    #[test]
    fn delta_trend_stop_stands_down_inside_fast_trend() {
        let cfg = cfg();
        let exec = ExecConfig {
            action_delay_blocks: 0,
            block_seconds: 2.0,
            funding_bps_per_day: 0.0,
            risk_asset_is_token1: true,
        };
        let mut stream = scenario_swaps(Scenario::Calm, 80_000, 80, 600, 1e18, 1e24);
        let trend = scenario_swaps(Scenario::Pump, 80_000, 240, 6_000, 1e18, 1e24);
        stream.extend(trend);
        for (i, s) in stream.iter_mut().enumerate() {
            s.block = 1000 + i as u64;
        }

        let delta = run_policy(
            &stream,
            &cfg,
            &exec,
            RangeMode::DeltaHedged {
                half_width_ticks: 100,
                rehedge_band: 0.25,
            },
            "delta_hedged",
        );
        let stop = run_policy(
            &stream,
            &cfg,
            &exec,
            RangeMode::DeltaTrendStop {
                half_width_ticks: 100,
                rehedge_band: 0.25,
                enter_threshold: 1.5,
                exit_threshold: 0.75,
                window: 15,
            },
            "delta_trend_stop",
        );

        assert!(
            stop.swaps_in_range < delta.swaps_in_range,
            "trend stop should stand down inside a fast trend ({} vs {})",
            stop.swaps_in_range,
            delta.swaps_in_range
        );
        assert!(stop.rebalances > 0, "trend stop should actively exit");
        assert!(
            stop.max_drawdown_usd < delta.max_drawdown_usd,
            "trend stop drawdown {} should be below delta hedge {}",
            stop.max_drawdown_usd,
            delta.max_drawdown_usd
        );
    }

    #[test]
    fn lvr_and_rewards_attribution() {
        let mut cfg = cfg();
        cfg.reward_apr = 0.20; // 20% reward APR
        let exec = ExecConfig::default();

        // Flat price: no arbitrage moves -> ~zero LVR, fee_minus_lvr ~ fees.
        let sqrt = sqrt_ratio_at_tick(80_000) * TWO_POW_96;
        let flat: Vec<SwapObs> = (0..500)
            .map(|i| SwapObs {
                block: 1000 + i * 12, // ~24s apart at 2s/block
                log_index: 0,
                amount0: 1e18,
                amount1: -3000e18,
                sqrt_price_x96: sqrt,
                liquidity: 1e24,
                tick: 80_000,
            })
            .collect();
        let r = run_policy(
            &flat,
            &cfg,
            &exec,
            RangeMode::CenteredRebalance {
                half_width_ticks: 600,
                rebalance: false,
            },
            "narrow",
        );
        assert!(
            r.lvr_usd < 1.0,
            "flat price -> ~zero LVR, got {}",
            r.lvr_usd
        );
        assert!(
            (r.fee_minus_lvr_usd - r.fee_income_usd).abs() < 1.0,
            "flat: fee-LVR ~= fees"
        );
        assert!(
            r.reward_income_usd > 0.0,
            "reward APR should accrue in range"
        );

        // Trending price: real arbitrage -> positive LVR below fee income shape.
        let trend: Vec<SwapObs> = (0..500)
            .map(|i| {
                let tick = 80_000 + (i as i32) * 3;
                SwapObs {
                    block: 1000 + i * 12,
                    log_index: 0,
                    amount0: 1e18,
                    amount1: -3000e18,
                    sqrt_price_x96: sqrt_ratio_at_tick(tick) * TWO_POW_96,
                    liquidity: 1e24,
                    tick,
                }
            })
            .collect();
        let rt = run_policy(
            &trend,
            &cfg,
            &exec,
            RangeMode::StaticWide {
                half_width_ticks: 6000,
            },
            "wide",
        );
        assert!(rt.lvr_usd > 0.0, "trending price should bleed LVR");
    }

    #[test]
    fn regime_gate_stands_down_in_trend() {
        let cfg = cfg();
        let exec = ExecConfig {
            action_delay_blocks: 3,
            block_seconds: 2.0,
            funding_bps_per_day: 0.0,
            risk_asset_is_token1: true,
        };
        // Calm fees, then a crash. The gate should LP through the calm part and
        // stand down to money during the crash, beating a static narrow LP and
        // capping drawdown.
        let mut stream = scenario_swaps(Scenario::Calm, 80_000, 600, 6_000, 1e18, 1e24);
        let crash = scenario_swaps(Scenario::Crash, 80_000, 900, 6_000, 1e18, 1e24);
        stream.extend(crash);
        for (i, s) in stream.iter_mut().enumerate() {
            s.block = 1000 + i as u64;
        }
        let reports = run_baseline_battery_with(&stream, &cfg, &exec, 300, 6000, 1.5, 1.0, 2.0);
        let pick = |name: &str| reports.iter().find(|r| r.policy == name).unwrap().clone();
        let gated = pick("regime_gated");
        let narrow_static = pick("narrow_static");
        assert!(
            gated.net_pnl_usd > narrow_static.net_pnl_usd,
            "gate {} should beat static LP {} across calm->crash",
            gated.net_pnl_usd,
            narrow_static.net_pnl_usd
        );
        assert!(
            gated.max_drawdown_usd < narrow_static.max_drawdown_usd,
            "gate should cap drawdown vs static LP"
        );
        // It should both earn some fees (calm part) and act (stand down in crash).
        assert!(gated.fee_income_usd > 0.0 && gated.rebalances > 0);
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
