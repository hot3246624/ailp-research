#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");

function usage() {
  console.error(`usage: node scripts/meteora-dlmm-join-flow-snapshots.cjs --flow <swap-flow.jsonl> --snapshots <dlmm-bin-snapshots.jsonl> --out <obs.jsonl> [--raw-out <join-report.json>] [--max-slot-distance 1200] [--active-bin-source flow-price|snapshot]`);
}

function parseArgs(argv) {
  const out = {
    maxSlotDistance: 1_200,
    activeBinSource: "flow-price",
  };
  for (let i = 2; i < argv.length; i += 1) {
    const key = argv[i];
    const value = argv[i + 1];
    if (!key.startsWith("--") || value === undefined) {
      usage();
      process.exit(2);
    }
    i += 1;
    switch (key) {
      case "--flow":
        out.flow = value;
        break;
      case "--snapshots":
        out.snapshots = value;
        break;
      case "--out":
        out.out = value;
        break;
      case "--raw-out":
        out.rawOut = value;
        break;
      case "--max-slot-distance":
        out.maxSlotDistance = Number(value);
        break;
      case "--active-bin-source":
        out.activeBinSource = value;
        break;
      default:
        console.error(`unknown arg: ${key}`);
        usage();
        process.exit(2);
    }
  }
  if (!out.flow || !out.snapshots || !out.out) {
    usage();
    process.exit(2);
  }
  if (!["flow-price", "snapshot"].includes(out.activeBinSource)) {
    throw new Error("--active-bin-source must be flow-price or snapshot");
  }
  return out;
}

function readJsonl(file) {
  const rows = [];
  for (const [index, line] of fs.readFileSync(file, "utf8").split(/\r?\n/).entries()) {
    if (!line.trim()) {
      continue;
    }
    try {
      rows.push(JSON.parse(line));
    } catch (err) {
      throw new Error(`failed to parse ${file} line ${index + 1}: ${err.message}`);
    }
  }
  return rows;
}

function nearestSnapshot(snapshots, block) {
  let best = null;
  let bestDistance = Number.POSITIVE_INFINITY;
  for (const snapshot of snapshots) {
    const distance = Math.abs(Number(snapshot.block) - Number(block));
    if (distance < bestDistance) {
      best = snapshot;
      bestDistance = distance;
    }
  }
  return best ? { snapshot: best, distance: bestDistance } : null;
}

function ensureDir(file) {
  const dir = path.dirname(file);
  if (dir && dir !== ".") {
    fs.mkdirSync(dir, { recursive: true });
  }
}

function main() {
  const args = parseArgs(process.argv);
  const flowRows = readJsonl(args.flow)
    .filter((row) => row.source === "meteora-dlmm-swap-flow")
    .sort((left, right) => Number(left.block) - Number(right.block));
  const snapshots = readJsonl(args.snapshots)
    .filter((row) => Number.isFinite(Number(row.block)))
    .sort((left, right) => Number(left.block) - Number(right.block));
  if (snapshots.length === 0) {
    throw new Error(`no usable snapshots in ${args.snapshots}`);
  }

  const joined = [];
  const skipped = {
    no_amount: 0,
    no_flow_bin: 0,
    stale_snapshot: 0,
  };
  for (const flow of flowRows) {
    const amountInUsd = Number(flow.amount_in_usd);
    if (!Number.isFinite(amountInUsd) || amountInUsd <= 0) {
      skipped.no_amount += 1;
      continue;
    }
    const match = nearestSnapshot(snapshots, flow.block);
    if (!match || match.distance > args.maxSlotDistance) {
      skipped.stale_snapshot += 1;
      continue;
    }
    let activeBinId = Number(match.snapshot.active_bin_id);
    if (args.activeBinSource === "flow-price") {
      activeBinId = Number(flow.estimated_bin_id_from_avg_price);
      if (!Number.isFinite(activeBinId)) {
        skipped.no_flow_bin += 1;
        continue;
      }
    }
    const activeLiquidityUsd = Number(match.snapshot.active_liquidity_usd);
    if (!Number.isFinite(activeLiquidityUsd) || activeLiquidityUsd <= 0) {
      skipped.stale_snapshot += 1;
      continue;
    }
    const obs = {
      block: Number(flow.block),
      active_bin_id: activeBinId,
      active_liquidity_usd: activeLiquidityUsd,
      amount_in_usd: amountInUsd,
    };
    joined.push({
      obs,
      signature: flow.signature,
      snapshot_block: Number(match.snapshot.block),
      snapshot_active_bin_id: Number(match.snapshot.active_bin_id),
      slot_distance: match.distance,
      input_symbol: flow.input_symbol,
      output_symbol: flow.output_symbol,
    });
  }

  ensureDir(args.out);
  fs.writeFileSync(args.out, joined.map((row) => JSON.stringify(row.obs)).join("\n") + (joined.length ? "\n" : ""));
  const distances = joined.map((row) => row.slot_distance);
  const summary = {
    flow_rows: flowRows.length,
    snapshots: snapshots.length,
    joined_rows: joined.length,
    skipped,
    max_slot_distance: args.maxSlotDistance,
    active_bin_source: args.activeBinSource,
    observed_slot_distance_max: distances.length ? Math.max(...distances) : null,
    observed_slot_distance_avg: distances.length
      ? distances.reduce((sum, value) => sum + value, 0) / distances.length
      : null,
    out: args.out,
    caveat: "proxy-only: amount_in_usd is real non-overlapping flow, but active_liquidity_usd comes from nearest active-bin snapshot rather than historical bin-array state",
  };
  if (args.rawOut) {
    ensureDir(args.rawOut);
    fs.writeFileSync(args.rawOut, `${JSON.stringify({ summary, joined }, null, 2)}\n`);
  }
  console.log(JSON.stringify(summary));
}

main();
