import { parseArgs } from "node:util";
import { fileURLToPath } from "node:url";
import { BN } from "@coral-xyz/anchor";
import { Connection, PublicKey } from "@solana/web3.js";
import {
  type ConfigParams,
  clusterUrl,
  defaultCpswapConfig,
  defaultKeypairPath,
  discover,
  initialize,
  loadKeypair,
  openCranker,
  parseCluster,
  printStatus,
  readIdl,
  runOnce,
  updateConfig,
} from "./crank.js";
import { fulfillLocal, initLocalVrf, pendingRequests } from "./vrf.js";

const DISCOVERY_REFRESH_MS = 5 * 60 * 1000;
/** An RPC call that hasn't answered by now fails instead of stalling the keeper. */
const RPC_TIMEOUT_MS = 30_000;
/** A pass still running after this exits the process, for the supervisor to restart. */
const PASS_LIMIT_MS = 10 * 60 * 1000;
const RATE_PER_BPS = 100;
const ORACLE_POLL_MS = 1_000;

const USAGE = `Usage: npm run crank -- <command> [options]

Crank commands (anyone):
  run              Claim and split fees and run kits, on a loop.
  once             Do it once and exit.
  status           Show the config, launches, and pending work.
  oracle           Localnet only: stand in for ORAO's oracles and answer VRF requests,
                   so Draw launches can settle. The keypair is the fulfillment authority.

Admin commands:
  initialize       Create the LabKit config. The program upgrade authority signs.
  update-config    Change the treasury, fee splits and platform template. The LabKit
                   authority signs. Existing launches keep what they were created with.

Options:
  --cluster mainnet|devnet|localnet   default mainnet
  --url <rpc>                         overrides the cluster URL
  --keypair <path>                    default ~/.config/solana/id.json
  --interval <ms>                     run loop delay, default 20000
  --min <lamports>                    smallest fee balance worth a transaction, default 1000000
  --treasury <pubkey>                 config: treasury wallet
  --crank-bps <n>                     config: keeper share of creator-side fees, default 500 (max 500)
  --kit-bps <n>                       config: kit share of platform-side fees, default 7000
  --amm-creator-bps <n>               config: share of the Raydium creator fee that counts as
                                      creator-side after graduation, default 5000
  --fee-bps <n>                       config: platform fee on curve trades, default 100
  --creator-fee-bps <n>               config: creator fee on curve trades, default 50 (max 50)
  --lp-lock-bps <n>                   config: graduated LP locked to --nft-wallet, default 0 (all burned)
  --nft-wallet <pubkey>               config: Fee Key owner, default the treasury
  --cpswap-config <pubkey>            config: CPMM AmmConfig, default index 11 (1% creator fee)
  --web, --img                        config: platform branding
`;

function required(value: string | undefined, name: string): string {
  if (!value) throw new Error(`missing --${name}`);
  return value;
}

function int(value: string | undefined, name: string, fallback?: number): number {
  if (value === undefined) {
    if (fallback === undefined) throw new Error(`missing --${name}`);
    return fallback;
  }
  const n = Number(value);
  if (!Number.isInteger(n) || n < 0) throw new Error(`--${name} must be a non-negative integer`);
  return n;
}

async function main() {
  const [command, ...argv] = process.argv.slice(2);
  if (!command || command === "help" || command === "--help") {
    console.log(USAGE);
    return;
  }

  const { values } = parseArgs({
    args: argv,
    options: {
      cluster: { type: "string", default: "mainnet" },
      url: { type: "string" },
      keypair: { type: "string" },
      interval: { type: "string" },
      min: { type: "string" },
      treasury: { type: "string" },
      "crank-bps": { type: "string" },
      "kit-bps": { type: "string" },
      "amm-creator-bps": { type: "string" },
      "fee-bps": { type: "string" },
      "creator-fee-bps": { type: "string" },
      "lp-lock-bps": { type: "string" },
      "nft-wallet": { type: "string" },
      "cpswap-config": { type: "string" },
      web: { type: "string" },
      img: { type: "string" },
    },
    strict: true,
  });

  const cluster = parseCluster(values.cluster ?? "mainnet");
  const connection = new Connection(values.url ?? clusterUrl(cluster), {
    commitment: "confirmed",
    fetch: (input, init) => fetch(input, { ...init, signal: AbortSignal.timeout(RPC_TIMEOUT_MS) }),
  });
  const payer = loadKeypair(values.keypair ?? defaultKeypairPath());
  const idlDir = new URL("../idl/", import.meta.url);
  const c = openCranker(
    connection,
    payer,
    cluster,
    await readIdl(fileURLToPath(new URL("labkit.json", idlDir))),
    await readIdl(fileURLToPath(new URL("raydium_launchpad.json", idlDir))),
  );

  switch (command) {
    case "status":
      await printStatus(c);
      return;
    case "once":
    case "run": {
      const min = BigInt(values.min ?? "1000000");
      const interval = int(values.interval, "interval", 20_000);
      if (command === "once") {
        console.log(`${await runOnce(c, await discover(c), min)} transaction(s)`);
        return;
      }
      // One failed or stuck pass must not stop the keeper: errors are logged and the next pass
      // runs, and a pass that never finishes exits the process so its supervisor restarts it.
      let discovery: Awaited<ReturnType<typeof discover>> | null = null;
      let discoveredAt = 0;
      for (;;) {
        const watchdog = setTimeout(() => {
          console.error(`a keeper pass ran over ${PASS_LIMIT_MS / 60_000} minutes; exiting so it restarts`);
          process.exit(1);
        }, PASS_LIMIT_MS);
        try {
          if (!discovery || Date.now() - discoveredAt > DISCOVERY_REFRESH_MS) {
            discovery = await discover(c);
            discoveredAt = Date.now();
          }
          await runOnce(c, discovery, min);
        } catch (err) {
          console.error("keeper pass failed:", err);
        } finally {
          clearTimeout(watchdog);
        }
        await new Promise((resolve) => setTimeout(resolve, interval));
      }
    }
    case "oracle": {
      if (cluster !== "localnet") throw new Error("oracle only runs on localnet; ORAO answers elsewhere");
      if (await initLocalVrf(connection, payer, payer.publicKey)) console.log("created the local ORAO network state");
      console.log(`answering ORAO requests as ${payer.publicKey.toBase58()}`);
      for (;;) {
        for (const seed of await pendingRequests(connection).catch(() => [])) {
          try {
            console.log(`fulfilled ${seed.toString("hex")} ${await fulfillLocal(connection, payer, seed)}`);
          } catch (err) {
            console.error(`fulfilling ${seed.toString("hex")} failed:`, err);
          }
        }
        await new Promise((resolve) => setTimeout(resolve, ORACLE_POLL_MS));
      }
    }
    case "initialize":
    case "update-config": {
      const treasury = new PublicKey(required(values.treasury, "treasury"));
      const rate = (name: string, fallback: number) => new BN(int(values[name as keyof typeof values] as string | undefined, name, fallback) * RATE_PER_BPS);
      const params: ConfigParams = {
        treasury,
        fees: {
          crankBps: int(values["crank-bps"], "crank-bps", 500),
          kitBps: int(values["kit-bps"], "kit-bps", 7_000),
          ammCreatorBps: int(values["amm-creator-bps"], "amm-creator-bps", 5_000),
        },
        platform: {
          feeRate: rate("fee-bps", 100),
          creatorFeeRate: rate("creator-fee-bps", 50),
          lpLockScale: rate("lp-lock-bps", 0),
          nftWallet: values["nft-wallet"] ? new PublicKey(values["nft-wallet"]) : treasury,
          cpswapConfig: values["cpswap-config"] ? new PublicKey(values["cpswap-config"]) : defaultCpswapConfig(cluster),
          web: values.web ?? "",
          img: values.img ?? "",
        },
      };
      const signature =
        command === "initialize" ? await initialize(c, params) : await updateConfig(c, params);
      console.log(`${command} ${signature}`);
      return;
    }
    default:
      throw new Error(`unknown command ${command}\n\n${USAGE}`);
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
