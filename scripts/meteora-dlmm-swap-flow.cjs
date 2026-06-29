#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");

const METEORA_PROGRAM_ID = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo";
const DEFAULT_RPC = process.env.SOLANA_RPC_URL || "https://solana-rpc.publicnode.com";
const HTTP_TIMEOUT_MS = 20_000;
const STABLE_MINTS = new Set([
  "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", // USDC
  "Es9vMFrzaCERmJfrF4H2FYD4KCo4wM5oZ1oF8QdA6vW", // USDT
]);

function usage() {
  console.error(`usage: node scripts/meteora-dlmm-swap-flow.cjs --spec <spec.json> --out <flow.jsonl> [--append] [--raw-out <sample.json>] [--rpc <url>] [--limit 50] [--signature-scan-limit 200] [--max-signature-pages 1] [--before-signature <sig>] [--request-sleep-ms 100]`);
}

function parseArgs(argv) {
  const out = {
    append: false,
    rpc: DEFAULT_RPC,
    limit: 50,
    signatureScanLimit: 200,
    maxSignaturePages: 1,
    requestSleepMs: 100,
  };
  for (let i = 2; i < argv.length; i += 1) {
    const key = argv[i];
    if (!key.startsWith("--")) {
      usage();
      process.exit(2);
    }
    if (key === "--append") {
      out.append = true;
      continue;
    }
    const value = argv[i + 1];
    if (value === undefined) {
      usage();
      process.exit(2);
    }
    i += 1;
    switch (key) {
      case "--spec":
        out.spec = value;
        break;
      case "--out":
        out.out = value;
        break;
      case "--raw-out":
        out.rawOut = value;
        break;
      case "--rpc":
        out.rpc = value;
        break;
      case "--limit":
        out.limit = Number(value);
        break;
      case "--signature-scan-limit":
        out.signatureScanLimit = Number(value);
        break;
      case "--max-signature-pages":
        out.maxSignaturePages = Number(value);
        break;
      case "--before-signature":
        out.beforeSignature = value;
        break;
      case "--request-sleep-ms":
        out.requestSleepMs = Number(value);
        break;
      default:
        console.error(`unknown arg: ${key}`);
        usage();
        process.exit(2);
    }
  }
  if (!out.spec || !out.out) {
    usage();
    process.exit(2);
  }
  return out;
}

function requireSdk(name) {
  try {
    return require(name);
  } catch (err) {
    console.error(`missing ${name}; run scripts/meteora-dlmm-swap-flow.sh so the official SDK is installed in a temp directory`);
    throw err;
  }
}

function decimalRawToNumber(raw, decimals) {
  const sign = raw < 0n ? -1 : 1;
  const abs = raw < 0n ? -raw : raw;
  const s = abs.toString(10);
  if (decimals === 0) {
    return sign * Number(s);
  }
  const padded = s.padStart(decimals + 1, "0");
  const whole = padded.slice(0, -decimals);
  const frac = padded.slice(-decimals).replace(/0+$/, "");
  return sign * Number(frac ? `${whole}.${frac}` : whole);
}

function tokenAmountRaw(balance) {
  return BigInt(balance?.uiTokenAmount?.amount ?? "0");
}

function sumPoolTokenBalances(tx, field, poolAddress, mint) {
  let sum = 0n;
  let decimals = null;
  for (const balance of tx?.meta?.[field] || []) {
    if (balance.owner !== poolAddress || balance.mint !== mint) {
      continue;
    }
    sum += tokenAmountRaw(balance);
    decimals = Number(balance.uiTokenAmount?.decimals ?? decimals);
  }
  return { sum, decimals };
}

function poolTokenDelta(tx, poolAddress, token) {
  const pre = sumPoolTokenBalances(tx, "preTokenBalances", poolAddress, token.address);
  const post = sumPoolTokenBalances(tx, "postTokenBalances", poolAddress, token.address);
  const decimals = post.decimals ?? pre.decimals ?? Number(token.decimals);
  const raw = post.sum - pre.sum;
  return {
    raw,
    ui: decimalRawToNumber(raw, decimals),
    decimals,
  };
}

async function rpcCall(rpc, method, params) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), HTTP_TIMEOUT_MS);
  try {
    const response = await fetch(rpc, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
      signal: controller.signal,
    });
    if (!response.ok) {
      throw new Error(`${method} HTTP ${response.status}`);
    }
    const json = await response.json();
    if (json.error) {
      throw new Error(`${method} RPC error: ${JSON.stringify(json.error)}`);
    }
    return json.result;
  } finally {
    clearTimeout(timeout);
  }
}

async function sleep(ms) {
  if (ms > 0) {
    await new Promise((resolve) => setTimeout(resolve, ms));
  }
}

async function fetchJson(url) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), HTTP_TIMEOUT_MS);
  try {
    const res = await fetch(url, { signal: controller.signal });
    if (!res.ok) {
      throw new Error(`${url} returned ${res.status}`);
    }
    return res.json();
  } finally {
    clearTimeout(timeout);
  }
}

function flattenInstructions(tx) {
  const out = [];
  for (const ix of tx?.transaction?.message?.instructions || []) {
    out.push(ix);
  }
  for (const group of tx?.meta?.innerInstructions || []) {
    for (const ix of group.instructions || []) {
      out.push(ix);
    }
  }
  return out;
}

function decodedMeteoraSwapInstructions(program, tx, poolAddress) {
  return flattenInstructions(tx)
    .filter((ix) => ix.programId === METEORA_PROGRAM_ID && Array.isArray(ix.accounts))
    .filter((ix) => ix.accounts.includes(poolAddress))
    .map((ix) => {
      let decoded = null;
      try {
        decoded = program.coder.instruction.decode(ix.data, "base58");
      } catch (_err) {
        decoded = null;
      }
      return { ix, decoded };
    })
    .filter((row) => row.decoded?.name?.toLowerCase().startsWith("swap"));
}

function existingJsonlKeys(file, key) {
  const keys = new Set();
  if (!fs.existsSync(file)) {
    return keys;
  }
  for (const line of fs.readFileSync(file, "utf8").split(/\r?\n/)) {
    if (!line.trim()) {
      continue;
    }
    try {
      const parsed = JSON.parse(line);
      if (parsed[key] !== undefined) {
        keys.add(parsed[key]);
      }
    } catch (_err) {
      // The consumer will surface malformed JSONL; keep append mode best-effort.
    }
  }
  return keys;
}

function writeJsonlRows(file, rows, append, key) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const seen = append ? existingJsonlKeys(file, key) : new Set();
  const fresh = rows.filter((row) => !seen.has(row[key]));
  const body = fresh.map((row) => JSON.stringify(row)).join("\n");
  if (!append) {
    fs.writeFileSync(file, body ? `${body}\n` : "");
  } else if (body) {
    fs.appendFileSync(file, `${body}\n`);
  }
  return { written: fresh.length, skipped_duplicates: rows.length - fresh.length };
}

function usdNotional(token0, token1, delta0, delta1, poolMeta) {
  const stable0 = STABLE_MINTS.has(token0.address);
  const stable1 = STABLE_MINTS.has(token1.address);
  if (stable0) {
    return Math.abs(delta0.ui);
  }
  if (stable1) {
    return Math.abs(delta1.ui);
  }
  const token0Price = Number(poolMeta.token_x?.price ?? 0);
  const token1Price = Number(poolMeta.token_y?.price ?? 0);
  const in0 = delta0.raw > 0n;
  if (in0 && token0Price > 0) {
    return Math.abs(delta0.ui) * token0Price;
  }
  if (!in0 && token1Price > 0) {
    return Math.abs(delta1.ui) * token1Price;
  }
  return null;
}

function estimateBinFromAveragePrice(priceToken1PerToken0, token0, token1, binStep) {
  if (!Number.isFinite(priceToken1PerToken0) || priceToken1PerToken0 <= 0 || !binStep) {
    return null;
  }
  const decimalScale = 10 ** (Number(token1.decimals) - Number(token0.decimals));
  const rawPrice = priceToken1PerToken0 * decimalScale;
  const base = 1 + Number(binStep) / 10_000;
  return Math.round(Math.log(rawPrice) / Math.log(base));
}

function flowRow({ tx, signature, spec, poolMeta, program }) {
  const poolAddress = spec.pool_address;
  const swapInstructions = decodedMeteoraSwapInstructions(program, tx, poolAddress);
  if (swapInstructions.length === 0) {
    return null;
  }
  const token0 = spec.token0;
  const token1 = spec.token1;
  const delta0 = poolTokenDelta(tx, poolAddress, token0);
  const delta1 = poolTokenDelta(tx, poolAddress, token1);
  if (delta0.raw === 0n || delta1.raw === 0n || (delta0.raw > 0n) === (delta1.raw > 0n)) {
    return null;
  }
  const token0In = delta0.raw > 0n;
  const token1In = delta1.raw > 0n;
  const amountInUsd = usdNotional(token0, token1, delta0, delta1, poolMeta);
  const avgPrice = Math.abs(delta1.ui) > 0 && Math.abs(delta0.ui) > 0
    ? Math.abs(delta1.ui) / Math.abs(delta0.ui)
    : null;
  const input = token0In ? token0 : token1;
  const output = token0In ? token1 : token0;
  return {
    source: "meteora-dlmm-swap-flow",
    signature,
    block: tx.slot,
    block_time: tx.blockTime ?? null,
    pool_address: poolAddress,
    symbol: spec.symbol,
    swap_ix_count: swapInstructions.length,
    swap_ix_names: [...new Set(swapInstructions.map((row) => row.decoded.name))],
    token0_mint: token0.address,
    token1_mint: token1.address,
    token0_symbol: token0.symbol,
    token1_symbol: token1.symbol,
    token0_pool_delta_raw: delta0.raw.toString(),
    token1_pool_delta_raw: delta1.raw.toString(),
    token0_pool_delta_ui: delta0.ui,
    token1_pool_delta_ui: delta1.ui,
    token0_in: token0In,
    token1_in: token1In,
    input_symbol: input.symbol,
    output_symbol: output.symbol,
    amount_in_usd: amountInUsd,
    avg_price_token1_per_token0: avgPrice,
    estimated_bin_id_from_avg_price: estimateBinFromAveragePrice(
      avgPrice,
      token0,
      token1,
      spec.bin_step,
    ),
    caveat: "flow row has non-overlapping reserve deltas but no historical active-bin liquidity",
  };
}

async function main() {
  const args = parseArgs(process.argv);
  const { Connection } = requireSdk("@solana/web3.js");
  const { createProgram } = requireSdk("@meteora-ag/dlmm");
  const spec = JSON.parse(fs.readFileSync(args.spec, "utf8"));
  if (spec.replay_model !== "dlmm_bin_replay") {
    throw new Error(`spec replay_model must be dlmm_bin_replay, got ${spec.replay_model}`);
  }
  const poolMeta = await fetchJson(`https://dlmm.datapi.meteora.ag/pools/${spec.pool_address}`);
  const program = createProgram(new Connection(args.rpc, "confirmed"), { cluster: "mainnet-beta" });

  let cursor = args.beforeSignature || null;
  let scanned = 0;
  let txErrors = 0;
  let decodedSwapTxs = 0;
  const rows = [];
  for (let page = 0; page < Math.max(1, args.maxSignaturePages); page += 1) {
    const config = { limit: Math.min(1_000, args.signatureScanLimit) };
    if (cursor) {
      config.before = cursor;
    }
    const signatures = await rpcCall(args.rpc, "getSignaturesForAddress", [
      spec.pool_address,
      config,
    ]);
    if (!signatures.length) {
      break;
    }
    cursor = signatures[signatures.length - 1].signature;
    for (const sig of signatures.filter((sig) => sig.err == null)) {
      scanned += 1;
      let tx = null;
      try {
        tx = await rpcCall(args.rpc, "getTransaction", [
          sig.signature,
          { encoding: "jsonParsed", maxSupportedTransactionVersion: 0 },
        ]);
      } catch (_err) {
        txErrors += 1;
        continue;
      }
      const row = flowRow({ tx, signature: sig.signature, spec, poolMeta, program });
      if (row) {
        decodedSwapTxs += 1;
        rows.push(row);
      }
      if (rows.length >= args.limit) {
        break;
      }
      await sleep(args.requestSleepMs);
    }
    if (rows.length >= args.limit) {
      break;
    }
  }

  rows.sort((left, right) => left.block - right.block || left.signature.localeCompare(right.signature));
  const write = writeJsonlRows(args.out, rows, args.append, "signature");
  const summary = {
    pool: spec.pool_address,
    symbol: spec.symbol,
    program_id: METEORA_PROGRAM_ID,
    scanned_signatures: scanned,
    decoded_swap_txs: decodedSwapTxs,
    rows: rows.length,
    written: write.written,
    skipped_duplicates: write.skipped_duplicates,
    tx_errors: txErrors,
    next_before_signature: cursor,
    out: args.out,
    raw_out: args.rawOut || null,
    caveat: "active-bin id/liquidity are not reconstructed here; join this flow with SDK snapshots or archival account state before replay APR",
  };
  if (args.rawOut) {
    fs.mkdirSync(path.dirname(args.rawOut), { recursive: true });
    fs.writeFileSync(args.rawOut, `${JSON.stringify({ summary, rows }, null, 2)}\n`);
  }
  console.log(JSON.stringify(summary));
}

main().catch((err) => {
  console.error(err && err.stack ? err.stack : err);
  process.exit(1);
});
