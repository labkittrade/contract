// Builds a LabKit launch in two transactions, both signed by the payer and the base mint
// keypair. `create_launch` makes the launch's own LaunchLab platform and fixes its kit. Then
// LaunchLab `initialize_v2` on that platform with the creator PDA as `creator`, and
// `register_launch`. A launchpad frontend builds the same pair.
import { BN } from "@coral-xyz/anchor";
import {
  type Keypair,
  type PublicKey,
  SystemProgram,
  SYSVAR_RENT_PUBKEY,
  type TransactionInstruction,
} from "@solana/web3.js";
import { TOKEN_PROGRAM_ID } from "@solana/spl-token";
import type { Cranker, LabkitConfig } from "./crank.js";
import {
  METADATA_PROGRAM_ID,
  configPda,
  creatorPda,
  eventAuthorityPda,
  kitPda,
  launchPda,
  launchPlatformPda,
  metadataPda,
  platformAdminPda,
  poolStatePda,
  poolVaultPda,
  vaultAuthorityPda,
  vestingRecordPda,
} from "./ids.js";

export const KIT = { burn: 0, draw: 1, floor: 2, graduate: 3, stake: 4 } as const;
export type KitName = keyof typeof KIT;

export interface VestingParams {
  /** Base tokens held out of the curve for beneficiaries, capped by the config's `max_lock_rate`. */
  totalLockedAmount: BN;
  /** Seconds after the curve fills before anything unlocks. */
  cliffPeriod: BN;
  /** Seconds of linear unlock after the cliff. */
  unlockPeriod: BN;
}

export interface LaunchParams {
  globalConfig: PublicKey;
  quoteMint: PublicKey;
  quoteTokenProgram: PublicKey;
  /** Receives the creator's share of creator fees and allocates vesting. */
  recipient: PublicKey;
  kit: number;
  /** Floor only: how far under the high, in bps, the price must fall before the kit buys. */
  drawdownBps: number;
  name: string;
  symbol: string;
  uri: string;
  decimals: number;
  supply: BN;
  totalBaseSell: BN;
  totalQuoteFundRaising: BN;
  vesting?: VestingParams;
}

/** The two transactions of a launch, in order. */
export async function launchInstructions(
  c: Cranker,
  config: LabkitConfig,
  params: LaunchParams,
  baseMint: Keypair,
): Promise<[TransactionInstruction[], TransactionInstruction[]]> {
  const mint = baseMint.publicKey;
  return [
    [await createLaunchIx(c, config, params, mint)],
    [await initializeLaunchIx(c, params, mint, creatorPda(mint)), await registerLaunchIx(c, params, mint)],
  ];
}

export async function createLaunchIx(
  c: Cranker,
  config: LabkitConfig,
  params: LaunchParams,
  baseMint: PublicKey,
): Promise<TransactionInstruction> {
  return c.labkit.methods
    .createLaunch({ recipient: params.recipient, kit: params.kit, drawdownBps: params.drawdownBps })
    .accountsStrict({
      payer: c.payer.publicKey,
      baseMint,
      config: configPda(),
      launch: launchPda(baseMint),
      creator: creatorPda(baseMint),
      kit: kitPda(baseMint),
      platformAdmin: platformAdminPda(baseMint),
      platformConfig: launchPlatformPda(c.launchlab.programId, baseMint),
      nftWallet: config.platform.nftWallet,
      cpswapConfig: config.platform.cpswapConfig,
      treasury: config.treasury,
      launchlabProgram: c.launchlab.programId,
      systemProgram: SystemProgram.programId,
    })
    .instruction();
}

export async function initializeLaunchIx(
  c: Cranker,
  params: LaunchParams,
  baseMint: PublicKey,
  creator: PublicKey,
  platform = launchPlatformPda(c.launchlab.programId, baseMint),
  ammFeeOn: { quoteToken: object } | { bothToken: object } = { quoteToken: {} },
): Promise<TransactionInstruction> {
  const ll = c.launchlab.programId;
  const pool = poolStatePda(ll, baseMint, params.quoteMint);
  const vesting = params.vesting ?? {
    totalLockedAmount: new BN(0),
    cliffPeriod: new BN(0),
    unlockPeriod: new BN(0),
  };
  return c.launchlab.methods
    .initializeV2(
      { decimals: params.decimals, name: params.name, symbol: params.symbol, uri: params.uri },
      {
        constant: {
          data: {
            supply: params.supply,
            totalBaseSell: params.totalBaseSell,
            totalQuoteFundRaising: params.totalQuoteFundRaising,
            migrateType: 1,
          },
        },
      },
      vesting,
      ammFeeOn,
    )
    .accountsStrict({
      payer: c.payer.publicKey,
      creator,
      globalConfig: params.globalConfig,
      platformConfig: platform,
      authority: vaultAuthorityPda(ll),
      poolState: pool,
      baseMint,
      quoteMint: params.quoteMint,
      baseVault: poolVaultPda(ll, pool, baseMint),
      quoteVault: poolVaultPda(ll, pool, params.quoteMint),
      metadataAccount: metadataPda(baseMint),
      baseTokenProgram: TOKEN_PROGRAM_ID,
      quoteTokenProgram: params.quoteTokenProgram,
      metadataProgram: METADATA_PROGRAM_ID,
      systemProgram: SystemProgram.programId,
      rentProgram: SYSVAR_RENT_PUBKEY,
      eventAuthority: eventAuthorityPda(ll),
      program: ll,
    })
    .instruction();
}

export async function registerLaunchIx(
  c: Cranker,
  params: LaunchParams,
  baseMint: PublicKey,
): Promise<TransactionInstruction> {
  return c.labkit.methods
    .registerLaunch()
    .accountsStrict({
      launch: launchPda(baseMint),
      creator: creatorPda(baseMint),
      poolState: poolStatePda(c.launchlab.programId, baseMint, params.quoteMint),
    })
    .instruction();
}

/** Signed by the launch's recipient, who also lends the vesting record's rent. */
export async function createCreatorVestingIx(
  c: Cranker,
  recipient: PublicKey,
  baseMint: PublicKey,
  quoteMint: PublicKey,
  beneficiary: PublicKey,
  shareAmount: BN,
): Promise<TransactionInstruction> {
  const ll = c.launchlab.programId;
  const pool = poolStatePda(ll, baseMint, quoteMint);
  return c.labkit.methods
    .createCreatorVesting(shareAmount)
    .accountsStrict({
      recipient,
      launch: launchPda(baseMint),
      creator: creatorPda(baseMint),
      beneficiary,
      poolState: pool,
      vestingRecord: vestingRecordPda(ll, pool, beneficiary),
      launchlabProgram: ll,
      systemProgram: SystemProgram.programId,
    })
    .instruction();
}
