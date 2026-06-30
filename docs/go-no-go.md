# Go / No-Go: Is autoLP Deployable at Our Capital Scale?

Snapshot: 2026-06-30 CST. Decision document built only from existing evidence
(no new experiments). It answers one question: **given everything we have learned,
should we deploy capital into an automated LP strategy now?**

Bottom line up front: **NO-GO for deploying capital now.** The research succeeded —
it produced a validated mechanism *and* a validated negative: at retail capital scale,
edge net of cost and capacity is ≈ 0. Stop spending build cycles on pool-hunting;
shelve with explicit revisit triggers (below).

---

## What we actually proved (the positive)

One mechanism survived the full statistical battery (multi-path moving-block
bootstrap, LVR attribution, demeaned/martingale paths, capacity gates):

- **High fee × LOW volatility** is the only target with positive LP expectancy.
  `fee/vol`, not headline APR, is the metric.
- Best researched candidate, **WETH-USDC 200 bps** (`delta_hedged`, 477-swap window,
  USDC numeraire):

  | policy | raw mean | std | fee−LVR | win% vs hold | meanDD |
  | --- | ---: | ---: | ---: | ---: | ---: |
  | narrow_static | +142 | 103 | +113 | 96–99% | 62–79 |
  | **delta_hedged** | **+88** | **21** | +113 | 61–76% | **~1** |
  | passive_wide | +65 | 114 | +14 | 98–99% | 91–107 |
  | hold_50_50 | +54 | 117 | 0 | — | 96 |

  per **$10k over one ~477-swap window** (≈ several days–1 week of this thin pool's
  flow; exact span not preserved in data).
- The execution machinery is real and tested: Slipstream calldata, funded fork sim
  passes for fresh mint and full rebalance (`swap→mint`, `collect→decreaseLiquidity
  →collect→mint`), real gas reads, read-only position monitor.

That is a genuine, end-to-end-validated, direction-robust, crash-proof LP shape.

## Why it is still a No-Go (the economics)

Three numbers kill it at our scale.

**1. Capacity is structurally tiny.** The +$90/$10k figure is unreachable, because
$10k is *rejected* by the price-impact gate:

| capital | swap impact | gate |
| ---: | ---: | --- |
| $10k | 53.3 bps | **rejected** (>30 bps) |
| $5k | 26.7 bps | passes |
| $3k | 16.0 bps | passes |
| $1k | 5.3 bps | passes |

Deployable size on the *single best pool we found* is **$1–3k**. Scaling the edge
down: ~**+$27 per window at $3k** (raw, best-case).

**2. Net of hedge funding, the mean is ≈ 0.** The delta hedge pays funding
(modeled 10 bps/day on hedge notional). On a $3k position the WETH leg ≈ $1.5k, so
funding ≈ $1.5/day ≈ **$10/week**, against a best-case raw +$27/window. The author's
own honest read on this pool: *"in the driftless world fee-alpha barely covers funding
+ hedge cost, ≈ 0."* The robust, repeatable value of the hedge is **variance/tail
removal** (std $110→$20, drawdown $96→$1), **not mean profit**.

**3. The deep-and-fee-dense pool does not exist in our universe.** The strategy only
*works* (mean≈0, variance-killed) at $1–3k, and only *pays* (+$90/$10k) at $10k where
impact kills it. The entire Solana pivot was the explicit search for a pool with high
fee density AND enough depth AND continuous non-toxic flow. It returned a string of
zeros — CARDS, SPCX, HYPE, MU all came back **deployable APR = 0 / negative**, every
one for the *same structural reason*: high headline APR is compensation for adverse
selection (LVR), and high-fee pools are high-fee precisely because they are thin and
toxic. Deep continuous flow and high fee density are **anti-correlated by
construction** — deep pools attract LP competition that compresses fees.

### The arithmetic of going live anyway

Best-case annualized (raw window read, requires uninterrupted flow):
~+$27/wk − $10/wk funding ≈ +$17/wk × 52 ≈ **~$880/yr on $3k (~29%)**.
Honest-case (driftless mean): **~$0/yr mean**, value = risk reduction only.

Against that, the "Not ready" list still requires: a perp **hedge adapter**
(Hyperliquid/cross-venue), **post-trade accounting ledger**, full **kill-switch**
wiring, **production RPC**, and **reward-liquidation** modeling — then ongoing
operation of an autonomous, cross-venue, key-custody system carrying smart-contract +
bridge + funding + RPC tail risk. **Standing up that stack to earn a best-case ~$880/yr
(honest ~$0 mean) on $1–3k is not justified.**

## The colleague's parallel track confirms it

The Meteora/Raydium memecoin scanner (CARDS → SPCX → HYPE → MU) is methodologically
clean but is **re-confirming a known negative, pool by pool**. Latest MU result: new
flow decoded, but strict-window rows did not grow (still 2 windows / 22 rows), so the
"p05 APR −5930%" is an annualization artifact of a ~77-swap sample, not a usable
number. Each additional memecoin replay has **~0 marginal information** — it tells us
again that thin tail pools fail the gate. That loop should stop.

## What would flip this to a Go

Concrete, falsifiable triggers — revisit only if one becomes true:

1. **Deployable size ≥ ~$50–100k** at the same `fee/vol` edge → mean becomes material.
   Requires a pool with *both* depth and fee density. **Not found in Base or Solana.**
2. **You already want LP inventory for another reason** (you are a market maker /
   protocol with the inventory anyway) → the validated variance/tail-removal primitive
   is worth wrapping around it. Not the case for discretionary $1–3k.
3. **Structurally cheaper or negative hedge funding**, or a pool where *you* are the
   dominant LP capturing most fees (not sharing the tier).
4. **A fee tier > 200 bps that is not a toxic tail pool.** None found.

## Recommendation

1. **Stop the pool-hunting loop** (especially the memecoin strict-window scanner) — it
   is mining a known negative.
2. **Freeze the research**, not the code: the engine, calldata, fork-sim, and monitor
   are reusable assets. Keep them; stop adding pools.
3. **Do not build** the hedge adapter / accounting ledger / live daemon **for this
   strategy** — the EV does not justify the operational surface at $1–3k.
4. **Revisit only on a trigger above.** If the goal is yield on small capital,
   automated LP is the wrong instrument and this is the honest finding the research was
   for.
