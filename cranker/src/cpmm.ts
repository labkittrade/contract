// Raydium CPMM pools that LaunchLab moved LabKit launches to. Their creator fees accrue on the
// pool for the launch's creator PDA, and CPMM's `collect_creator_fee_permissionless` lets anyone
// send them to the PDA's ATAs, where `split_creator_fees` pays them out.
import crypto from "node:crypto";
import { type AccountInfo, type Connection, PublicKey, SystemProgram, TransactionInstruction } from "@solana/web3.js";
import { ASSOCIATED_TOKEN_PROGRAM_ID, getAssociatedTokenAddressSync } from "@solana/spl-token";
import type { Cluster } from "./ids.js";

const CPMM_MAINNET = new PublicKey("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C");
const CPMM_DEVNET = new PublicKey("DRaycpLY18LhpbydsBWbVJtxpNv9oXPgjRSfpF2bWpYb");

/** Localnet clones the mainnet CPMM program. */
export function cpmmProgramId(cluster: Cluster): PublicKey {
  return cluster === "devnet" ? CPMM_DEVNET : CPMM_MAINNET;
}

// `PoolState` is zero-copy and packed, so fields sit at fixed offsets (raydium-cp-swap `states/pool.rs`).
const POOL_SIZE = 637;
const AMM_CONFIG = 8;
const POOL_CREATOR = 40;
const TOKEN_0_VAULT = 72;
const TOKEN_1_VAULT = 104;
const TOKEN_0_MINT = 168;
const TOKEN_1_MINT = 200;
const TOKEN_0_PROGRAM = 232;
const TOKEN_1_PROGRAM = 264;
const OBSERVATION = 296;
const PROTOCOL_FEES_0 = 341;
const FUND_FEES_0 = 357;
const CREATOR_FEES_0 = 397;
const CREATOR_FEES_1 = 405;

export interface CpmmPool {
  address: PublicKey;
  ammConfig: PublicKey;
  creator: PublicKey;
  vaults: [PublicKey, PublicKey];
  mints: [PublicKey, PublicKey];
  tokenPrograms: [PublicKey, PublicKey];
  observation: PublicKey;
  /** Creator fees booked per side, before CPMM keeps its share at collection. */
  creatorFees: [bigint, bigint];
  /** Protocol, fund and creator fees booked per side. They sit in the vaults but aren't liquidity. */
  bookedFees: [bigint, bigint];
}

const key = (data: Buffer, at: number) => new PublicKey(data.subarray(at, at + 32));

export function decodeCpmmPool(address: PublicKey, info: AccountInfo<Buffer> | null): CpmmPool | null {
  const d = info?.data;
  if (!d || d.length !== POOL_SIZE) return null;
  const booked = (side: 0 | 1) =>
    [PROTOCOL_FEES_0, FUND_FEES_0, CREATOR_FEES_0].reduce((sum, at) => sum + d.readBigUInt64LE(at + 8 * side), 0n);
  return {
    address,
    ammConfig: key(d, AMM_CONFIG),
    creator: key(d, POOL_CREATOR),
    vaults: [key(d, TOKEN_0_VAULT), key(d, TOKEN_1_VAULT)],
    mints: [key(d, TOKEN_0_MINT), key(d, TOKEN_1_MINT)],
    tokenPrograms: [key(d, TOKEN_0_PROGRAM), key(d, TOKEN_1_PROGRAM)],
    observation: key(d, OBSERVATION),
    creatorFees: [d.readBigUInt64LE(CREATOR_FEES_0), d.readBigUInt64LE(CREATOR_FEES_1)],
    bookedFees: [booked(0), booked(1)],
  };
}

/** The pool's SOL side and token side, by index. */
export function sides(pool: CpmmPool, quoteMint: PublicKey): { quote: 0 | 1; base: 0 | 1 } {
  return pool.mints[0].equals(quoteMint) ? { quote: 0, base: 1 } : { quote: 1, base: 0 };
}

export const cpmmAuthorityPda = (cluster: Cluster) =>
  PublicKey.findProgramAddressSync([Buffer.from("vault_and_lp_mint_auth_seed")], cpmmProgramId(cluster))[0];

const SWAP_BASE_INPUT = Buffer.from([143, 190, 90, 218, 196, 30, 51, 222]);

/** CPMM `swap_base_input` from `input` into `output`, both owned by `payer`. */
export function swapIx(
  cluster: Cluster,
  pool: CpmmPool,
  payer: PublicKey,
  inputSide: 0 | 1,
  accounts: { input: PublicKey; output: PublicKey },
  amountIn: bigint,
  minimumOut: bigint,
): TransactionInstruction {
  const outputSide = (1 - inputSide) as 0 | 1;
  const keys = [
    { pubkey: payer, isSigner: true, isWritable: false },
    { pubkey: cpmmAuthorityPda(cluster), isSigner: false, isWritable: false },
    { pubkey: pool.ammConfig, isSigner: false, isWritable: false },
    { pubkey: pool.address, isSigner: false, isWritable: true },
    { pubkey: accounts.input, isSigner: false, isWritable: true },
    { pubkey: accounts.output, isSigner: false, isWritable: true },
    { pubkey: pool.vaults[inputSide], isSigner: false, isWritable: true },
    { pubkey: pool.vaults[outputSide], isSigner: false, isWritable: true },
    { pubkey: pool.tokenPrograms[inputSide], isSigner: false, isWritable: false },
    { pubkey: pool.tokenPrograms[outputSide], isSigner: false, isWritable: false },
    { pubkey: pool.mints[inputSide], isSigner: false, isWritable: false },
    { pubkey: pool.mints[outputSide], isSigner: false, isWritable: false },
    { pubkey: pool.observation, isSigner: false, isWritable: true },
  ];
  const data = Buffer.alloc(24);
  SWAP_BASE_INPUT.copy(data, 0);
  data.writeBigUInt64LE(amountIn, 8);
  data.writeBigUInt64LE(minimumOut, 16);
  return new TransactionInstruction({ programId: cpmmProgramId(cluster), keys, data });
}

function sorted(a: PublicKey, b: PublicKey): [PublicKey, PublicKey] {
  return a.toBuffer().compare(b.toBuffer()) < 0 ? [a, b] : [b, a];
}

/**
 * The pool LaunchLab moved a launch to. Anyone can open a CPMM pool for the same mints, so only
 * a pool whose creator is the launch's creator PDA counts. LaunchLab uses the config's PDA pool
 * unless it's taken, and a fresh keypair otherwise, hence the scan.
 */
export async function findCpmmPool(
  connection: Connection,
  cluster: Cluster,
  baseMint: PublicKey,
  quoteMint: PublicKey,
  cpswapConfig: PublicKey,
  creator: PublicKey,
): Promise<CpmmPool | null> {
  const program = cpmmProgramId(cluster);
  const [token0, token1] = sorted(baseMint, quoteMint);
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("pool"), cpswapConfig.toBuffer(), token0.toBuffer(), token1.toBuffer()],
    program,
  );
  const atPda = decodeCpmmPool(pda, await connection.getAccountInfo(pda));
  if (atPda?.creator.equals(creator)) return atPda;
  const found = await connection.getProgramAccounts(program, {
    filters: [
      { dataSize: POOL_SIZE },
      { memcmp: { offset: TOKEN_0_MINT, bytes: token0.toBase58() } },
      { memcmp: { offset: TOKEN_1_MINT, bytes: token1.toBase58() } },
      { memcmp: { offset: POOL_CREATOR, bytes: creator.toBase58() } },
    ],
  });
  const first = found[0];
  return first ? decodeCpmmPool(first.pubkey, first.account) : null;
}

const COLLECT_CREATOR_FEE_PERMISSIONLESS = crypto
  .createHash("sha256")
  .update("global:collect_creator_fee_permissionless")
  .digest()
  .subarray(0, 8);

/** Sends a pool's creator fees to its creator's ATAs, creating them at the payer's expense. */
export function collectCreatorFeeIx(cluster: Cluster, payer: PublicKey, pool: CpmmPool): TransactionInstruction {
  const program = cpmmProgramId(cluster);
  const pda = (...seeds: Buffer[]) => PublicKey.findProgramAddressSync(seeds, program)[0];
  const creatorToken = (i: 0 | 1) => getAssociatedTokenAddressSync(pool.mints[i], pool.creator, true, pool.tokenPrograms[i]);
  const keys = [
    { pubkey: payer, isSigner: true, isWritable: true },
    { pubkey: pool.creator, isSigner: false, isWritable: false },
    { pubkey: pda(Buffer.from("vault_and_lp_mint_auth_seed")), isSigner: false, isWritable: false },
    { pubkey: pool.address, isSigner: false, isWritable: true },
    { pubkey: pool.vaults[0], isSigner: false, isWritable: true },
    { pubkey: pool.vaults[1], isSigner: false, isWritable: true },
    { pubkey: pool.mints[0], isSigner: false, isWritable: false },
    { pubkey: pool.mints[1], isSigner: false, isWritable: false },
    { pubkey: creatorToken(0), isSigner: false, isWritable: true },
    { pubkey: creatorToken(1), isSigner: false, isWritable: true },
    { pubkey: pool.tokenPrograms[0], isSigner: false, isWritable: false },
    { pubkey: pool.tokenPrograms[1], isSigner: false, isWritable: false },
    { pubkey: ASSOCIATED_TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
    { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    { pubkey: pool.ammConfig, isSigner: false, isWritable: false },
    {
      pubkey: pda(Buffer.from("creator_fee_share"), pool.creator.toBuffer(), pool.ammConfig.toBuffer()),
      isSigner: false,
      isWritable: false,
    },
  ];
  return new TransactionInstruction({ programId: program, keys, data: Buffer.from(COLLECT_CREATOR_FEE_PERMISSIONLESS) });
}
