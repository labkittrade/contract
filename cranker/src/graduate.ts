// Localnet only: fills a launch's curve and moves it to Raydium CPMM the way LaunchLab's
// migrate wallet does on mainnet. `scripts/local-validator.sh` makes the local key that wallet.
// Usage: npm run graduate -- <base mint>
import {
  type AddressLookupTableAccount,
  AddressLookupTableProgram,
  ComputeBudgetProgram,
  Keypair,
  LAMPORTS_PER_SOL,
  PublicKey,
  SYSVAR_CLOCK_PUBKEY,
  SYSVAR_RENT_PUBKEY,
  SystemProgram,
  type TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
} from "@solana/web3.js";
import {
  ASSOCIATED_TOKEN_PROGRAM_ID,
  NATIVE_MINT,
  TOKEN_2022_PROGRAM_ID,
  TOKEN_PROGRAM_ID,
  getAssociatedTokenAddressSync,
} from "@solana/spl-token";
import { fileURLToPath } from "node:url";
import type { Cranker } from "./crank.js";
import { cpmmProgramId } from "./cpmm.js";
import { METADATA_PROGRAM_ID, metadataPda, poolStatePda, poolVaultPda, vaultAuthorityPda } from "./ids.js";
import { type Env, airdrop, buy, openEnv, run } from "./test-env.js";

const LOCK_PROGRAM = new PublicKey("LockrWmn6K5twhz3y9w1dQERbmgSaRkfnTeTKbpofwE");
const CPMM_CREATE_POOL_FEE = new PublicKey("DNXgeM9EiiaAbaWvwjHj9fQQLAX5ZsfHyvmYUNRAdNC8");
const CPMM_OPEN_TIME_OFFSET = 373;
const CURVE = 0;
const MIGRATE_PENDING = 1;

interface Pool {
  status: number;
  globalConfig: PublicKey;
  platformConfig: PublicKey;
  creator: PublicKey;
  realQuote: bigint;
  totalQuoteFundRaising: bigint;
}

async function readPool(c: Cranker, address: PublicKey): Promise<Pool> {
  const info = await c.connection.getAccountInfo(address);
  if (!info) throw new Error(`no LaunchLab pool at ${address.toBase58()}`);
  const s = c.launchlab.coder.accounts.decode("poolState", info.data);
  return {
    status: s.status as number,
    globalConfig: s.globalConfig as PublicKey,
    platformConfig: s.platformConfig as PublicKey,
    creator: s.creator as PublicKey,
    realQuote: BigInt(s.realQuote.toString()),
    totalQuoteFundRaising: BigInt(s.totalQuoteFundRaising.toString()),
  };
}

async function migrate(c: Cranker, baseMint: PublicKey, address: PublicKey, pool: Pool): Promise<PublicKey> {
  const ll = c.launchlab.programId;
  const cpmm = cpmmProgramId(c.cluster);
  const payer = c.payer.publicKey;
  const platformInfo = await c.connection.getAccountInfo(pool.platformConfig);
  if (!platformInfo) throw new Error("platform config missing");
  const platform = c.launchlab.coder.accounts.decode("platformConfig", platformInfo.data);
  const cpswapConfig = platform.cpswapConfig as PublicKey;
  const cpCreator = platform.platformCpCreator as PublicKey;

  const cp = (...seeds: Buffer[]) => PublicKey.findProgramAddressSync(seeds, cpmm)[0];
  const [token0, token1] =
    baseMint.toBuffer().compare(NATIVE_MINT.toBuffer()) < 0 ? [baseMint, NATIVE_MINT] : [NATIVE_MINT, baseMint];
  const cpswapPool = cp(Buffer.from("pool"), cpswapConfig.toBuffer(), token0.toBuffer(), token1.toBuffer());
  const lpMint = cp(Buffer.from("pool_lp_mint"), cpswapPool.toBuffer());
  const lockAuthority = PublicKey.findProgramAddressSync([Buffer.from("lock_cp_authority_seed")], LOCK_PROGRAM)[0];
  const authority = vaultAuthorityPda(ll);
  const supportMint = (mint: PublicKey) => cp(Buffer.from("support_mint"), mint.toBuffer());

  const nftMint = Keypair.generate();
  const nftWallet = platform.platformNftWallet as PublicKey;
  const lockedLiquidity = PublicKey.findProgramAddressSync(
    [Buffer.from("locked_liquidity"), nftMint.publicKey.toBuffer()],
    LOCK_PROGRAM,
  )[0];

  const ix = await c.launchlab.methods
    .migrateToCpswap()
    .accountsStrict({
      payer,
      baseMint,
      quoteMint: NATIVE_MINT,
      platformConfig: pool.platformConfig,
      cpswapProgram: cpmm,
      cpswapPool,
      cpswapAuthority: cp(Buffer.from("vault_and_lp_mint_auth_seed")),
      cpswapLpMint: lpMint,
      cpswapBaseVault: cp(Buffer.from("pool_vault"), cpswapPool.toBuffer(), baseMint.toBuffer()),
      cpswapQuoteVault: cp(Buffer.from("pool_vault"), cpswapPool.toBuffer(), NATIVE_MINT.toBuffer()),
      cpswapConfig,
      cpswapCreatePoolFee: CPMM_CREATE_POOL_FEE,
      cpswapObservation: cp(Buffer.from("observation"), cpswapPool.toBuffer()),
      lockProgram: LOCK_PROGRAM,
      lockAuthority,
      lockLpVault: getAssociatedTokenAddressSync(lpMint, lockAuthority, true),
      authority,
      poolState: address,
      globalConfig: pool.globalConfig,
      baseVault: poolVaultPda(ll, address, baseMint),
      quoteVault: poolVaultPda(ll, address, NATIVE_MINT),
      poolLpToken: getAssociatedTokenAddressSync(lpMint, authority, true),
      tokenProgram: TOKEN_PROGRAM_ID,
      tokenProgram2022: TOKEN_2022_PROGRAM_ID,
      associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
      systemProgram: SystemProgram.programId,
      rentProgram: SYSVAR_RENT_PUBKEY,
      metadataProgram: METADATA_PROGRAM_ID,
    })
    .remainingAccounts([
      { pubkey: nftWallet, isSigner: false, isWritable: true },
      { pubkey: nftMint.publicKey, isSigner: true, isWritable: true },
      { pubkey: getAssociatedTokenAddressSync(nftMint.publicKey, nftWallet, true), isSigner: false, isWritable: true },
      { pubkey: lockedLiquidity, isSigner: false, isWritable: true },
      { pubkey: metadataPda(nftMint.publicKey), isSigner: false, isWritable: true },
      { pubkey: pool.creator, isSigner: false, isWritable: true },
      { pubkey: cp(Buffer.from("permission"), authority.toBuffer()), isSigner: false, isWritable: false },
      { pubkey: cpCreator, isSigner: false, isWritable: false },
      { pubkey: supportMint(token0), isSigner: false, isWritable: false },
      { pubkey: supportMint(token1), isSigner: false, isWritable: false },
    ])
    .instruction();

  // Too many accounts for a legacy transaction, so put the fixed ones in a lookup table.
  const slot = await c.connection.getSlot("finalized");
  const [createTable, table] = AddressLookupTableProgram.createLookupTable({ authority: payer, payer, recentSlot: slot });
  const signers = new Set([payer.toBase58(), nftMint.publicKey.toBase58()]);
  const addresses = [...new Set([ix.programId, ...ix.keys.map((k) => k.pubkey)].map((k) => k.toBase58()))]
    .filter((k) => !signers.has(k))
    .map((k) => new PublicKey(k));
  const extend = (part: PublicKey[]) =>
    AddressLookupTableProgram.extendLookupTable({ lookupTable: table, authority: payer, payer, addresses: part });
  await sendV0(c, [createTable, extend(addresses.slice(0, 20))], [c.payer], []);
  await sendV0(c, [extend(addresses.slice(20))], [c.payer], []);
  await new Promise((resolve) => setTimeout(resolve, 1_000));
  const lookup = (await c.connection.getAddressLookupTable(table)).value;
  if (!lookup) throw new Error("lookup table not active");

  const signature = await sendV0(
    c,
    [ComputeBudgetProgram.setComputeUnitLimit({ units: 1_000_000 }), ix],
    [c.payer, nftMint],
    [lookup],
  );
  console.log(`migrated to ${cpswapPool.toBase58()} ${signature}`);
  // CPMM rejects swaps until the clock passes the pool's `open_time`.
  const openTime = (await c.connection.getAccountInfo(cpswapPool))!.data.readBigInt64LE(CPMM_OPEN_TIME_OFFSET);
  while ((await c.connection.getAccountInfo(SYSVAR_CLOCK_PUBKEY))!.data.readBigInt64LE(32) <= openTime) {
    await new Promise((resolve) => setTimeout(resolve, 400));
  }
  return cpswapPool;
}

async function sendV0(
  c: Cranker,
  instructions: TransactionInstruction[],
  signers: Keypair[],
  lookups: AddressLookupTableAccount[],
): Promise<string> {
  const { blockhash, lastValidBlockHeight } = await c.connection.getLatestBlockhash();
  const message = new TransactionMessage({ payerKey: c.payer.publicKey, recentBlockhash: blockhash, instructions })
    .compileToV0Message(lookups);
  const tx = new VersionedTransaction(message);
  tx.sign(signers);
  const signature = await c.connection.sendTransaction(tx);
  const result = await c.connection.confirmTransaction({ signature, blockhash, lastValidBlockHeight }, "confirmed");
  if (result.value.err) throw new Error(`${signature} failed: ${JSON.stringify(result.value.err)}`);
  return signature;
}

/** Buys out the curve if it still trades. LaunchLab stops the buy at the raise target. */
export async function fillCurve(env: Env, baseMint: PublicKey): Promise<void> {
  const migrator = env.open(env.authority);
  const address = poolStatePda(migrator.launchlab.programId, baseMint, NATIVE_MINT);
  const pool = await readPool(migrator, address);
  if (pool.status !== CURVE) return;
  const left = pool.totalQuoteFundRaising - pool.realQuote;
  const whale = Keypair.generate();
  const lamports = (left * 105n) / 100n + BigInt(LAMPORTS_PER_SOL);
  await airdrop(env.connection, whale.publicKey, Number(lamports / BigInt(LAMPORTS_PER_SOL)) + 2);
  await buy(env.open(whale), pool.platformConfig, baseMint, 0, null, lamports);
  console.log(`filled the curve with ${whale.publicKey.toBase58()}`);
}

/** Fills the curve and moves it to CPMM. Returns the pool. */
export async function graduate(env: Env, baseMint: PublicKey): Promise<PublicKey> {
  await fillCurve(env, baseMint);
  const migrator = env.open(env.authority);
  const address = poolStatePda(migrator.launchlab.programId, baseMint, NATIVE_MINT);
  const pool = await readPool(migrator, address);
  if (pool.status !== MIGRATE_PENDING) throw new Error(`pool status ${pool.status}, nothing to migrate`);
  return migrate(migrator, baseMint, address, pool);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  run(async () => {
    const [mint] = process.argv.slice(2);
    if (!mint) throw new Error("usage: npm run graduate -- <base mint>");
    await graduate(await openEnv(), new PublicKey(mint));
  });
}
