import { PublicKey } from "@solana/web3.js";

export type Cluster = "mainnet" | "devnet" | "localnet";

export const LABKIT_PROGRAM_ID = new PublicKey("133fUn4mjvLVKoe6oR7BWwAiF4qtFgmkg35BdegqgMG3");

const LAUNCHLAB_MAINNET = new PublicKey("LanMV9sAd7wArD4vJFi2qDdfnVhFxYSUg6eADduJ3uj");
const LAUNCHLAB_DEVNET = new PublicKey("DRay6fNdQ5J82H7xV6uq2aV3mNrUZ1J4PgSKsWgptcm6");

/** Localnet clones the mainnet LaunchLab program. */
export function launchlabProgramId(cluster: Cluster): PublicKey {
  return cluster === "devnet" ? LAUNCHLAB_DEVNET : LAUNCHLAB_MAINNET;
}

/** CPMM `AmmConfig` index 11 on mainnet: 0.25% trade fee and a 1% creator fee. Devnet needs `--cpswap-config`. */
export const CPMM_CONFIG_11_MAINNET = new PublicKey("CRRS5ieQmBrZjWhcj99JuGrT5tyuWDaGAXLXLFjbAtjQ");
/** Mainnet LaunchLab SOL constant-product `GlobalConfig`, index 0. */
export const SOL_GLOBAL_CONFIG_MAINNET = new PublicKey(
  "6s1xP3hpbAfFoNtUNF8mfHsjr2Bd97JxFJRWLbL6aHuX",
);
export const METADATA_PROGRAM_ID = new PublicKey("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s");
const BPF_UPGRADEABLE_LOADER = new PublicKey("BPFLoaderUpgradeab1e11111111111111111111111");

/** Byte offset of `amount` in an SPL / Token-2022 token account. */
export const TOKEN_AMOUNT_OFFSET = 64;

function pda(seeds: (Buffer | Uint8Array)[], program: PublicKey): PublicKey {
  return PublicKey.findProgramAddressSync(seeds, program)[0];
}

export const configPda = () => pda([Buffer.from("config")], LABKIT_PROGRAM_ID);
export const feeWalletPda = () => pda([Buffer.from("fee_wallet")], LABKIT_PROGRAM_ID);
export const launchPda = (baseMint: PublicKey) =>
  pda([Buffer.from("launch"), baseMint.toBuffer()], LABKIT_PROGRAM_ID);
/** The LaunchLab `creator` of a LabKit launch. */
export const creatorPda = (baseMint: PublicKey) =>
  pda([Buffer.from("creator"), baseMint.toBuffer()], LABKIT_PROGRAM_ID);
/** The launch's LaunchLab platform fee wallet, which holds the kit's SOL as WSOL. */
export const kitPda = (baseMint: PublicKey) => pda([Buffer.from("kit"), baseMint.toBuffer()], LABKIT_PROGRAM_ID);
/** A Stake launch's pool, which owns the staked tokens. */
export const stakePoolPda = (baseMint: PublicKey) =>
  pda([Buffer.from("stake_pool"), baseMint.toBuffer()], LABKIT_PROGRAM_ID);
export const stakePositionPda = (baseMint: PublicKey, owner: PublicKey) =>
  pda([Buffer.from("stake"), baseMint.toBuffer(), owner.toBuffer()], LABKIT_PROGRAM_ID);
/** A Draw launch's schedule and pending request. */
export const drawPda = (baseMint: PublicKey) => pda([Buffer.from("draw"), baseMint.toBuffer()], LABKIT_PROGRAM_ID);
/** A Draw launch's Fenwick tree of stakes. */
export const drawTreePda = (baseMint: PublicKey) =>
  pda([Buffer.from("draw_tree"), baseMint.toBuffer()], LABKIT_PROGRAM_ID);

/** ORAO VRF, the same program on every cluster. Localnet clones it from mainnet. */
export const ORAO_VRF_PROGRAM_ID = new PublicKey("VRFzZoJdhFWL8rkvu87LpKM3RbcVezpMEc6X5GVDr7y");
export const oraoNetworkStatePda = () => pda([Buffer.from("orao-vrf-network-configuration")], ORAO_VRF_PROGRAM_ID);
export const oraoRandomnessPda = (seed: Uint8Array) =>
  pda([Buffer.from("orao-vrf-randomness-request"), seed], ORAO_VRF_PROGRAM_ID);

/** Admin of the launch's own LaunchLab platform. */
export const platformAdminPda = (baseMint: PublicKey) =>
  pda([Buffer.from("platform_admin"), baseMint.toBuffer()], LABKIT_PROGRAM_ID);
export const programDataPda = (program: PublicKey) => pda([program.toBuffer()], BPF_UPGRADEABLE_LOADER);

export const platformConfigPda = (launchlab: PublicKey, admin: PublicKey) =>
  pda([Buffer.from("platform_config"), admin.toBuffer()], launchlab);
/** The LaunchLab platform `create_launch` made for this mint. */
export const launchPlatformPda = (launchlab: PublicKey, baseMint: PublicKey) =>
  platformConfigPda(launchlab, platformAdminPda(baseMint));
export const platformFeeVaultPda = (launchlab: PublicKey, platform: PublicKey, quoteMint: PublicKey) =>
  pda([platform.toBuffer(), quoteMint.toBuffer()], launchlab);
export const feeVaultAuthorityPda = (launchlab: PublicKey) =>
  pda([Buffer.from("platform_fee_vault_auth_seed")], launchlab);
export const creatorFeeVaultPda = (launchlab: PublicKey, creator: PublicKey, quoteMint: PublicKey) =>
  pda([creator.toBuffer(), quoteMint.toBuffer()], launchlab);
export const creatorFeeVaultAuthorityPda = (launchlab: PublicKey) =>
  pda([Buffer.from("creator_fee_vault_auth_seed")], launchlab);
export const vaultAuthorityPda = (launchlab: PublicKey) => pda([Buffer.from("vault_auth_seed")], launchlab);
export const poolStatePda = (launchlab: PublicKey, baseMint: PublicKey, quoteMint: PublicKey) =>
  pda([Buffer.from("pool"), baseMint.toBuffer(), quoteMint.toBuffer()], launchlab);
export const poolVaultPda = (launchlab: PublicKey, pool: PublicKey, mint: PublicKey) =>
  pda([Buffer.from("pool_vault"), pool.toBuffer(), mint.toBuffer()], launchlab);
export const vestingRecordPda = (launchlab: PublicKey, pool: PublicKey, beneficiary: PublicKey) =>
  pda([Buffer.from("pool_vesting"), pool.toBuffer(), beneficiary.toBuffer()], launchlab);
export const eventAuthorityPda = (program: PublicKey) => pda([Buffer.from("__event_authority")], program);
export const metadataPda = (mint: PublicKey) =>
  pda([Buffer.from("metadata"), METADATA_PROGRAM_ID.toBuffer(), mint.toBuffer()], METADATA_PROGRAM_ID);
