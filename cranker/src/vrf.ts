// ORAO VRF from its IDL, plus a stand-in oracle for the local validator, where ORAO's
// own oracles can't answer. Localnet clones the ORAO program; `initLocalVrf` gives it a
// network configuration whose only fulfillment authority is a local key.
import { createHash, createPrivateKey, sign } from "node:crypto";
import { utils } from "@coral-xyz/anchor";
import {
  type Connection,
  Ed25519Program,
  type Keypair,
  PublicKey,
  SYSVAR_INSTRUCTIONS_PUBKEY,
  SystemProgram,
  Transaction,
  TransactionInstruction,
  sendAndConfirmTransaction,
} from "@solana/web3.js";
import { ORAO_VRF_PROGRAM_ID, oraoNetworkStatePda, oraoRandomnessPda } from "./ids.js";

const INIT_NETWORK = Buffer.from([104, 231, 73, 121, 240, 0, 5, 148]);
const FULFILL_V2 = Buffer.from([164, 227, 237, 84, 112, 111, 147, 196]);
const RANDOMNESS_V2 = Buffer.from([139, 239, 184, 215, 227, 86, 191, 226]);
/** `NetworkState`: discriminator, then `config.authority`, `config.treasury`. */
const TREASURY_OFFSET = 8 + 32;
/** `RandomnessV2`: discriminator, `RequestAccount` variant, then client and seed. */
const VARIANT_OFFSET = 8;
const CLIENT_OFFSET = 9;
const RANDOMNESS_OFFSET = 9 + 32 + 32;
const PENDING = 0;
const FULFILLED = 1;
/** The mainnet request fee. */
const LOCAL_FEE = 500_000n;
/** PKCS#8 wrapping of a raw Ed25519 private key, for Node's crypto. */
const ED25519_PKCS8_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");

function ed25519Sign(message: Uint8Array, keypair: Keypair): Buffer {
  const key = createPrivateKey({
    key: Buffer.concat([ED25519_PKCS8_PREFIX, keypair.secretKey.subarray(0, 32)]),
    format: "der",
    type: "pkcs8",
  });
  return sign(null, message, key);
}

export async function oraoTreasury(connection: Connection): Promise<PublicKey> {
  const state = await connection.getAccountInfo(oraoNetworkStatePda());
  if (!state) throw new Error("ORAO VRF has no network state on this cluster");
  return new PublicKey(state.data.subarray(TREASURY_OFFSET, TREASURY_OFFSET + 32));
}

/** The randomness for `seed`, or null while the request is pending or absent. */
export async function readRandomness(connection: Connection, seed: Uint8Array): Promise<Buffer | null> {
  const info = await connection.getAccountInfo(oraoRandomnessPda(seed));
  if (!info || !info.owner.equals(ORAO_VRF_PROGRAM_ID)) return null;
  const d = info.data;
  if (!d.subarray(0, 8).equals(RANDOMNESS_V2) || d[VARIANT_OFFSET] !== FULFILLED) return null;
  return Buffer.from(d.subarray(RANDOMNESS_OFFSET, RANDOMNESS_OFFSET + 64));
}

/** Pick `n` of a draw: a point in `0..total`, as the program derives it. */
export function drawPoint(randomness: Buffer, n: number, total: bigint): bigint {
  const hash = createHash("sha256").update(randomness).update(Buffer.from([n])).digest();
  let value = 0n;
  for (let i = 15; i >= 0; i--) value = (value << 8n) | BigInt(hash[i]);
  return value % total;
}

/** Localnet: creates ORAO's network state with `authority` as the only fulfillment authority. */
export async function initLocalVrf(connection: Connection, payer: Keypair, authority: PublicKey): Promise<boolean> {
  if (await connection.getAccountInfo(oraoNetworkStatePda())) return false;
  const fee = Buffer.alloc(8);
  fee.writeBigUInt64LE(LOCAL_FEE);
  const count = Buffer.alloc(4);
  count.writeUInt32LE(1);
  const data = Buffer.concat([INIT_NETWORK, fee, payer.publicKey.toBuffer(), count, authority.toBuffer(), Buffer.from([0])]);
  const ix = new TransactionInstruction({
    programId: ORAO_VRF_PROGRAM_ID,
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: oraoNetworkStatePda(), isSigner: false, isWritable: true },
      { pubkey: payer.publicKey, isSigner: false, isWritable: false },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
    data,
  });
  await sendAndConfirmTransaction(connection, new Transaction().add(ix), [payer]);
  return true;
}

/** Localnet: answers one pending request as the local fulfillment authority. */
export async function fulfillLocal(connection: Connection, authority: Keypair, seed: Uint8Array): Promise<string> {
  const request = oraoRandomnessPda(seed);
  const info = await connection.getAccountInfo(request);
  if (!info) throw new Error(`no ORAO request for seed ${Buffer.from(seed).toString("hex")}`);
  const client = new PublicKey(info.data.subarray(CLIENT_OFFSET, CLIENT_OFFSET + 32));
  const signature = ed25519Sign(seed, authority);
  const verify = Ed25519Program.createInstructionWithPublicKey({
    publicKey: authority.publicKey.toBytes(),
    message: seed,
    signature,
  });
  const fulfill = new TransactionInstruction({
    programId: ORAO_VRF_PROGRAM_ID,
    keys: [
      { pubkey: authority.publicKey, isSigner: true, isWritable: true },
      { pubkey: SYSVAR_INSTRUCTIONS_PUBKEY, isSigner: false, isWritable: false },
      { pubkey: oraoNetworkStatePda(), isSigner: false, isWritable: true },
      { pubkey: request, isSigner: false, isWritable: true },
      { pubkey: client, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
    data: FULFILL_V2,
  });
  return sendAndConfirmTransaction(connection, new Transaction().add(verify, fulfill), [authority]);
}

/** Localnet: seeds of every request still waiting for randomness. */
export async function pendingRequests(connection: Connection): Promise<Buffer[]> {
  const accounts = await connection.getProgramAccounts(ORAO_VRF_PROGRAM_ID, {
    filters: [{ memcmp: { offset: 0, bytes: utils.bytes.bs58.encode(RANDOMNESS_V2) } }],
  });
  return accounts
    .filter(({ account }) => account.data[VARIANT_OFFSET] === PENDING)
    .map(({ account }) => Buffer.from(account.data.subarray(CLIENT_OFFSET + 32, CLIENT_OFFSET + 64)));
}
