# Strategy Research Plan

## Core Claim

The strategy problem is a stochastic control problem under transaction costs. The action is not "choose the highest APR pool"; the action is "choose the position and rebalance policy with the best expected utility under a specific price path and network regime."

## Objective

For each candidate action, estimate:

```text
expected_net_value =
  expected_fee_income
  + expected_reward_income_after_liquidation
  + expected_inventory_pnl
  - expected_impermanent_loss_vs_hold
  - expected_gas_cost
  - expected_swap_slippage
  - expected_mev_loss
  - risk_penalty
```

The policy should maximize expected net value subject to hard risk limits.

## State Features

Pool features:

- current tick and price
- fee tier
- active liquidity
- tick liquidity distribution around price
- volume and volume-to-TVL
- fee growth over recent windows
- price occupancy by tick/range
- realized volatility and jump frequency
- swap imbalance and large-trade pressure

Wallet features:

- current range
- token0/token1 inventory
- uncollected fees
- entry basis
- current value
- pool and token exposure limits

Network features:

- gas cost by action type
- gas volatility
- block time
- RPC health and provider disagreement
- private route availability
- estimated sandwich/MEV exposure
- L2 sequencer health where relevant

## Actions

- hold
- collect fees only
- recenter same width
- widen range
- narrow range
- partially reduce
- fully exit
- swap inventory before minting
- switch capital to another pool

## Baselines

Do not call a strategy good unless it beats:

- hold the inventory
- passive wide range
- fixed narrow range with naive out-of-range rebalance
- volatility-scaled range with gas threshold

Results must be compared after gas, swaps, and inventory mark-to-market.

## Initial Policies

### Opportunistic Volatile Range

Target pools with high APR, high fee tier, strong observed swap density, and acceptable TVL. Examples include WETH-USDC, WETH-AERO, and other active volatile pairs. These pools should be judged by fee density after inventory drift, not by headline APR alone.

### Gas-Aware Volatility Range

Width increases with realized volatility and gas. Rebalance only when expected future fees exceed transaction costs by a configurable margin.

### Fee-Density Range

Estimate expected fees per unit of liquidity for nearby ticks. Place liquidity where expected fee density is high but price occupancy probability remains acceptable.

### Inventory-Aware Range

Penalize actions that leave the wallet with excessive exposure to the weaker or more volatile asset. This avoids treating fees as profit while accumulating adverse inventory.

### Flow-Toxicity Filter

Avoid tight ranges when directional flow suggests the LP is being paid fees to absorb informed flow. Proxy signals include swap imbalance, large-trade price impact, and fee earned per unit of inventory drift.

## Research Loop

1. Build a historical state stream from pool state, swaps, prices, wallet states, and network regime.
2. Replay every policy action-by-action.
3. Attribute PnL into fees, inventory PnL, IL estimate, gas, slippage, rewards, and opportunity cost.
4. Compare against baselines.
5. Promote only policies that survive out-of-sample periods and multiple market regimes.
6. Run promoted policy in dry-run before signing any transaction.

## Common Failure Modes

- Backtest profit comes from token beta, not LP skill.
- APY is reward-heavy and assumes impossible reward liquidation.
- Narrow ranges look good before gas and failed rebalance windows.
- Chain is cheap, but pool depth is too thin for real execution.
- Strategy overfits one volatility regime.
- Simulated rebalances assume a price that would not be available after mempool exposure.
- High APR volatile pools hide adverse inventory selection: the LP can earn fees while accumulating the asset being sold by informed flow.
