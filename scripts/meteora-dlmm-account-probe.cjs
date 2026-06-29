#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");

const DEFAULT_RPC = process.env.SOLANA_RPC_URL || "https://solana-rpc.publicnode.com";
const HTTP_TIMEOUT_MS = 20_000;

function usage() {
  console.error(
    "usage: node scripts/meteora-dlmm-account-probe.cjs --spec <spec.json> --flow <swap-flow.jsonl> [--raw-out <report.json>] [--rpc <url>] [--limit 5] [--request-sleep-ms 100]",
  );
}

function parseArgs(argv) {
  const out = {
    rpc: DEFAULT_RPC,
    limit: 5,
    requestSleepMs: 100,
  };
  for (let i = 2; i < argv.length; i += 1) {
    const key = argv[i];
    if (!key.startsWith("--")) {
      usage();
      process.exit(2);
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
      case "--flow":
        out.flow = value;
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
      case "--request-sleep-ms":
        out.requestSleepMs = Number(value);
        break;
      default:
        console.error(`unknown arg: ${key}`);
        usage();
        process.exit(2);
    }
  }
  if (!out.spec || !out.flow) {
    usage();
    process.exit(2);
  }
  if (!Number.isFinite(out.limit) || out.limit <= 0) {
    throw new Error("--limit must be positive");
  }
  return out;
}

function requireSdk(name) {
  try {
    return require(name);
  } catch (err) {
    console.error(
      `missing ${name}; run scripts/meteora-dlmm-account-probe.sh so the official SDK is installed in a temp directory`,
    );
    throw err;
  }
}

function sanitizeRpc(url) {
  return String(url).replace(/(api-key=|apikey=|key=)[^&]+/gi, "$1<redacted>");
}

function sleep(ms) {
  if (ms <= 0) {
    return Promise.resolve();
  }
  return new Promise((resolve) => setTimeout(resolve, ms));
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

function readJsonl(file) {
  return fs
    .readFileSync(file, "utf8")
    .split(/\r?\n/)
    .filter((line) => line.trim())
    .map((line, index) => {
      try {
        return JSON.parse(line);
      } catch (err) {
        throw new Error(`failed to parse ${file} line ${index + 1}: ${err.message}`);
      }
    });
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

function stringifyBn(value) {
  return JSON.parse(
    JSON.stringify(value, (_key, item) => {
      if (typeof item === "bigint") {
        return item.toString();
      }
      if (item && item.constructor?.name === "BN" && typeof item.toString === "function") {
        return item.toString();
      }
      return item;
    }),
  );
}

function decodedSwapInstructions(program, tx, poolAddress) {
  return flattenInstructions(tx)
    .filter((ix) => ix.programId === program.programId.toBase58())
    .filter((ix) => Array.isArray(ix.accounts) && ix.accounts.includes(poolAddress))
    .map((ix) => {
      try {
        const decoded = program.coder.instruction.decode(ix.data, "base58");
        return { ix, decoded };
      } catch (err) {
        return { ix, decode_error: String(err.message || err) };
      }
    })
    .filter((row) => row.decoded?.name?.toLowerCase().startsWith("swap"));
}

function parseEvents(eventParser, logs) {
  const events = [];
  for (const event of eventParser.parseLogs(logs || [])) {
    events.push({
      name: event.name,
      data: stringifyBn(event.data),
    });
  }
  return events;
}

async function classifyAccount(connection, program, decodeAccount, PublicKey, pubkey) {
  let info = null;
  try {
    info = await connection.getAccountInfo(new PublicKey(pubkey));
  } catch (err) {
    return {
      pubkey,
      account_type: "fetch_error",
      error: String(err.message || err).slice(0, 240),
    };
  }
  if (!info) {
    return { pubkey, account_type: "missing" };
  }
  const owner = info.owner.toBase58();
  if (owner !== program.programId.toBase58()) {
    return {
      pubkey,
      owner,
      bytes: info.data.length,
      account_type: "external",
    };
  }
  let accountType = "unknown_program_account";
  let decoded = null;
  for (const name of ["lbPair", "binArray", "binArrayBitmapExtension", "oracle"]) {
    try {
      decoded = decodeAccount(program, name, info.data);
      accountType = name;
      break;
    } catch (_err) {
      // Try the next known account type.
    }
  }
  return {
    pubkey,
    owner,
    bytes: info.data.length,
    account_type: accountType,
    index: decoded?.index?.toString?.() ?? null,
    active_id: decoded?.activeId?.toString?.() ?? null,
    bins: decoded?.bins?.length ?? null,
    caveat: "current account fetch; not historical state at transaction slot",
  };
}

async function currentActiveBinState(connection, program, helpers, spec) {
  const { PublicKey } = helpers;
  const {
    BN,
    decodeAccount,
    binIdToBinArrayIndex,
    deriveBinArray,
    getBinFromBinArray,
  } = helpers;
  const poolPk = new PublicKey(spec.pool_address);
  const poolInfo = await connection.getAccountInfo(poolPk);
  if (!poolInfo) {
    return { error: "pool account missing" };
  }
  const lbPair = decodeAccount(program, "lbPair", poolInfo.data);
  const activeId = new BN(lbPair.activeId);
  const binArrayIndex = binIdToBinArrayIndex(activeId);
  const [binArrayPk] = deriveBinArray(poolPk, binArrayIndex, program.programId);
  const binArrayInfo = await connection.getAccountInfo(binArrayPk);
  if (!binArrayInfo) {
    return {
      active_id: activeId.toString(),
      bin_step: Number(lbPair.binStep),
      bin_array_index: binArrayIndex.toString(),
      bin_array: binArrayPk.toBase58(),
      error: "active bin array account missing",
    };
  }
  const binArray = decodeAccount(program, "binArray", binArrayInfo.data);
  const bin = getBinFromBinArray(activeId.toNumber(), binArray);
  return {
    active_id: activeId.toString(),
    bin_step: Number(lbPair.binStep),
    bin_array_index: binArrayIndex.toString(),
    bin_array: binArrayPk.toBase58(),
    amount_x_raw: bin.amountX?.toString?.() ?? null,
    amount_y_raw: bin.amountY?.toString?.() ?? null,
    liquidity_supply_raw: bin.liquiditySupply?.toString?.() ?? null,
    caveat: "current decoded active bin; not historical state at sampled swap slots",
  };
}

async function main() {
  const args = parseArgs(process.argv);
  const { Connection, PublicKey } = requireSdk("@solana/web3.js");
  const {
    createProgram,
    decodeAccount,
    binIdToBinArrayIndex,
    deriveBinArray,
    getBinFromBinArray,
  } = requireSdk("@meteora-ag/dlmm");
  const { EventParser, BN } = requireSdk("@coral-xyz/anchor");

  const spec = JSON.parse(fs.readFileSync(args.spec, "utf8"));
  if (spec.replay_model !== "dlmm_bin_replay") {
    throw new Error(`spec replay_model must be dlmm_bin_replay, got ${spec.replay_model}`);
  }
  const flowRows = readJsonl(args.flow)
    .filter((row) => row.pool_address === spec.pool_address)
    .filter((row) => row.source === "meteora-dlmm-swap-flow")
    .slice(0, args.limit);
  if (flowRows.length === 0) {
    throw new Error(`no meteora-dlmm-swap-flow rows for ${spec.pool_address} in ${args.flow}`);
  }

  const connection = new Connection(args.rpc, "confirmed");
  const program = createProgram(connection, { cluster: "mainnet-beta" });
  const eventParser = new EventParser(program.programId, program.coder);

  const accountCache = new Map();
  async function cachedClassify(pubkey) {
    if (!accountCache.has(pubkey)) {
      accountCache.set(
        pubkey,
        await classifyAccount(connection, program, decodeAccount, PublicKey, pubkey),
      );
      await sleep(args.requestSleepMs);
    }
    return accountCache.get(pubkey);
  }

  const samples = [];
  for (const flow of flowRows) {
    const tx = await rpcCall(args.rpc, "getTransaction", [
      flow.signature,
      { encoding: "jsonParsed", maxSupportedTransactionVersion: 0 },
    ]);
    if (!tx) {
      samples.push({
        signature: flow.signature,
        slot: flow.block,
        error: "getTransaction returned null",
      });
      continue;
    }
    const events = parseEvents(eventParser, tx.meta?.logMessages || []);
    const swapIxs = decodedSwapInstructions(program, tx, spec.pool_address);
    const accounts = [...new Set(swapIxs.flatMap((row) => row.ix.accounts || []))];
    const classifiedAccounts = [];
    for (const pubkey of accounts) {
      classifiedAccounts.push(await cachedClassify(pubkey));
    }
    samples.push({
      signature: flow.signature,
      slot: tx.slot,
      flow_estimated_bin_id: flow.estimated_bin_id_from_avg_price ?? null,
      flow_amount_in_usd: flow.amount_in_usd ?? null,
      log_messages: tx.meta?.logMessages?.length ?? 0,
      parsed_events: events,
      swap_instructions: swapIxs.map((row) => ({
        name: row.decoded.name,
        data: stringifyBn(row.decoded.data),
        account_count: row.ix.accounts.length,
      })),
      program_accounts: classifiedAccounts.filter(
        (account) =>
          account.owner === program.programId.toBase58() ||
          account.account_type === "missing" ||
          account.account_type === "fetch_error",
      ),
    });
    await sleep(args.requestSleepMs);
  }

  const typedProgramAccounts = samples.flatMap((sample) => sample.program_accounts || []);
  const uniqueBinArrays = [
    ...new Map(
      typedProgramAccounts
        .filter((account) => account.account_type === "binArray")
        .map((account) => [account.pubkey, account]),
    ).values(),
  ];
  const eventNames = samples.flatMap((sample) => sample.parsed_events || []).map((event) => event.name);
  const decodedSwapIxs = samples.reduce(
    (sum, sample) => sum + (sample.swap_instructions?.length || 0),
    0,
  );
  const currentState = await currentActiveBinState(connection, program, {
    PublicKey,
    BN,
    decodeAccount,
    binIdToBinArrayIndex,
    deriveBinArray,
    getBinFromBinArray,
  }, spec);

  const report = {
    source: "meteora-dlmm-account-probe",
    sdk_package: "@meteora-ag/dlmm@1.9.10",
    rpc: sanitizeRpc(args.rpc),
    spec: args.spec,
    flow: args.flow,
    pool_address: spec.pool_address,
    symbol: spec.symbol,
    sampled_flow_rows: flowRows.length,
    decoded_swap_instructions: decodedSwapIxs,
    parsed_events: eventNames.length,
    parsed_event_names: [...new Set(eventNames)],
    unique_touched_bin_arrays: uniqueBinArrays.map((account) => ({
      pubkey: account.pubkey,
      index: account.index,
      bytes: account.bytes,
    })),
    current_active_bin_state: currentState,
    feasibility: {
      status: eventNames.length > 0 ? "needs_event_field_review" : "blocked_without_archival_account_state",
      can_identify_touched_bin_arrays_from_transactions: uniqueBinArrays.length > 0,
      current_account_decoding_works: !currentState.error,
      get_transaction_contains_historical_account_data: false,
      sampled_logs_had_parseable_swap_events: eventNames.some((name) =>
        name.toLowerCase().includes("swap"),
      ),
      blocker:
        "Solana getTransaction exposes instructions, logs, token balance deltas, and touched bin-array keys, but not historical lbPair/binArray account data. Public getAccountInfo returns current state only. Sampled logs produced no parseable Swap/Swap2Evt events with the official EventParser, so replay-grade historical active liquidity needs an archival account-state/indexer source or a same-slot live snapshot pipeline.",
    },
    samples,
  };

  if (args.rawOut) {
    fs.mkdirSync(path.dirname(args.rawOut), { recursive: true });
    fs.writeFileSync(args.rawOut, `${JSON.stringify(report, null, 2)}\n`);
  }
  console.log(
    JSON.stringify({
      pool: report.pool_address,
      symbol: report.symbol,
      sampled_flow_rows: report.sampled_flow_rows,
      decoded_swap_instructions: report.decoded_swap_instructions,
      parsed_events: report.parsed_events,
      touched_bin_arrays: report.unique_touched_bin_arrays.length,
      current_active_id: report.current_active_bin_state.active_id ?? null,
      status: report.feasibility.status,
      blocker: report.feasibility.blocker,
      raw_out: args.rawOut || null,
    }),
  );
}

main().catch((err) => {
  console.error(err && err.stack ? err.stack : err);
  process.exit(1);
});
