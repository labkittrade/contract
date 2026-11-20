// Shared setup for the local tests against `scripts/local-validator.sh`.
import { fileURLToPath } from "node:url";
import { BN } from "@coral-xyz/anchor";
import {
  ComputeBudgetProgram,
  Connection,
  Keypair,
  LAMPORTS_PER_SOL,
  PublicKey,
  SYSVAR_CLOCK_PUBKEY,
  SystemProgram,
  Transaction,
  type TransactionInstruction,
} from "@solana/web3.js";
import {
  NATIVE_MINT,
  TOKEN_PROGRAM_ID,
  createAssociatedTokenAccountIdempotentInstruction,
  createSyncNativeInstruction,
  getAssociatedTokenAddressSync,
} from "@solana/spl-token";
import {
  type ConfigParams,
  type Cranker,
  type LabkitConfig,
  decodeLaunch,
  defaultCpswapConfig,
  defaultKeypairPath,
  fetchConfig,
  initialize,
  loadKeypair,
  openCranker,
  readIdl,
} from "./crank.js";
import { type CpmmPool, sides, swapIx } from "./cpmm.js";
import { type KitName, type LaunchParams, type VestingParams, KIT, launchInstructions } from "./launch.js";
import {
  SOL_GLOBAL_CONFIG_MAINNET,
  creatorFeeVaultPda,
  creatorPda,
  eventAuthorityPda,
  feeWalletPda,
  kitPda,
  launchPda,
  launchPlatformPda,
  platformFeeVaultPda,
  poolStatePda,
  poolVaultPda,
  drawPda,
  drawTreePda,
  stakePoolPda,
  stakePositionPda,
  vaultAuthorityPda,
} from "./ids.js";
import { fulfillLocal, initLocalVrf } from "./vrf.js";

export const PLATFORM_FEE_BPS = 100;
export const CREATOR_FEE_BPS = 50;
export const CRANK_BPS = 500;
export const KIT_BPS = 7_000;
export const AMM_CREATOR_BPS = 5_000;
export const REFERRAL_RATE = 10_000; // 1% on LaunchLab's 1_000_000 denominator
export const BUY_LAMPORTS = BigInt(LAMPORTS_PER_SOL);

export const bps = (amount: bigint, rate: number) => (amount * BigInt(rate)) / 10_000n;

export interface Env {
  connection: Connection;
  open: (kp: Keypair) => Cranker;
  authority: Keypair;
}

export async function openEnv(): Promise<Env> {
  const connection = new Connection(process.env.LABKIT_RPC_URL ?? "http://127.0.0.1:8899", "confirmed");
  const idlDir = new URL("../idl/", import.meta.url);
  const labkitIdl = await readIdl(fileURLToPath(new URL("labkit.json", idlDir)));
  const launchlabIdl = await readIdl(fileURLToPath(new URL("raydium_launchpad.json", idlDir)));
  return {
    connection,
    open: (kp) => openCranker(connection, kp, "localnet", labkitIdl, launchlabIdl),
    authority: loadKeypair(defaultKeypairPath()),
  };
}

export async function funded(env: Env, count: number): Promise<Keypair[]> {
  const keys = Array.from({ length: count }, () => Keypair.generate());
  for (const kp of [env.authority, ...keys]) await airdrop(env.connection, kp.publicKey, 20);
  return keys;
}

export function testConfig(treasury: PublicKey): ConfigParams {
  return {
    treasury,
    fees: { crankBps: CRANK_BPS, kitBps: KIT_BPS, ammCreatorBps: AMM_CREATOR_BPS },
    platform: {
      feeRate: new BN(PLATFORM_FEE_BPS * 100),
      creatorFeeRate: new BN(CREATOR_FEE_BPS * 100),
      lpLockScale: new BN(0),
      nftWallet: treasury,
      cpswapConfig: defaultCpswapConfig("localnet"),
      web: "https://example.com",
      img: "https://example.com/logo.png",
    },
  };
}

/** Initializes LabKit with the test rates on a fresh validator. */
export async function ensureConfig(env: Env): Promise<LabkitConfig> {
  const owner = env.open(env.authority);
  if (!(await fetchConfig(owner))) await initialize(owner, testConfig(Keypair.generate().publicKey));
  const config = await fetchConfig(owner);
  if (!config) throw new Error("initialize did not create the config");
  return config;
}

export function launchParams(
  recipient: PublicKey,
  kit: KitName = "burn",
  drawdownBps = 0,
  vesting?: VestingParams,
): LaunchParams {
  return {
    globalConfig: SOL_GLOBAL_CONFIG_MAINNET,
    quoteMint: NATIVE_MINT,
    quoteTokenProgram: TOKEN_PROGRAM_ID,
    recipient,
    kit: KIT[kit],
    drawdownBps,
    name: "LabKit Test",
    symbol: "LKT",
    uri: "https://example.com/lkt.json",
    decimals: 6,
    supply: new BN("1000000000000000"),
    totalBaseSell: new BN("793100000000000"),
    totalQuoteFundRaising: new BN(85 * LAMPORTS_PER_SOL),
    vesting,
  };
}

/** Sends both launch transactions. Returns the base mint and its platform. */
export async function launch(
  c: Cranker,
  config: LabkitConfig,
  params: LaunchParams,
): Promise<{ mint: PublicKey; platform: PublicKey }> {
  const base = Keypair.generate();
  const [create, start] = await launchInstructions(c, config, params, base);
  await send(c, create, [base]);
  await send(c, start, [base]);
  return { mint: base.publicKey, platform: launchPlatformPda(c.launchlab.programId, base.publicKey) };
}

function curveAccounts(c: Cranker, platform: PublicKey, baseMint: PublicKey) {
  const ll = c.launchlab.programId;
  const user = c.payer.publicKey;
  const pool = poolStatePda(ll, baseMint, NATIVE_MINT);
  return {
    accounts: {
      payer: user,
      authority: vaultAuthorityPda(ll),
      globalConfig: SOL_GLOBAL_CONFIG_MAINNET,
      platformConfig: platform,
      poolState: pool,
      userBaseToken: getAssociatedTokenAddressSync(baseMint, user),
      userQuoteToken: ata(user),
      baseVault: poolVaultPda(ll, pool, baseMint),
      quoteVault: poolVaultPda(ll, pool, NATIVE_MINT),
      baseTokenMint: baseMint,
      quoteTokenMint: NATIVE_MINT,
      baseTokenProgram: TOKEN_PROGRAM_ID,
      quoteTokenProgram: TOKEN_PROGRAM_ID,
      eventAuthority: eventAuthorityPda(ll),
      program: ll,
    },
    remaining: (shareFeeReceiver: PublicKey | null) => [
      ...(shareFeeReceiver ? [{ pubkey: shareFeeReceiver, isSigner: false, isWritable: true }] : []),
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      { pubkey: platformFeeVaultPda(ll, platform, NATIVE_MINT), isSigner: false, isWritable: true },
      { pubkey: creatorFeeVaultPda(ll, creatorPda(baseMint), NATIVE_MINT), isSigner: false, isWritable: true },
    ],
  };
}

export async function buy(
  c: Cranker,
  platform: PublicKey,
  baseMint: PublicKey,
  shareFeeRate = 0,
  shareFeeReceiver: PublicKey | null = null,
  lamports = BUY_LAMPORTS,
): Promise<void> {
  const user = c.payer.publicKey;
  const { accounts, remaining } = curveAccounts(c, platform, baseMint);
  const pre = [
    createAssociatedTokenAccountIdempotentInstruction(user, accounts.userBaseToken, user, baseMint),
    createAssociatedTokenAccountIdempotentInstruction(user, accounts.userQuoteToken, user, NATIVE_MINT),
    SystemProgram.transfer({ fromPubkey: user, toPubkey: accounts.userQuoteToken, lamports: Number(lamports) }),
    createSyncNativeInstruction(accounts.userQuoteToken),
  ];
  if (shareFeeReceiver) {
    pre.push(
      createAssociatedTokenAccountIdempotentInstruction(user, shareFeeReceiver, feeWalletPda(), NATIVE_MINT),
    );
  }
  await c.launchlab.methods
    .buyExactIn(new BN(lamports.toString()), new BN(0), new BN(shareFeeRate))
    .accountsStrict(accounts)
    .remainingAccounts(remaining(shareFeeReceiver))
    .preInstructions(pre)
    .rpc();
}

/** Sells `amount` raw tokens on the curve. The SOL stays in the wallet's WSOL account. */
export async function sell(c: Cranker, platform: PublicKey, baseMint: PublicKey, amount: bigint): Promise<void> {
  const { accounts, remaining } = curveAccounts(c, platform, baseMint);
  await c.launchlab.methods
    .sellExactIn(new BN(amount.toString()), new BN(0), new BN(0))
    .accountsStrict(accounts)
    .remainingAccounts(remaining(null))
    .rpc();
}

/** Buys on a CPMM pool with `lamports` of the wallet's SOL, wrapped first. */
export async function ammBuy(c: Cranker, pool: CpmmPool, baseMint: PublicKey, lamports: bigint): Promise<void> {
  const user = c.payer.publicKey;
  const userQuote = ata(user);
  const userBase = getAssociatedTokenAddressSync(baseMint, user);
  const { quote } = sides(pool, NATIVE_MINT);
  await send(
    c,
    [
      createAssociatedTokenAccountIdempotentInstruction(user, userBase, user, baseMint),
      createAssociatedTokenAccountIdempotentInstruction(user, userQuote, user, NATIVE_MINT),
      SystemProgram.transfer({ fromPubkey: user, toPubkey: userQuote, lamports: Number(lamports) }),
      createSyncNativeInstruction(userQuote),
      swapIx(c.cluster, pool, user, quote, { input: userQuote, output: userBase }, lamports, 0n),
    ],
    [],
  );
}

export interface StakeOverrides {
  ownerBase?: PublicKey;
  ownerQuote?: PublicKey;
  vault?: PublicKey;
  kitQuote?: PublicKey;
  position?: PublicKey;
  /** Null to leave the draw tree out. */
  drawTree?: PublicKey | null;
}

function stakeAccounts(c: Cranker, baseMint: PublicKey, o: StakeOverrides = {}) {
  const owner = c.payer.publicKey;
  const pool = stakePoolPda(baseMint);
  const kit = kitPda(baseMint);
  return {
    owner,
    launch: launchPda(baseMint),
    stakePool: pool,
    position: o.position ?? stakePositionPda(baseMint, owner),
    baseMint,
    ownerBase: o.ownerBase ?? getAssociatedTokenAddressSync(baseMint, owner),
    vault: o.vault ?? ata(pool, baseMint),
    kit,
    quoteMint: NATIVE_MINT,
    kitQuote: o.kitQuote ?? ata(kit),
    tokenProgram: TOKEN_PROGRAM_ID,
  };
}

function openStakePoolIx(c: Cranker, baseMint: PublicKey) {
  return c.labkit.methods
    .openStakePool()
    .accountsStrict({
      payer: c.payer.publicKey,
      launch: launchPda(baseMint),
      stakePool: stakePoolPda(baseMint),
      systemProgram: SystemProgram.programId,
    })
    .instruction();
}

function openDrawIx(c: Cranker, baseMint: PublicKey) {
  return c.labkit.methods
    .openDraw()
    .accountsStrict({
      payer: c.payer.publicKey,
      launch: launchPda(baseMint),
      stakePool: stakePoolPda(baseMint),
      draw: drawPda(baseMint),
      drawTree: drawTreePda(baseMint),
      systemProgram: SystemProgram.programId,
    })
    .instruction();
}

export async function openDraw(c: Cranker, baseMint: PublicKey): Promise<string> {
  return send(c, [await openDrawIx(c, baseMint)], []);
}

/** The draw tree a Draw launch's stake and unstake update, null for Stake launches. */
async function drawTreeOf(c: Cranker, baseMint: PublicKey): Promise<PublicKey | null> {
  const info = await c.connection.getAccountInfo(launchPda(baseMint));
  if (!info) throw new Error(`no launch for ${baseMint.toBase58()}`);
  return decodeLaunch(c, launchPda(baseMint), info.data).kit === KIT.draw ? drawTreePda(baseMint) : null;
}

export async function openStakePool(c: Cranker, baseMint: PublicKey): Promise<string> {
  return send(c, [await openStakePoolIx(c, baseMint)], []);
}

/** Opens the pool if needed, then stakes `amount` raw tokens. */
export async function stake(c: Cranker, baseMint: PublicKey, amount: bigint, o: StakeOverrides = {}): Promise<string> {
  const a = stakeAccounts(c, baseMint, o);
  const pre = [
    createAssociatedTokenAccountIdempotentInstruction(a.owner, ata(a.stakePool, baseMint), a.stakePool, baseMint),
    createAssociatedTokenAccountIdempotentInstruction(a.owner, ata(a.kit), a.kit, NATIVE_MINT),
  ];
  const drawTree = o.drawTree !== undefined ? o.drawTree : await drawTreeOf(c, baseMint);
  if (!(await c.connection.getAccountInfo(a.stakePool))) {
    pre.push(await (drawTree ? openDrawIx(c, baseMint) : openStakePoolIx(c, baseMint)));
  }
  const ix = await c.labkit.methods
    .stake(new BN(amount.toString()))
    .accountsStrict({
      owner: a.owner,
      launch: a.launch,
      stakePool: a.stakePool,
      drawTree: drawTree as PublicKey,
      position: a.position,
      baseMint,
      ownerBase: a.ownerBase,
      vault: a.vault,
      kit: a.kit,
      quoteMint: NATIVE_MINT,
      kitQuote: a.kitQuote,
      tokenProgram: TOKEN_PROGRAM_ID,
      systemProgram: SystemProgram.programId,
    })
    .instruction();
  return send(c, [...pre, ix], []);
}

export async function unstake(c: Cranker, baseMint: PublicKey, amount: bigint, o: StakeOverrides = {}): Promise<string> {
  const { owner, launch, stakePool, position, ownerBase, vault, kit, kitQuote } = stakeAccounts(c, baseMint, o);
  const drawTree = o.drawTree !== undefined ? o.drawTree : await drawTreeOf(c, baseMint);
  const ix = await c.labkit.methods
    .unstake(new BN(amount.toString()))
    .accountsStrict({
      owner,
      launch,
      stakePool,
      drawTree: drawTree as PublicKey,
      position,
      baseMint,
      ownerBase,
      vault,
      kit,
      quoteMint: NATIVE_MINT,
      kitQuote,
      tokenProgram: TOKEN_PROGRAM_ID,
    })
    .instruction();
  return send(c, [ix], []);
}

/** Claims into the owner's WSOL account, created first. */
export async function claimStake(c: Cranker, baseMint: PublicKey, o: StakeOverrides = {}): Promise<string> {
  const { owner, launch, stakePool, position, kit, kitQuote } = stakeAccounts(c, baseMint, o);
  const ownerQuote = o.ownerQuote ?? ata(owner);
  const ix = await c.labkit.methods
    .claimStakeRewards()
    .accountsStrict({
      owner,
      launch,
      stakePool,
      position,
      kit,
      quoteMint: NATIVE_MINT,
      kitQuote,
      ownerQuote,
      tokenProgram: TOKEN_PROGRAM_ID,
    })
    .instruction();
  return send(c, [createAssociatedTokenAccountIdempotentInstruction(owner, ata(owner), owner, NATIVE_MINT), ix], []);
}

/** The validator's clock, which can lag the wall clock. */
export async function chainTime(connection: Connection): Promise<number> {
  const info = await connection.getAccountInfo(SYSVAR_CLOCK_PUBKEY);
  if (!info) throw new Error("no clock sysvar");
  return Number(info.data.readBigInt64LE(32));
}

export async function waitForChain(connection: Connection, time: number): Promise<void> {
  while ((await chainTime(connection)) < time) await new Promise((resolve) => setTimeout(resolve, 1_000));
}

export async function readDraw(c: Cranker, baseMint: PublicKey) {
  const info = await c.connection.getAccountInfo(drawPda(baseMint));
  if (!info) throw new Error(`no draw for ${baseMint.toBase58()}`);
  const d = c.labkit.coder.accounts.decode("draw", info.data);
  const big = (v: BN) => BigInt(v.toString());
  return {
    round: big(d.round),
    pending: d.pending as boolean,
    seed: Buffer.from(d.seed as number[]),
    requestedAt: Number(d.requestedAt),
    lastDraw: Number(d.lastDraw),
    lastWinner: d.lastWinner as PublicKey,
    lastPrize: big(d.lastPrize),
    won: big(d.won),
  };
}

/** Gives the cloned ORAO program a network state that the test authority answers for. */
export async function ensureLocalVrf(env: Env): Promise<void> {
  await initLocalVrf(env.connection, env.authority, env.authority.publicKey);
}

/** Answers a draw's ORAO request as the test authority, as ORAO's oracles would. */
export function fulfillDraw(env: Env, seed: Uint8Array): Promise<string> {
  return fulfillLocal(env.connection, env.authority, seed);
}

export async function send(c: Cranker, ixs: TransactionInstruction[], signers: Keypair[]): Promise<string> {
  const tx = new Transaction().add(ComputeBudgetProgram.setComputeUnitLimit({ units: 400_000 }), ...ixs);
  return c.labkit.provider.sendAndConfirm!(tx, signers);
}

export function ata(owner: PublicKey, mint = NATIVE_MINT): PublicKey {
  return getAssociatedTokenAddressSync(mint, owner, true);
}

export async function airdrop(connection: Connection, to: PublicKey, sol: number): Promise<void> {
  const signature = await connection.requestAirdrop(to, sol * LAMPORTS_PER_SOL);
  await connection.confirmTransaction(signature, "confirmed");
}

export async function tokenAmount(connection: Connection, account: PublicKey): Promise<bigint> {
  const info = await connection.getAccountInfo(account);
  return info ? info.data.readBigUInt64LE(64) : 0n;
}

export async function supply(connection: Connection, mint: PublicKey): Promise<bigint> {
  const info = await connection.getAccountInfo(mint);
  if (!info) throw new Error(`no mint ${mint.toBase58()}`);
  return info.data.readBigUInt64LE(36);
}

export async function snapshot(connection: Connection, accounts: PublicKey[]) {
  return { accounts, amounts: await Promise.all(accounts.map((a) => tokenAmount(connection, a))) };
}

export async function deltas(connection: Connection, before: Awaited<ReturnType<typeof snapshot>>) {
  const after = await Promise.all(before.accounts.map((a) => tokenAmount(connection, a)));
  return after.map((amount, i) => amount - before.amounts[i]);
}

export function expect(condition: boolean, what: string): void {
  if (!condition) throw new Error(`failed: ${what}`);
}

/** Expects `action` to fail with an error whose text contains `code`. */
export async function expectError(what: string, code: string, action: () => Promise<unknown>): Promise<void> {
  let message: string | null = null;
  try {
    await action();
  } catch (err) {
    const logs = (err as { logs?: string[] }).logs ?? [];
    message = `${String(err)}\n${logs.join("\n")}`;
  }
  if (message === null) throw new Error(`failed: ${what} succeeded`);
  if (!message.includes(code)) throw new Error(`failed: ${what} did not fail with ${code}:\n${message}`);
  const reason = message.match(/Error Code: (\w+)/)?.[1] ?? message.match(/custom program error: (0x\w+)/)?.[1] ?? code;
  console.log(`  rejected: ${what} (${reason})`);
}

export function run(main: () => Promise<void>): void {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
