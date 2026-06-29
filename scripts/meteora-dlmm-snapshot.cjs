#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");

function usage() {
  console.error(`usage: node scripts/meteora-dlmm-snapshot.cjs --spec <spec.json> --out <obs.jsonl> [--append] [--raw-out <snapshot.json>] [--raw-jsonl-out <snapshots.jsonl>] [--rpc <url>] [--bins-left 8] [--bins-right 8] [--volume-window 30m]`);
}

function parseArgs(argv) {
  const out = {
    append: false,
    binsLeft: 8,
    binsRight: 8,
    volumeWindow: "30m",
    rpc: process.env.SOLANA_RPC_URL || "https://api.mainnet-beta.solana.com",
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
      case "--raw-jsonl-out":
        out.rawJsonlOut = value;
        break;
      case "--rpc":
        out.rpc = value;
        break;
      case "--bins-left":
        out.binsLeft = Number(value);
        break;
      case "--bins-right":
        out.binsRight = Number(value);
        break;
      case "--volume-window":
        out.volumeWindow = value;
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

function jsonlStatus(file, dedupeKey, dedupeValue) {
  if (!fs.existsSync(file)) {
    return { rows: 0, duplicate: false };
  }
  const lines = fs.readFileSync(file, "utf8").split(/\r?\n/);
  let rows = 0;
  let duplicate = false;
  for (const line of lines) {
    if (!line.trim()) {
      continue;
    }
    rows += 1;
    if (!duplicate) {
      try {
        const parsed = JSON.parse(line);
        duplicate = parsed[dedupeKey] === dedupeValue;
      } catch (_err) {
        // Keep counting usable-looking lines; parse errors should be caught by the
        // Rust replay reader when the stream is consumed.
      }
    }
  }
  return { rows, duplicate };
}

function writeJsonl(file, value, options) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  if (!options.append) {
    fs.writeFileSync(file, `${JSON.stringify(value)}\n`);
    return { existing_rows: 0, appended: true, duplicate: false };
  }
  const status = jsonlStatus(file, options.dedupeKey, value[options.dedupeKey]);
  if (status.duplicate) {
    return { existing_rows: status.rows, appended: false, duplicate: true };
  }
  fs.appendFileSync(file, `${JSON.stringify(value)}\n`);
  return { existing_rows: status.rows, appended: true, duplicate: false };
}

function requireSdk(name) {
  try {
    return require(name);
  } catch (err) {
    console.error(`missing ${name}; run scripts/meteora-dlmm-snapshot.sh so the official SDK is installed in a temp directory`);
    throw err;
  }
}

function decimalRawToNumber(raw, decimals) {
  const s = raw.toString(10);
  if (decimals === 0) {
    return Number(s);
  }
  const padded = s.padStart(decimals + 1, "0");
  const whole = padded.slice(0, -decimals);
  const frac = padded.slice(-decimals).replace(/0+$/, "");
  return Number(frac ? `${whole}.${frac}` : whole);
}

function binToPlain(bin, tokenX, tokenY) {
  const xRaw = bin.xAmount.toString(10);
  const yRaw = bin.yAmount.toString(10);
  const xAmount = decimalRawToNumber(bin.xAmount, tokenX.decimals);
  const yAmount = decimalRawToNumber(bin.yAmount, tokenY.decimals);
  const xUsd = xAmount * tokenX.price;
  const yUsd = yAmount * tokenY.price;
  return {
    bin_id: bin.binId,
    price: Number(bin.price),
    price_per_token: Number(bin.pricePerToken),
    x_raw: xRaw,
    y_raw: yRaw,
    x_amount: xAmount,
    y_amount: yAmount,
    liquidity_usd: xUsd + yUsd,
    supply_raw: bin.supply.toString(10),
  };
}

async function fetchJson(url) {
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(`${url} returned ${res.status}`);
  }
  return res.json();
}

async function main() {
  const args = parseArgs(process.argv);
  const { Connection, PublicKey } = requireSdk("@solana/web3.js");
  const dlmmModule = requireSdk("@meteora-ag/dlmm");
  const DLMM = dlmmModule.default || dlmmModule;

  const spec = JSON.parse(fs.readFileSync(args.spec, "utf8"));
  if (spec.replay_model !== "dlmm_bin_replay") {
    throw new Error(`spec replay_model must be dlmm_bin_replay, got ${spec.replay_model}`);
  }

  const poolAddress = spec.pool_address;
  const poolMeta = await fetchJson(`https://dlmm.datapi.meteora.ag/pools/${poolAddress}`);
  const tokenX = {
    symbol: poolMeta.token_x?.symbol || spec.token0.symbol,
    decimals: Number(poolMeta.token_x?.decimals ?? spec.token0.decimals),
    price: Number(poolMeta.token_x?.price ?? 0),
  };
  const tokenY = {
    symbol: poolMeta.token_y?.symbol || spec.token1.symbol,
    decimals: Number(poolMeta.token_y?.decimals ?? spec.token1.decimals),
    price: Number(poolMeta.token_y?.price ?? 0),
  };
  if (!Number.isFinite(tokenX.price) || !Number.isFinite(tokenY.price) || tokenX.price <= 0 || tokenY.price <= 0) {
    throw new Error("pool metadata did not include usable token USD prices");
  }

  const connection = new Connection(args.rpc, "confirmed");
  const pool = await DLMM.create(connection, new PublicKey(poolAddress));
  const slot = await connection.getSlot("confirmed");
  const activeBin = await pool.getActiveBin();
  const around = await pool.getBinsAroundActiveBin(args.binsLeft, args.binsRight);
  const bins = around.bins.map((bin) => binToPlain(bin, tokenX, tokenY));
  const active = binToPlain(activeBin, tokenX, tokenY);
  const volume = Number(poolMeta.volume?.[args.volumeWindow] ?? poolMeta.volume?.["30m"] ?? 0);

  const obs = {
    block: slot,
    active_bin_id: activeBin.binId,
    active_liquidity_usd: active.liquidity_usd,
    amount_in_usd: volume,
  };
  const obsWrite = writeJsonl(args.out, obs, {
    append: args.append,
    dedupeKey: "block",
  });

  const snapshot = {
    source: "meteora-dlmm-sdk",
    sdk_package: "@meteora-ag/dlmm@1.9.10",
    rpc: args.rpc.replace(/(api-key=|apikey=|key=)[^&]+/gi, "$1<redacted>"),
    spec: args.spec,
    pool_address: poolAddress,
    symbol: spec.symbol,
    slot,
    bin_step: Number(pool.lbPair.binStep),
    data_api: {
      tvl_usd: Number(poolMeta.tvl),
      current_price: Number(poolMeta.current_price),
      dynamic_fee_pct: Number(poolMeta.dynamic_fee_pct ?? 0),
      base_fee_pct: Number(poolMeta.pool_config?.base_fee_pct ?? 0),
      volume_window: args.volumeWindow,
      volume_usd: volume,
      fees_24h_usd: Number(poolMeta.fees?.["24h"] ?? 0),
      volume_24h_usd: Number(poolMeta.volume?.["24h"] ?? 0),
    },
    token_x: tokenX,
    token_y: tokenY,
    active_bin: active,
    bins,
    normalized_observation: obs,
  };
  if (args.rawOut) {
    fs.mkdirSync(path.dirname(args.rawOut), { recursive: true });
    fs.writeFileSync(args.rawOut, `${JSON.stringify(snapshot, null, 2)}\n`);
  }
  const rawJsonlWrite = args.rawJsonlOut
    ? writeJsonl(args.rawJsonlOut, snapshot, {
        append: args.append,
        dedupeKey: "slot",
      })
    : null;
  console.log(JSON.stringify({
    pool: poolAddress,
    symbol: spec.symbol,
    slot,
    active_bin_id: obs.active_bin_id,
    active_liquidity_usd: obs.active_liquidity_usd,
    amount_in_usd: obs.amount_in_usd,
    out: args.out,
    append: args.append,
    appended: obsWrite.appended,
    duplicate: obsWrite.duplicate,
    existing_rows: obsWrite.existing_rows,
    raw_out: args.rawOut || null,
    raw_jsonl_out: args.rawJsonlOut || null,
    raw_jsonl_appended: rawJsonlWrite ? rawJsonlWrite.appended : null,
  }));
}

main().catch((err) => {
  console.error(err && err.stack ? err.stack : err);
  process.exit(1);
});
