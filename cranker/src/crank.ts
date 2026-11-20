import { randomBytes } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { AnchorProvider, BN, Program, Wallet, type Idl } from "@coral-xyz/anchor";
import {
  type AccountInfo,
  ComputeBudgetProgram,
  Connection,
  Keypair,
  PublicKey,
  SYSVAR_CLOCK_PUBKEY,
  SystemProgram,
  type TransactionInstruction,
} from "@solana/web3.js";
import {
  ASSOCIATED_TOKEN_PROGRAM_ID,
  NATIVE_MINT,
  TOKEN_PROGRAM_ID,
  createAssociatedTokenAccountIdempotentInstruction,
  getAssociatedTokenAddressSync,
} from "@solana/spl-token";
import {
  type Cluster,
  CPMM_CONFIG_11_MAINNET,
  LABKIT_PROGRAM_ID,
  TOKEN_AMOUNT_OFFSET,
  configPda,
  creatorFeeVaultAuthorityPda,
  creatorFeeVaultPda,
  creatorPda,
  drawPda,
  drawTreePda,
  eventAuthorityPda,
  feeVaultAuthorityPda,
  feeWalletPda,
  kitPda,
  launchPda,
  launchlabProgramId,
  ORAO_VRF_PROGRAM_ID,
  oraoNetworkStatePda,
  oraoRandomnessPda,
  platformFeeVaultPda,
  poolVaultPda,
  programDataPda,
  stakePoolPda,
  vaultAuthorityPda,
} from "./ids.js";
import { drawPoint, oraoTreasury, readRandomness } from "./vrf.js";
import {
  type CpmmPool,
  collectCreatorFeeIx,
  cpmmAuthorityPda,
  cpmmProgramId,
  decodeCpmmPool,
  findCpmmPool,
  sides,
} from "./cpmm.js";
import { KIT } from "./launch.js";

export const RATE_DENOMINATOR = 1_000_000n;
const MULTIPLE_ACCOUNTS_LIMIT = 100;
/** Mirrors the program: the smallest kit buy, and one buy's cap as a share of the quote reserve. */
const MIN_EXECUTE_LAMPORTS = 10_000_000n;
const MAX_SPEND_OF_RESERVE_BPS = 100n;
/** Floor's high is only checkpointed once the price is this far above it, in bps. */
const CHECKPOINT_STEP_BPS = 100n;
const EXECUTE_COMPUTE_UNITS = 400_000;

const POOL_TRADING = 0;
const POOL_MIGRATED = 2;

/**
 * Mirrors the program's draw schedule and Floor's rise window. Localnet runs a
 * `local-timing` build, so these must match the build: the keeper only skips sends the
 * program would turn down anyway.
 */
export function drawTiming(cluster: Cluster) {
  return cluster === "localnet"
    ? { interval: 20, minStakeAge: 10, timeout: 30 }
    : { interval: 86_400, minStakeAge: 86_400, timeout: 3_600 };
}
const HIGH_RISE_BPS = 1_000n;
export const highRiseSeconds = (cluster: Cluster) => (cluster === "localnet" ? 2 : 3_600);

/** Mirrors the program's `raise_high`: Floor's high after seeing `price` at `now`. */
export function raiseHigh(launch: Pick<LaunchInfo, "highQ64" | "highAt">, price: bigint, now: number, cluster: Cluster) {
  const { highQ64: high } = launch;
  if (high === 0n) return price;
  const window = highRiseSeconds(cluster);
  const elapsed = BigInt(Math.min(Math.max(now - launch.highAt, 0), window));
  const cap = high + (high * HIGH_RISE_BPS * elapsed) / (10_000n * BigInt(window));
  const target = price < cap ? price : cap;
  return target > high ? target : high;
}
const DRAW_MIN_POT_LAMPORTS = 50_000_000n;
const DRAW_PICKS = 8;
/** `StakePosition`: discriminator, bump, owner, then base mint. */
const POSITION_BASE_MINT_OFFSET = 8 + 1 + 32;
/** `DrawTree`: discriminator, bump, frozen, len, then the entries. */
const DRAW_TREE_LEN_OFFSET = 10;
const DRAW_TREE_HEADER_LEN = 14;

export interface Cranker {
  connection: Connection;
  payer: Keypair;
  cluster: Cluster;
  labkit: Program;
  launchlab: Program;
}

export interface FeeSplit {
  /** Keeper share of creator-side fees. */
  crankBps: number;
  /** Kit share of platform-side fees. The treasury gets the rest. */
  kitBps: number;
  /** Share of Raydium creator fees that counts as creator-side after graduation. */
  ammCreatorBps: number;
}

export interface PlatformTemplate {
  /** LaunchLab rates on its 1_000_000 denominator. */
  feeRate: BN;
  creatorFeeRate: BN;
  lpLockScale: BN;
  nftWallet: PublicKey;
  cpswapConfig: PublicKey;
  web: string;
  img: string;
}

export interface ConfigParams {
  treasury: PublicKey;
  fees: FeeSplit;
  platform: PlatformTemplate;
}

export interface LabkitConfig extends ConfigParams {
  authority: PublicKey;
  pendingAuthority: PublicKey;
}

export interface LaunchInfo {
  address: PublicKey;
  baseMint: PublicKey;
  recipient: PublicKey;
  kit: number;
  drawdownBps: number;
  platformConfig: PublicKey;
  poolState: PublicKey;
  highQ64: bigint;
  /** Unix seconds when `highQ64` last rose. */
  highAt: number;
}

/** A constant-product market as the kit sees it. `baseLeft` is null on Raydium. */
interface Market {
  status: number;
  quote: bigint;
  base: bigint;
  feeRate: bigint;
  baseLeft: bigint | null;
  globalConfig: PublicKey;
  amm: CpmmPool | null;
}

/**
 * Work the crank can do. Fee kinds move WSOL out of a LaunchLab fee vault or a LabKit PDA's
 * WSOL account; `amount` is what sits there. Kit kinds act on a launch's kit.
 */
export type Pending =
  | { kind: "fee-wallet"; account: PublicKey; amount: bigint }
  | { kind: "kit-fees"; launch: LaunchInfo; account: PublicKey; amount: bigint }
  | { kind: "creator-vault"; launch: LaunchInfo; account: PublicKey; amount: bigint }
  | { kind: "creator-wallet"; launch: LaunchInfo; account: PublicKey; amount: bigint }
  /** Creator fees booked on a graduated launch's CPMM pool. `account` is the pool. */
  | { kind: "amm-creator"; launch: LaunchInfo; pool: CpmmPool; account: PublicKey; amount: bigint }
  | { kind: "kit-buy"; launch: LaunchInfo; market: Market; account: PublicKey; amount: bigint }
  | { kind: "kit-checkpoint"; launch: LaunchInfo; market: Market; account: PublicKey; amount: bigint }
  | { kind: "kit-release"; launch: LaunchInfo; account: PublicKey; amount: bigint }
  /** Draw kinds: `account` is the kit's WSOL and `amount` the pot. */
  | { kind: "draw-request"; launch: LaunchInfo; account: PublicKey; amount: bigint }
  | { kind: "draw-settle"; launch: LaunchInfo; account: PublicKey; amount: bigint; draw: DrawState; randomness: Buffer }
  | { kind: "draw-cancel"; launch: LaunchInfo; account: PublicKey; amount: bigint; draw: DrawState };

export interface DrawState {
  pending: boolean;
  seed: Buffer;
  requestedAt: number;
  lastDraw: number;
}

export interface Discovery {
  launches: LaunchInfo[];
  /** CPMM pool of each graduated launch, keyed by base58 base mint. */
  ammPools: Map<string, PublicKey>;
}

export function clusterUrl(cluster: string): string {
  switch (cluster) {
    case "mainnet":
      return "https://api.mainnet-beta.solana.com";
    case "devnet":
      return "https://api.devnet.solana.com";
    case "localnet":
      return "http://127.0.0.1:8899";
    default:
      throw new Error(`unknown cluster ${cluster}`);
  }
}

export function parseCluster(value: string): Cluster {
  if (value !== "mainnet" && value !== "devnet" && value !== "localnet") {
    throw new Error(`unknown cluster ${value}`);
  }
  return value;
}

export function defaultKeypairPath(): string {
  return path.join(os.homedir(), ".config", "solana", "id.json");
}

export function loadKeypair(file: string): Keypair {
  const resolved = file.startsWith("~") ? path.join(os.homedir(), file.slice(1)) : file;
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(resolved, "utf8"))));
}

export async function readIdl(file: string): Promise<Idl> {
  return JSON.parse(await fs.promises.readFile(file, "utf8")) as Idl;
}

export function openCranker(
  connection: Connection,
  payer: Keypair,
  cluster: Cluster,
  labkitIdl: Idl,
  launchlabIdl: Idl,
): Cranker {
  const provider = new AnchorProvider(connection, new Wallet(payer), { commitment: "confirmed" });
  const launchlab = new Program(
    { ...launchlabIdl, address: launchlabProgramId(cluster).toBase58() },
    provider,
  );
  return { connection, payer, cluster, labkit: new Program(labkitIdl, provider), launchlab };
}

export function defaultCpswapConfig(cluster: Cluster): PublicKey {
  if (cluster === "devnet") throw new Error("pass --cpswap-config on devnet");
  return CPMM_CONFIG_11_MAINNET;
}

export async function fetchConfig(c: Cranker): Promise<LabkitConfig | null> {
  const info = await c.connection.getAccountInfo(configPda());
  if (!info) return null;
  const config = c.labkit.coder.accounts.decode("config", info.data);
  return {
    authority: config.authority,
    pendingAuthority: config.pendingAuthority,
    treasury: config.treasury,
    fees: config.fees,
    platform: config.platform,
  };
}

export function decodeLaunch(c: Cranker, address: PublicKey, data: Buffer): LaunchInfo & { registered: boolean } {
  const launch = c.labkit.coder.accounts.decode("launch", data);
  return {
    address,
    baseMint: launch.baseMint,
    recipient: launch.recipient,
    kit: launch.kit,
    drawdownBps: launch.drawdownBps,
    platformConfig: launch.platformConfig,
    poolState: launch.poolState,
    registered: launch.registered,
    highQ64: BigInt(launch.highQ64.toString()),
    highAt: Number(launch.highAt),
  };
}

/** Every registered launch. */
export async function findLaunches(c: Cranker): Promise<LaunchInfo[]> {
  const accounts = await c.connection.getProgramAccounts(c.labkit.programId, {
    filters: [{ memcmp: c.labkit.coder.accounts.memcmp("launch") }],
  });
  return accounts.map(({ pubkey, account }) => decodeLaunch(c, pubkey, account.data)).filter((l) => l.registered);
}

export async function discover(c: Cranker): Promise<Discovery> {
  return { launches: await findLaunches(c), ammPools: new Map() };
}

const amountOf = (info: AccountInfo<Buffer> | null | undefined) =>
  info && info.data.length >= TOKEN_AMOUNT_OFFSET + 8 ? info.data.readBigUInt64LE(TOKEN_AMOUNT_OFFSET) : 0n;
const wsol = (owner: PublicKey) => getAssociatedTokenAddressSync(NATIVE_MINT, owner, true);

/** The CPMM pool LaunchLab moved a launch to, via the cpswap config of its platform. */
async function ammPoolOf(c: Cranker, d: Discovery, launch: LaunchInfo): Promise<PublicKey | null> {
  const key = launch.baseMint.toBase58();
  const known = d.ammPools.get(key);
  if (known) return known;
  const platformInfo = await c.connection.getAccountInfo(launch.platformConfig);
  if (!platformInfo) return null;
  const platform = c.launchlab.coder.accounts.decode("platformConfig", platformInfo.data);
  const pool = await findCpmmPool(
    c.connection,
    c.cluster,
    launch.baseMint,
    NATIVE_MINT,
    platform.cpswapConfig,
    creatorPda(launch.baseMint),
  );
  if (pool) d.ammPools.set(key, pool.address);
  return pool?.address ?? null;
}

/** Each launch's market: its curve while it trades, its CPMM pool once it has moved. */
async function readMarkets(c: Cranker, d: Discovery): Promise<Map<string, Market>> {
  const pools = await getAccounts(c.connection, d.launches.map((l) => l.poolState));
  const platforms = await getAccounts(c.connection, d.launches.map((l) => l.platformConfig));
  const globals = new Map<string, bigint>();
  const markets = new Map<string, Market>();
  for (const [i, launch] of d.launches.entries()) {
    const info = pools[i];
    if (!info) continue;
    const s = c.launchlab.coder.accounts.decode("poolState", info.data);
    const big = (v: BN) => BigInt(v.toString());
    const status = s.status as number;
    const globalConfig = s.globalConfig as PublicKey;
    const market: Market = {
      status,
      quote: big(s.virtualQuote) + big(s.realQuote),
      base: big(s.virtualBase) - big(s.realBase),
      feeRate: 0n,
      baseLeft: big(s.totalBaseSell) - big(s.realBase),
      globalConfig,
      amm: null,
    };
    if (status === POOL_TRADING) {
      const g = globalConfig.toBase58();
      if (!globals.has(g)) {
        const gi = await c.connection.getAccountInfo(globalConfig);
        globals.set(g, gi ? big(c.launchlab.coder.accounts.decode("globalConfig", gi.data).tradeFeeRate) : 0n);
      }
      const p = platforms[i] ? c.launchlab.coder.accounts.decode("platformConfig", platforms[i]!.data) : null;
      market.feeRate = globals.get(g)! + (p ? big(p.feeRate) + big(p.creatorFeeRate) : 0n);
    } else if (status === POOL_MIGRATED) {
      const address = await ammPoolOf(c, d, launch);
      const pool = address ? decodeCpmmPool(address, await c.connection.getAccountInfo(address)) : null;
      if (pool) {
        const { quote, base } = sides(pool, NATIVE_MINT);
        const [config, quoteVault, baseVault] = await c.connection.getMultipleAccountsInfo([
          pool.ammConfig,
          pool.vaults[quote],
          pool.vaults[base],
        ]);
        market.amm = pool;
        market.quote = amountOf(quoteVault) - pool.bookedFees[quote];
        market.base = amountOf(baseVault) - pool.bookedFees[base];
        market.baseLeft = null;
        market.feeRate = config ? config.data.readBigUInt64LE(12) + config.data.readBigUInt64LE(108) : 0n;
      }
    }
    markets.set(launch.baseMint.toBase58(), market);
  }
  return markets;
}

const priceQ64 = (m: Market) => (m.base > 0n ? (m.quote << 64n) / m.base : 0n);

/** What a kit can do now, mirroring the program's checks closely enough to skip doomed sends. */
function kitWork(
  c: Cranker,
  launch: LaunchInfo,
  market: Market,
  balance: bigint,
  now: number,
): "buy" | "checkpoint" | "release" | null {
  if (launch.kit === KIT.stake || launch.kit === KIT.draw) return null;
  const trading = market.status === POOL_TRADING;
  const onAmm = market.status === POOL_MIGRATED && market.amm !== null;
  if (launch.kit === KIT.graduate) {
    if (!trading) return balance > 0n ? "release" : null;
  } else if (!trading && !onAmm) {
    return null;
  }
  const cap = (market.quote * MAX_SPEND_OF_RESERVE_BPS) / 10_000n;
  let spend = balance < cap ? balance : cap;
  let fills = false;
  if (market.baseLeft !== null && market.baseLeft < market.base) {
    const net = (market.quote * market.baseLeft + market.base - market.baseLeft - 1n) / (market.base - market.baseLeft);
    const fill = (net * RATE_DENOMINATOR) / (RATE_DENOMINATOR - market.feeRate) + 2n;
    if (spend >= fill) [spend, fills] = [fill, true];
  }
  if (launch.kit === KIT.floor) {
    // A raised high still leaves the price above its line, so the stored high decides.
    const price = priceQ64(market);
    const line = (launch.highQ64 * (10_000n - BigInt(launch.drawdownBps))) / 10_000n;
    if (price > line) {
      const step = (launch.highQ64 * (10_000n + CHECKPOINT_STEP_BPS)) / 10_000n;
      return raiseHigh(launch, price, now, c.cluster) >= step ? "checkpoint" : null;
    }
  }
  return spend >= MIN_EXECUTE_LAMPORTS || (fills && spend > 0n) ? "buy" : null;
}

export async function findPending(c: Cranker, d: Discovery): Promise<Pending[]> {
  const ll = c.launchlab.programId;
  const feeWallet = wsol(feeWalletPda());
  const perLaunch = d.launches.map((launch) => {
    const creator = creatorPda(launch.baseMint);
    return {
      launch,
      kitFees: platformFeeVaultPda(ll, launch.platformConfig, NATIVE_MINT),
      creatorVault: creatorFeeVaultPda(ll, creator, NATIVE_MINT),
      creatorWallet: wsol(creator),
      kitWallet: wsol(kitPda(launch.baseMint)),
    };
  });
  const keys = [feeWallet, ...perLaunch.flatMap((l) => [l.kitFees, l.creatorVault, l.creatorWallet, l.kitWallet])];
  const [infos, markets, [clock, ...launches]] = await Promise.all([
    getAccounts(c.connection, keys),
    readMarkets(c, d),
    getAccounts(c.connection, [SYSVAR_CLOCK_PUBKEY, ...d.launches.map((l) => l.address)]),
  ]);
  const amounts = infos.map(amountOf);
  const now = clock ? Number(clock.data.readBigInt64LE(32)) : Math.floor(Date.now() / 1000);
  launches.forEach((info, i) => {
    if (!info) return;
    const { highQ64, highAt } = decodeLaunch(c, d.launches[i].address, info.data);
    Object.assign(d.launches[i], { highQ64, highAt });
  });

  const pending: Pending[] = [];
  if (amounts[0] > 0n) pending.push({ kind: "fee-wallet", account: feeWallet, amount: amounts[0] });
  perLaunch.forEach((l, i) => {
    const [kitFees, creatorVault, creatorWallet, kitWallet] = amounts.slice(1 + 4 * i, 5 + 4 * i);
    const { launch } = l;
    if (kitFees > 0n) pending.push({ kind: "kit-fees", launch, account: l.kitFees, amount: kitFees });
    if (creatorVault > 0n) pending.push({ kind: "creator-vault", launch, account: l.creatorVault, amount: creatorVault });
    if (creatorWallet > 0n) pending.push({ kind: "creator-wallet", launch, account: l.creatorWallet, amount: creatorWallet });

    const market = markets.get(launch.baseMint.toBase58());
    if (!market) return;
    if (market.amm) {
      const amount = market.amm.creatorFees[sides(market.amm, NATIVE_MINT).quote];
      if (amount > 0n) pending.push({ kind: "amm-creator", launch, pool: market.amm, account: market.amm.address, amount });
    }
    const work = kitWork(c, launch, market, kitWallet, now);
    if (work === "buy") pending.push({ kind: "kit-buy", launch, market, account: l.kitWallet, amount: kitWallet });
    if (work === "checkpoint") pending.push({ kind: "kit-checkpoint", launch, market, account: l.kitWallet, amount: kitWallet });
    if (work === "release") pending.push({ kind: "kit-release", launch, account: l.kitWallet, amount: kitWallet });
  });
  const kitBalances = new Map(perLaunch.map((l, i) => [l.launch.baseMint.toBase58(), amounts[4 + 4 * i]]));
  pending.push(...(await drawWork(c, d.launches.filter((l) => l.kit === KIT.draw), kitBalances)));
  return pending;
}

export function decodeDraw(c: Cranker, data: Buffer): DrawState {
  const d = c.labkit.coder.accounts.decode("draw", data);
  return {
    pending: d.pending,
    seed: Buffer.from(d.seed),
    requestedAt: Number(d.requestedAt),
    lastDraw: Number(d.lastDraw),
  };
}

/** Draws to request, settle or cancel, judged on chain time. */
async function drawWork(c: Cranker, launches: LaunchInfo[], kitBalances: Map<string, bigint>): Promise<Pending[]> {
  if (launches.length === 0) return [];
  const timing = drawTiming(c.cluster);
  const keys = [SYSVAR_CLOCK_PUBKEY, ...launches.flatMap((l) => [drawPda(l.baseMint), stakePoolPda(l.baseMint)])];
  const infos = await getAccounts(c.connection, keys);
  const now = infos[0] ? Number(infos[0].data.readBigInt64LE(32)) : Math.floor(Date.now() / 1000);
  const pending: Pending[] = [];
  for (const [i, launch] of launches.entries()) {
    const [drawInfo, poolInfo] = [infos[1 + 2 * i], infos[2 + 2 * i]];
    if (!drawInfo || !poolInfo) continue;
    const draw = decodeDraw(c, drawInfo.data);
    const pool = c.labkit.coder.accounts.decode("stakePool", poolInfo.data);
    const owed = BigInt(pool.distributed.toString()) - BigInt(pool.claimed.toString());
    const balance = kitBalances.get(launch.baseMint.toBase58()) ?? 0n;
    const pot = balance > owed ? balance - owed : 0n;
    const account = wsol(kitPda(launch.baseMint));
    if (draw.pending) {
      const randomness = await readRandomness(c.connection, draw.seed);
      if (randomness) pending.push({ kind: "draw-settle", launch, account, amount: pot, draw, randomness });
      else if (now >= draw.requestedAt + timing.timeout) pending.push({ kind: "draw-cancel", launch, account, amount: pot, draw });
    } else if (
      now >= draw.lastDraw + timing.interval &&
      pot >= DRAW_MIN_POT_LAMPORTS &&
      BigInt(pool.totalStaked.toString()) > 0n
    ) {
      pending.push({ kind: "draw-request", launch, account, amount: pot });
    }
  }
  return pending;
}

/** Fenwick search over the draw tree's entries: the least index whose prefix sum exceeds `point`. */
function treeFind(entries: Buffer, len: number, point: bigint): number {
  const get = (i: number) => entries.readBigUInt64LE((i - 1) * 8);
  let at = 0;
  let left = point;
  for (let step = 1 << Math.floor(Math.log2(Math.max(len, 1))); step > 0; step >>= 1) {
    const next = at + step;
    if (next <= len && get(next) <= left) {
      at = next;
      left -= get(next);
    }
  }
  return at + 1;
}

function treeTotal(entries: Buffer, len: number): bigint {
  let sum = 0n;
  for (let i = len; i > 0; i -= i & -i) sum += entries.readBigUInt64LE((i - 1) * 8);
  return sum;
}

/**
 * The positions `settle_draw` will look at, in order: each pick's owner until one staked
 * long enough before the request, at most `DRAW_PICKS`.
 */
export async function drawCandidates(c: Cranker, baseMint: PublicKey, draw: DrawState, randomness: Buffer) {
  const [tree, positions] = await Promise.all([
    c.connection.getAccountInfo(drawTreePda(baseMint)),
    c.connection.getProgramAccounts(LABKIT_PROGRAM_ID, {
      filters: [
        { memcmp: { offset: 0, bytes: c.labkit.coder.accounts.memcmp("stakePosition").bytes as string } },
        { memcmp: { offset: POSITION_BASE_MINT_OFFSET, bytes: baseMint.toBase58() } },
      ],
    }),
  ]);
  if (!tree) throw new Error(`no draw tree for ${baseMint.toBase58()}`);
  const len = tree.data.readUInt32LE(DRAW_TREE_LEN_OFFSET);
  const entries = tree.data.subarray(DRAW_TREE_HEADER_LEN);
  const total = treeTotal(entries, len);
  const bySlot = new Map<number, { address: PublicKey; stakedAt: number; owner: PublicKey }>();
  for (const { pubkey, account } of positions) {
    const p = c.labkit.coder.accounts.decode("stakePosition", account.data);
    bySlot.set(p.slot, { address: pubkey, stakedAt: Number(p.stakedAt), owner: p.owner });
  }
  const eligibleBefore = draw.requestedAt - drawTiming(c.cluster).minStakeAge;
  const picked: { address: PublicKey; owner: PublicKey; eligible: boolean }[] = [];
  for (let n = 0; n < DRAW_PICKS; n++) {
    const slot = treeFind(entries, len, drawPoint(randomness, n, total));
    const position = bySlot.get(slot);
    if (!position) throw new Error(`no position holds draw slot ${slot}`);
    const eligible = position.stakedAt <= eligibleBefore;
    picked.push({ address: position.address, owner: position.owner, eligible });
    if (eligible) break;
  }
  return picked;
}

export function drawAccounts(baseMint: PublicKey) {
  const kit = kitPda(baseMint);
  return {
    launch: launchPda(baseMint),
    stakePool: stakePoolPda(baseMint),
    draw: drawPda(baseMint),
    drawTree: drawTreePda(baseMint),
    kit,
    quoteMint: NATIVE_MINT,
    kitQuote: wsol(kit),
    tokenProgram: TOKEN_PROGRAM_ID,
  };
}

/**
 * Fee moves under `min` wait for more to accrue. An `amm-creator` split takes the whole
 * creator wallet, so that launch's `creator-wallet` is dropped. Kit work goes last, after
 * the claims that fund it.
 */
export function selectWork(pending: Pending[], min: bigint): Pending[] {
  const isKit = (p: Pending) => p.kind.startsWith("kit-") && p.kind !== "kit-fees" || p.kind.startsWith("draw-");
  const amm = new Set(pending.flatMap((p) => (p.kind === "amm-creator" ? [p.launch.baseMint.toBase58()] : [])));
  const fees = pending.filter(
    (p) =>
      !isKit(p) &&
      p.amount >= min &&
      !(p.kind === "creator-wallet" && amm.has(p.launch.baseMint.toBase58())),
  );
  return [...fees, ...pending.filter(isKit)];
}

/** `overrides` replaces accounts by name. */
export async function crank(
  c: Cranker,
  config: LabkitConfig,
  p: Pending,
  overrides: Record<string, PublicKey> = {},
): Promise<string> {
  const ll = c.launchlab.programId;
  const ensure = (owner: PublicKey, mint = NATIVE_MINT): TransactionInstruction =>
    createAssociatedTokenAccountIdempotentInstruction(
      c.payer.publicKey,
      getAssociatedTokenAddressSync(mint, owner, true),
      owner,
      mint,
    );
  const programs = {
    tokenProgram: TOKEN_PROGRAM_ID,
    associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
    systemProgram: SystemProgram.programId,
  };
  const crankToken = wsol(c.payer.publicKey);
  const treasuryToken = wsol(config.treasury);

  if (p.kind === "fee-wallet") {
    return c.labkit.methods
      .splitFees()
      .accountsStrict({
        crank: c.payer.publicKey,
        config: configPda(),
        feeWallet: feeWalletPda(),
        mint: NATIVE_MINT,
        feeWalletToken: p.account,
        treasuryToken,
        tokenProgram: TOKEN_PROGRAM_ID,
        ...overrides,
      })
      .preInstructions([ensure(config.treasury)])
      .rpc();
  }

  const { launch } = p;
  const baseMint = launch.baseMint;
  const kit = kitPda(baseMint);
  const creator = creatorPda(baseMint);
  const recipientToken = wsol(launch.recipient);

  switch (p.kind) {
    case "kit-fees":
      return c.labkit.methods
        .claimKitFees()
        .accountsStrict({
          crank: c.payer.publicKey,
          config: configPda(),
          launch: launch.address,
          kit,
          quoteMint: NATIVE_MINT,
          kitQuote: wsol(kit),
          treasuryToken,
          platformConfig: launch.platformConfig,
          platformFeeVault: p.account,
          feeVaultAuthority: feeVaultAuthorityPda(ll),
          launchlabProgram: ll,
          ...programs,
          ...overrides,
        })
        .preInstructions([ensure(config.treasury)])
        .rpc();
    case "creator-vault":
      return c.labkit.methods
        .claimCreatorFees()
        .accountsStrict({
          crank: c.payer.publicKey,
          launch: launch.address,
          creator,
          quoteMint: NATIVE_MINT,
          creatorToken: wsol(creator),
          recipientToken,
          crankToken,
          creatorFeeVault: p.account,
          feeVaultAuthority: creatorFeeVaultAuthorityPda(ll),
          launchlabProgram: ll,
          ...programs,
          ...overrides,
        })
        .preInstructions([ensure(launch.recipient), ensure(c.payer.publicKey)])
        .rpc();
    case "creator-wallet":
    case "amm-creator": {
      const pre = [ensure(launch.recipient), ensure(c.payer.publicKey), ensure(config.treasury)];
      if (p.kind === "amm-creator") pre.push(collectCreatorFeeIx(c.cluster, c.payer.publicKey, p.pool));
      return c.labkit.methods
        .splitCreatorFees()
        .accountsStrict({
          crank: c.payer.publicKey,
          config: configPda(),
          launch: launch.address,
          poolState: launch.poolState,
          creator,
          kit,
          quoteMint: NATIVE_MINT,
          creatorToken: wsol(creator),
          kitQuote: wsol(kit),
          recipientToken,
          treasuryToken,
          crankToken,
          ...programs,
          ...overrides,
        })
        .preInstructions(pre)
        .rpc();
    }
    case "draw-request": {
      const seed = randomBytes(32);
      return c.labkit.methods
        .requestDraw([...seed])
        .accountsStrict({
          keeper: c.payer.publicKey,
          keeperQuote: crankToken,
          ...drawAccounts(baseMint),
          vrf: ORAO_VRF_PROGRAM_ID,
          networkState: oraoNetworkStatePda(),
          treasury: await oraoTreasury(c.connection),
          randomness: oraoRandomnessPda(seed),
          systemProgram: SystemProgram.programId,
          ...overrides,
        })
        .preInstructions([ensure(c.payer.publicKey)])
        .rpc();
    }
    case "draw-settle": {
      const picked = await drawCandidates(c, baseMint, p.draw, p.randomness);
      const { launch: launchKey, stakePool, draw, drawTree, quoteMint, kitQuote, tokenProgram } = drawAccounts(baseMint);
      return c.labkit.methods
        .settleDraw()
        .accountsStrict({
          launch: launchKey,
          stakePool,
          draw,
          drawTree,
          kit,
          quoteMint,
          kitQuote,
          randomness: oraoRandomnessPda(p.draw.seed),
          tokenProgram,
          ...overrides,
        })
        .remainingAccounts(picked.map(({ address }) => ({ pubkey: address, isSigner: false, isWritable: true })))
        .rpc();
    }
    case "draw-cancel":
      return c.labkit.methods
        .cancelDraw()
        .accountsStrict({
          launch: launch.address,
          draw: drawPda(baseMint),
          drawTree: drawTreePda(baseMint),
          randomness: oraoRandomnessPda(p.draw.seed),
          ...overrides,
        })
        .rpc();
    case "kit-release":
      return c.labkit.methods
        .release()
        .accountsStrict({
          crank: c.payer.publicKey,
          launch: launch.address,
          poolState: launch.poolState,
          kit,
          quoteMint: NATIVE_MINT,
          kitQuote: p.account,
          recipientToken,
          tokenProgram: TOKEN_PROGRAM_ID,
          ...overrides,
        })
        .preInstructions([ensure(launch.recipient)])
        .rpc();
    case "kit-checkpoint": {
      const amm = p.market.amm;
      const remaining = amm
        ? [amm.address, amm.ammConfig, amm.vaults[sides(amm, NATIVE_MINT).quote], amm.vaults[sides(amm, NATIVE_MINT).base]]
        : [];
      return c.labkit.methods
        .checkpoint()
        .accountsStrict({
          crank: c.payer.publicKey,
          launch: launch.address,
          poolState: launch.poolState,
          platformConfig: launch.platformConfig,
          creator,
          ...overrides,
        })
        .remainingAccounts(remaining.map((pubkey) => ({ pubkey, isSigner: false, isWritable: false })))
        .rpc();
    }
    case "kit-buy":
      return kitBuyMethod(c, p, overrides).rpc();
  }
}

/** `execute_swap` once the launch trades on Raydium, `execute_curve` before. */
export function kitBuyMethod(
  c: Cranker,
  p: Extract<Pending, { kind: "kit-buy" }>,
  overrides: Record<string, PublicKey> = {},
) {
  const ll = c.launchlab.programId;
  const { launch } = p;
  const baseMint = launch.baseMint;
  const kit = kitPda(baseMint);
  const creator = creatorPda(baseMint);
  const budget = [ComputeBudgetProgram.setComputeUnitLimit({ units: EXECUTE_COMPUTE_UNITS })];
  const common = {
    crank: c.payer.publicKey,
    launch: launch.address,
    kit,
    baseMint,
    quoteMint: NATIVE_MINT,
    kitBase: getAssociatedTokenAddressSync(baseMint, kit, true),
    kitQuote: p.account,
    poolState: launch.poolState,
    platformConfig: launch.platformConfig,
    tokenProgram: TOKEN_PROGRAM_ID,
    associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
    systemProgram: SystemProgram.programId,
  };
  const amm = p.market.amm;
  if (amm) {
    const { quote, base } = sides(amm, NATIVE_MINT);
    return c.labkit.methods
      .executeSwap()
      .accountsStrict({
        ...common,
        creator,
        cpmmPool: amm.address,
        ammConfig: amm.ammConfig,
        cpmmAuthority: cpmmAuthorityPda(c.cluster),
        quoteVault: amm.vaults[quote],
        baseVault: amm.vaults[base],
        observation: amm.observation,
        cpmmProgram: cpmmProgramId(c.cluster),
        ...overrides,
      })
      .preInstructions(budget);
  }
  return c.labkit.methods
    .executeCurve()
    .accountsStrict({
      ...common,
      globalConfig: p.market.globalConfig,
      launchlabAuthority: vaultAuthorityPda(ll),
      baseVault: poolVaultPda(ll, launch.poolState, baseMint),
      quoteVault: poolVaultPda(ll, launch.poolState, NATIVE_MINT),
      eventAuthority: eventAuthorityPda(ll),
      platformFeeVault: platformFeeVaultPda(ll, launch.platformConfig, NATIVE_MINT),
      creatorFeeVault: creatorFeeVaultPda(ll, creator, NATIVE_MINT),
      launchlabProgram: ll,
      ...overrides,
    })
    .preInstructions(budget);
}

const KIT_NAMES = ["Burn", "Draw", "Floor", "Graduate", "Stake"];

export function describe(p: Pending): string {
  if (p.kind === "fee-wallet") return "split referral fees";
  const mint = p.launch.baseMint.toBase58();
  switch (p.kind) {
    case "kit-fees":
      return `claim platform fees of ${mint}`;
    case "creator-vault":
      return `claim creator fees of ${mint}`;
    case "creator-wallet":
      return `split creator wallet of ${mint}`;
    case "amm-creator":
      return `collect Raydium creator fees of ${mint}`;
    case "kit-buy":
      return `${KIT_NAMES[p.launch.kit]} buy for ${mint} on ${p.market.amm ? "Raydium" : "the curve"}`;
    case "kit-checkpoint":
      return `Floor checkpoint for ${mint}`;
    case "kit-release":
      return `Graduate release for ${mint}`;
    case "draw-request":
      return `request a draw for ${mint}`;
    case "draw-settle":
      return `settle the draw for ${mint}`;
    case "draw-cancel":
      return `cancel the unanswered draw for ${mint}`;
  }
}

/** Kit work the program turns down because the market or a draw moved since it was read. */
const EXPECTED = [
  "BelowMinimum",
  "AboveFloor",
  "ExecutedThisSlot",
  "DrawNotDue",
  "PotTooSmall",
  "NotTimedOut",
  "NoDrawPending",
  "DrawInProgress",
  "AlreadyFulfilled",
];

export async function runOnce(c: Cranker, discovery: Discovery, min: bigint): Promise<number> {
  const config = await fetchConfig(c);
  if (!config) throw new Error("LabKit is not initialized on this cluster");
  const pending = await findPending(c, discovery);
  let sent = 0;
  for (const p of selectWork(pending, min)) {
    try {
      const signature = await crank(c, config, p);
      console.log(`${describe(p)}: ${p.amount} ${signature}`);
      sent += 1;
    } catch (err) {
      const text = `${String(err)} ${((err as { logs?: string[] }).logs ?? []).join(" ")}`;
      if (EXPECTED.some((code) => text.includes(code))) continue;
      console.error(`${describe(p)} failed:`, err);
    }
  }
  return sent;
}

export async function printStatus(c: Cranker): Promise<void> {
  const config = await fetchConfig(c);
  console.log(`LabKit program   ${LABKIT_PROGRAM_ID.toBase58()}`);
  console.log(`LaunchLab        ${c.launchlab.programId.toBase58()}`);
  console.log(`Fee wallet PDA   ${feeWalletPda().toBase58()}`);
  if (!config) {
    console.log("Config           not initialized");
    return;
  }
  const { fees, platform } = config;
  console.log(`Authority        ${config.authority.toBase58()}`);
  if (!config.pendingAuthority.equals(PublicKey.default)) {
    console.log(`Pending          ${config.pendingAuthority.toBase58()}`);
  }
  console.log(`Treasury         ${config.treasury.toBase58()}`);
  console.log(`Keeper           ${fees.crankBps} bps of creator-side fees`);
  console.log(`Kit              ${fees.kitBps} bps of platform-side fees`);
  console.log(`Raydium split    ${fees.ammCreatorBps} bps of the Raydium creator fee counts as creator-side`);
  console.log(`New platforms    fee ${platform.feeRate} creator ${platform.creatorFeeRate} (of 1_000_000), CPMM config ${platform.cpswapConfig.toBase58()}`);

  const discovery = await discover(c);
  console.log(`Launches         ${discovery.launches.length}`);
  const pending = await findPending(c, discovery);
  console.log(`Pending          ${pending.length}`);
  for (const p of pending) console.log(`  ${describe(p)}: ${p.amount}`);
}

export async function initialize(c: Cranker, params: ConfigParams): Promise<string> {
  return c.labkit.methods
    .initialize(params)
    .accountsStrict({
      authority: c.payer.publicKey,
      config: configPda(),
      feeWallet: feeWalletPda(),
      program: LABKIT_PROGRAM_ID,
      programData: programDataPda(LABKIT_PROGRAM_ID),
      systemProgram: SystemProgram.programId,
    })
    .rpc();
}

export async function updateConfig(c: Cranker, params: ConfigParams): Promise<string> {
  return c.labkit.methods
    .updateConfig(params)
    .accountsStrict({ authority: c.payer.publicKey, config: configPda() })
    .rpc();
}

async function getAccounts(connection: Connection, keys: PublicKey[]) {
  const out = [];
  for (let i = 0; i < keys.length; i += MULTIPLE_ACCOUNTS_LIMIT) {
    out.push(...(await connection.getMultipleAccountsInfo(keys.slice(i, i + MULTIPLE_ACCOUNTS_LIMIT))));
  }
  return out;
}
