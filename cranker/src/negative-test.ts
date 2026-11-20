// Rejection paths against a fresh `scripts/local-validator.sh`. Run before
// `local-test.ts`: it initializes the config itself, and leaves it as that test expects.
// Leave `npm run oracle` off: the test decides when ORAO answers.
import { BN } from "@coral-xyz/anchor";
import {
  ComputeBudgetProgram,
  Keypair,
  PublicKey,
  SystemProgram,
  Transaction,
  type TransactionInstruction,
} from "@solana/web3.js";
import { NATIVE_MINT, TOKEN_2022_PROGRAM_ID, createAssociatedTokenAccountIdempotentInstruction } from "@solana/spl-token";
import {
  type Cranker,
  type LabkitConfig,
  type LaunchInfo,
  type Pending,
  crank,
  drawTiming,
  decodeLaunch,
  drawAccounts,
  drawCandidates,
  fetchConfig,
  initialize,
  kitBuyMethod,
  updateConfig,
} from "./crank.js";
import { createCreatorVestingIx, createLaunchIx, initializeLaunchIx, registerLaunchIx } from "./launch.js";
import {
  CRANK_BPS,
  ata,
  buy,
  claimStake,
  ensureConfig,
  ensureLocalVrf,
  expect,
  expectError,
  fulfillDraw,
  funded,
  launch,
  launchParams,
  openDraw,
  openEnv,
  openStakePool,
  readDraw,
  run,
  send,
  stake,
  testConfig,
  tokenAmount,
  unstake,
  waitForChain,
} from "./test-env.js";
import { readRandomness } from "./vrf.js";
import {
  SOL_GLOBAL_CONFIG_MAINNET,
  configPda,
  creatorFeeVaultPda,
  creatorPda,
  drawPda,
  eventAuthorityPda,
  feeWalletPda,
  kitPda,
  launchPda,
  launchPlatformPda,
  oraoRandomnessPda,
  platformFeeVaultPda,
  poolStatePda,
  poolVaultPda,
  stakePositionPda,
  vaultAuthorityPda,
} from "./ids.js";

const SOL = 1_000_000_000n;
const DRAW = drawTiming("localnet");

run(async () => {
  const env = await openEnv();
  const { connection } = env;
  const [stranger, admin, trader, keeper, creatorWallet] = await funded(env, 5);
  const owner = env.open(env.authority);
  const s = env.open(stranger);
  const t = env.open(trader);
  const k = env.open(keeper);
  const ll = t.launchlab.programId;
  if (await fetchConfig(owner)) throw new Error("run this on a fresh validator, before local-test");

  console.log("admin");
  const params = testConfig(Keypair.generate().publicKey);
  const { fees, platform } = params;
  await expectError("initialize by a stranger", "NotUpgradeAuthority", () => initialize(s, params));
  await initialize(owner, params);
  await expectError("initialize twice", "already in use", () => initialize(owner, params));
  await expectError("update_config by a stranger", "ConstraintHasOne", () => updateConfig(s, params));
  await expectError("keeper share above 5%", "CrankFeeTooHigh", () =>
    updateConfig(owner, { ...params, fees: { ...fees, crankBps: 501 } }),
  );
  await expectError("kit share above 100%", "InvalidBps", () =>
    updateConfig(owner, { ...params, fees: { ...fees, kitBps: 10_001 } }),
  );
  await expectError("Raydium creator side above 100%", "InvalidBps", () =>
    updateConfig(owner, { ...params, fees: { ...fees, ammCreatorBps: 10_001 } }),
  );
  await expectError("default treasury", "InvalidTreasury", () =>
    updateConfig(owner, { ...params, treasury: PublicKey.default }),
  );
  await expectError("platform fee of 100%", "InvalidPlatform", () =>
    updateConfig(owner, { ...params, platform: { ...platform, feeRate: new BN(1_000_000) } }),
  );
  await expectError("LP lock above 100%", "InvalidPlatform", () =>
    updateConfig(owner, { ...params, platform: { ...platform, lpLockScale: new BN(1_000_001) } }),
  );
  await expectError("platform web too long", "InvalidPlatform", () =>
    updateConfig(owner, { ...params, platform: { ...platform, web: "x".repeat(65) } }),
  );
  await expectError("default CPMM config", "InvalidPlatform", () =>
    updateConfig(owner, { ...params, platform: { ...platform, cpswapConfig: PublicKey.default } }),
  );
  await expectError("transfer_authority by a stranger", "ConstraintHasOne", () =>
    transferAuthority(s, stranger.publicKey),
  );
  await transferAuthority(owner, admin.publicKey);
  await expectError("accept_authority by a stranger", "NotPendingAuthority", () => acceptAuthority(s));
  await acceptAuthority(env.open(admin));
  await expectError("update_config by the old authority", "ConstraintHasOne", () => updateConfig(owner, params));
  await transferAuthority(env.open(admin), env.authority.publicKey);
  await acceptAuthority(owner);
  await expectError("accept_authority twice", "NotPendingAuthority", () => acceptAuthority(owner));
  const config = await ensureConfig(env);
  expect(config.authority.equals(env.authority.publicKey), "authority round trip restored the authority");
  expect(config.fees.crankBps === CRANK_BPS, "rejected updates left the config alone");

  console.log("create_launch");
  const recipient = creatorWallet.publicKey;
  const createOnly = async (c: Cranker, cfg: LabkitConfig, p: ReturnType<typeof launchParams>) => {
    const base = Keypair.generate();
    await send(c, [await createLaunchIx(c, cfg, p, base.publicKey)], [base]);
    return base;
  };
  await expectError("default recipient", "InvalidRecipient", () =>
    createOnly(t, config, launchParams(PublicKey.default)),
  );
  await expectError("Draw with a drawdown", "InvalidKit", () =>
    createOnly(t, config, launchParams(recipient, "draw", 1_000)),
  );
  await expectError("Floor with a 0.5% drawdown", "InvalidKit", () =>
    createOnly(t, config, launchParams(recipient, "floor", 50)),
  );
  await expectError("Floor with a 95% drawdown", "InvalidKit", () =>
    createOnly(t, config, launchParams(recipient, "floor", 9_500)),
  );
  await expectError("Burn with a drawdown", "InvalidKit", () =>
    createOnly(t, config, launchParams(recipient, "burn", 1_000)),
  );
  await expectError("an NFT wallet off the template", "ConstraintAddress", () =>
    createOnly(t, { ...config, platform: { ...config.platform, nftWallet: stranger.publicKey } }, launchParams(recipient)),
  );
  await expectError("a CPMM config off the template", "ConstraintAddress", () =>
    createOnly(t, { ...config, platform: { ...config.platform, cpswapConfig: stranger.publicKey } }, launchParams(recipient)),
  );
  await expectError("a treasury other than the config's", "ConstraintAddress", () =>
    createOnly(t, { ...config, treasury: stranger.publicKey }, launchParams(recipient)),
  );

  console.log("register_launch");
  const main = await launch(t, config, launchParams(recipient));
  const pending = await createOnly(t, config, launchParams(recipient));
  const p = launchParams(recipient);
  await expectError("register a pool on another launch's platform", "PlatformMismatch", async () =>
    send(
      t,
      [
        await initializeLaunchIx(t, p, pending.publicKey, creatorPda(pending.publicKey), main.platform),
        await registerLaunchIx(t, p, pending.publicKey),
      ],
      [pending],
    ),
  );
  await expectError("register a pool whose creator is not the PDA", "PoolCreatorMismatch", async () =>
    send(
      t,
      [
        await initializeLaunchIx(t, p, pending.publicKey, trader.publicKey),
        await registerLaunchIx(t, p, pending.publicKey),
      ],
      [pending],
    ),
  );
  await expectError("register a pool that takes Raydium creator fees in both tokens", "CreatorFeeNotQuote", async () =>
    send(
      t,
      [
        await initializeLaunchIx(t, p, pending.publicKey, creatorPda(pending.publicKey), undefined, { bothToken: {} }),
        await registerLaunchIx(t, p, pending.publicKey),
      ],
      [pending],
    ),
  );
  await expectError("register a Token-2022 pool", "UnsupportedBaseToken", async () =>
    send(t, [await initializeToken2022Ix(t, p, pending.publicKey), await registerLaunchIx(t, p, pending.publicKey)], [
      pending,
    ]),
  );
  await expectError("register a Token-2022 pool with a transfer fee", "UnsupportedBaseToken", async () =>
    send(
      t,
      [
        await initializeToken2022Ix(t, p, pending.publicKey, { transferFeeBasisPoints: 100, maximumFee: p.supply }),
        await registerLaunchIx(t, p, pending.publicKey),
      ],
      [pending],
    ),
  );
  await expectError("register another mint's pool", "BadPoolState", async () =>
    send(
      t,
      [
        await t.labkit.methods
          .registerLaunch()
          .accountsStrict({
            launch: launchPda(pending.publicKey),
            creator: creatorPda(pending.publicKey),
            poolState: poolStatePda(ll, main.mint, NATIVE_MINT),
          })
          .instruction(),
      ],
      [],
    ),
  );
  await expectError("register twice", "AlreadyRegistered", async () =>
    send(t, [await registerLaunchIx(t, p, main.mint)], []),
  );

  console.log("launch recipient");
  await expectError("set_launch_recipient by a stranger", "ConstraintHasOne", () =>
    s.labkit.methods
      .setLaunchRecipient(stranger.publicKey)
      .accountsStrict({ recipient: stranger.publicKey, launch: launchPda(main.mint) })
      .rpc(),
  );
  await expectError("set_launch_recipient to the default key", "InvalidRecipient", () =>
    env
      .open(creatorWallet)
      .labkit.methods.setLaunchRecipient(PublicKey.default)
      .accountsStrict({ recipient, launch: launchPda(main.mint) })
      .rpc(),
  );

  console.log("vesting");
  const locked = new BN("10000000000000");
  const vested = await launch(
    t,
    config,
    launchParams(recipient, "burn", 0, { totalLockedAmount: locked, cliffPeriod: new BN(0), unlockPeriod: new BN(86_400) }),
  );
  const w = env.open(creatorWallet);
  await expectError("create_creator_vesting by a stranger", "ConstraintHasOne", async () =>
    send(s, [await createCreatorVestingIx(s, stranger.publicKey, vested.mint, NATIVE_MINT, stranger.publicKey, locked)], []),
  );
  await expectError("create_creator_vesting with another launch's pool", "PoolMismatch", async () => {
    const ix = await createCreatorVestingIx(w, recipient, vested.mint, NATIVE_MINT, recipient, locked);
    ix.keys[4].pubkey = poolStatePda(ll, main.mint, NATIVE_MINT);
    return send(w, [ix], []);
  });
  await expectError("vesting more than the pool locked", "Error Number: 60", async () =>
    send(w, [await createCreatorVestingIx(w, recipient, vested.mint, NATIVE_MINT, recipient, locked.addn(1))], []),
  );

  console.log("claims and splits");
  const info = async (mint: PublicKey): Promise<LaunchInfo> =>
    decodeLaunch(k, launchPda(mint), (await connection.getAccountInfo(launchPda(mint)))!.data);
  const other = await launch(t, config, launchParams(stranger.publicKey));
  await buy(t, main.platform, main.mint);
  await buy(t, other.platform, other.mint);
  const mainInfo = await info(main.mint);
  const otherInfo = await info(other.mint);
  const kitClaim: Pending = {
    kind: "kit-fees",
    launch: mainInfo,
    account: platformFeeVaultPda(ll, main.platform, NATIVE_MINT),
    amount: 1n,
  };
  const creatorClaim: Pending = {
    kind: "creator-vault",
    launch: mainInfo,
    account: creatorFeeVaultPda(ll, creatorPda(main.mint), NATIVE_MINT),
    amount: 1n,
  };
  const strangerToken = ata(stranger.publicKey);
  await send(
    s,
    [createAssociatedTokenAccountIdempotentInstruction(stranger.publicKey, strangerToken, stranger.publicKey, NATIVE_MINT)],
    [],
  );

  await expectError("claim_kit_fees paying a token account the treasury does not own", "TreasuryMismatch", () =>
    crank(k, config, kitClaim, { treasuryToken: strangerToken }),
  );
  await expectError("claim_kit_fees from another launch's platform", "PlatformMismatch", () =>
    crank(k, config, kitClaim, {
      platformConfig: other.platform,
      platformFeeVault: platformFeeVaultPda(ll, other.platform, NATIVE_MINT),
    }),
  );
  await expectError("claim_kit_fees from another platform's vault", "Error", () =>
    crank(k, config, kitClaim, { platformFeeVault: platformFeeVaultPda(ll, other.platform, NATIVE_MINT) }),
  );
  await expectError("claim_kit_fees into another launch's kit", "ConstraintSeeds", () =>
    crank(k, config, kitClaim, { kit: kitPda(other.mint), kitQuote: ata(kitPda(other.mint)) }),
  );
  await expectError("claim_creator_fees paying someone other than the recipient", "RecipientMismatch", () =>
    crank(k, config, creatorClaim, { recipientToken: strangerToken }),
  );
  await expectError("claim_creator_fees from another launch's vault", "Error", () =>
    crank(k, config, creatorClaim, { creatorFeeVault: creatorFeeVaultPda(ll, creatorPda(other.mint), NATIVE_MINT) }),
  );

  await crank(k, config, kitClaim);
  await crank(k, config, creatorClaim);
  await crank(k, config, {
    ...creatorClaim,
    launch: otherInfo,
    account: creatorFeeVaultPda(ll, creatorPda(other.mint), NATIVE_MINT),
  });
  await crank(k, config, {
    ...kitClaim,
    launch: otherInfo,
    account: platformFeeVaultPda(ll, other.platform, NATIVE_MINT),
  });
  await send(
    k,
    [createAssociatedTokenAccountIdempotentInstruction(keeper.publicKey, ata(feeWalletPda()), feeWalletPda(), NATIVE_MINT)],
    [],
  );
  await expectError("split_fees on an empty fee wallet", "NothingToSplit", () =>
    crank(k, config, { kind: "fee-wallet", account: ata(feeWalletPda()), amount: 0n }),
  );
  await expectError("split_fees from a token account that is not the fee wallet's", "ConstraintTokenOwner", () =>
    crank(k, config, { kind: "fee-wallet", account: strangerToken, amount: 0n }),
  );
  const creatorSplit: Pending = {
    kind: "creator-wallet",
    launch: mainInfo,
    account: ata(creatorPda(main.mint)),
    amount: 0n,
  };
  await expectError("split_creator_fees on an empty creator wallet", "NothingToSplit", () =>
    crank(k, config, creatorSplit),
  );
  await expectError("split_creator_fees naming another launch's creator PDA", "ConstraintSeeds", () =>
    crank(k, config, creatorSplit, {
      creator: creatorPda(other.mint),
      creatorToken: ata(creatorPda(other.mint)),
    }),
  );

  console.log("kits");
  const market = {
    status: 0,
    quote: 0n,
    base: 0n,
    feeRate: 0n,
    baseLeft: null,
    globalConfig: SOL_GLOBAL_CONFIG_MAINNET,
    amm: null,
  };
  const kitBuy = (l: LaunchInfo): Extract<Pending, { kind: "kit-buy" }> => ({
    kind: "kit-buy",
    launch: l,
    market,
    account: ata(kitPda(l.baseMint)),
    amount: 0n,
  });
  const release = (l: LaunchInfo): Pending => ({ kind: "kit-release", launch: l, account: ata(kitPda(l.baseMint)), amount: 0n });
  const checkpoint = (l: LaunchInfo): Pending => ({ kind: "kit-checkpoint", launch: l, market, account: launchPda(l.baseMint), amount: 0n });

  // One 1 SOL buy leaves the kit 0.7% of it, under the 0.01 SOL minimum.
  await expectError("a Burn buy under the minimum", "BelowMinimum", () => crank(k, config, kitBuy(mainInfo)));
  await expectError("a Burn buy through another launch's platform", "PlatformMismatch", () =>
    crank(k, config, kitBuy(mainInfo), { platformConfig: other.platform }),
  );
  await expectError("a Burn buy spending another launch's kit", "ConstraintSeeds", () =>
    crank(k, config, kitBuy(mainInfo), {
      kit: kitPda(other.mint),
      kitQuote: ata(kitPda(other.mint)),
      kitBase: ata(kitPda(other.mint), main.mint),
    }),
  );
  await expectError("a Burn buy with another pool's global config", "BadPoolState", () =>
    crank(k, config, kitBuy(mainInfo), { globalConfig: configPda() }),
  );
  await expectError("checkpoint on a Burn launch", "WrongKit", () => crank(k, config, checkpoint(mainInfo)));
  await expectError("release on a Burn launch", "WrongKit", () => crank(k, config, release(mainInfo)));

  await buy(t, main.platform, main.mint, 0, null, 3n * SOL);
  await crank(k, config, kitClaim);
  await expectError("two kit buys in one transaction", "ExecutedThisSlot", async () => {
    const buyIx = () => kitBuyMethod(k, kitBuy(mainInfo)).instruction();
    const tx = new Transaction().add(ComputeBudgetProgram.setComputeUnitLimit({ units: 1_000_000 }), await buyIx(), await buyIx());
    return k.labkit.provider.sendAndConfirm!(tx);
  });

  const floor = await launch(t, config, launchParams(recipient, "floor", 1_000));
  await buy(t, floor.platform, floor.mint, 0, null, 3n * 1_000_000_000n);
  const floorInfo = await info(floor.mint);
  await crank(k, config, { kind: "kit-fees", launch: floorInfo, account: platformFeeVaultPda(ll, floor.platform, NATIVE_MINT), amount: 1n });
  await expectError("a Floor buy at its high", "AboveFloor", () => crank(k, config, kitBuy(floorInfo)));
  await expectError("release on a Floor launch", "WrongKit", () => crank(k, config, release(floorInfo)));

  const grad = await launch(t, config, launchParams(recipient, "graduate"));
  await buy(t, grad.platform, grad.mint, 0, null, 3n * 1_000_000_000n);
  const gradInfo = await info(grad.mint);
  await crank(k, config, { kind: "kit-fees", launch: gradInfo, account: platformFeeVaultPda(ll, grad.platform, NATIVE_MINT), amount: 1n });
  await expectError("release before the curve completes", "NotGraduated", () => crank(k, config, release(gradInfo)));
  await expectError("release to someone other than the recipient", "RecipientMismatch", () =>
    crank(k, config, release(gradInfo), { recipientToken: strangerToken }),
  );
  await expectError("checkpoint on a Graduate launch", "WrongKit", () => crank(k, config, checkpoint(gradInfo)));

  console.log("stake");
  const stk = await launch(t, config, launchParams(recipient, "stake"));
  await buy(t, stk.platform, stk.mint, 0, null, 3n * 1_000_000_000n);
  await buy(s, stk.platform, stk.mint, 0, null, 100_000_000n);
  const stkInfo = await info(stk.mint);
  await crank(k, config, { kind: "kit-fees", launch: stkInfo, account: platformFeeVaultPda(ll, stk.platform, NATIVE_MINT), amount: 1n });
  await expectError("a kit buy on a Stake launch", "WrongKit", () => crank(k, config, kitBuy(stkInfo)));
  await expectError("release on a Stake launch", "WrongKit", () => crank(k, config, release(stkInfo)));
  await expectError("checkpoint on a Stake launch", "WrongKit", () => crank(k, config, checkpoint(stkInfo)));
  await expectError("open a stake pool on a Burn launch", "WrongKit", () => openStakePool(t, main.mint));
  await expectError("stake zero", "ZeroAmount", () => stake(t, stk.mint, 0n));
  const held = await tokenAmount(connection, ata(trader.publicKey, stk.mint));
  await expectError("stake more than the wallet holds", "insufficient funds", () => stake(t, stk.mint, held + 1n));
  await expectError("stake from someone else's token account", "ConstraintTokenOwner", () =>
    stake(t, stk.mint, 1n, { ownerBase: ata(stranger.publicKey, stk.mint) }),
  );
  await expectError("stake into a vault the pool does not own", "ConstraintTokenOwner", () =>
    stake(t, stk.mint, 1n, { vault: ata(trader.publicKey, stk.mint) }),
  );
  await expectError("stake with another launch's kit account", "ConstraintTokenOwner", () =>
    stake(t, stk.mint, 1n, { kitQuote: ata(kitPda(main.mint)) }),
  );
  await stake(t, stk.mint, held);
  await expectError("open the stake pool twice", "already in use", () => openStakePool(s, stk.mint));
  await expectError("unstake more than staked", "InsufficientStake", () => unstake(t, stk.mint, held + 1n));
  await expectError("unstake without a stake", "AccountNotInitialized", () => unstake(s, stk.mint, 1n));
  await expectError("unstake someone else's stake", "ConstraintSeeds", () =>
    unstake(s, stk.mint, 1n, { position: stakePositionPda(stk.mint, trader.publicKey) }),
  );
  await expectError("unstake to someone else's token account", "ConstraintTokenOwner", () =>
    unstake(t, stk.mint, 1n, { ownerBase: ata(stranger.publicKey, stk.mint) }),
  );
  await expectError("claim someone else's stake", "ConstraintSeeds", () =>
    claimStake(s, stk.mint, { position: stakePositionPda(stk.mint, trader.publicKey) }),
  );
  await expectError("claim into someone else's WSOL account", "ConstraintTokenOwner", () =>
    claimStake(t, stk.mint, { ownerQuote: strangerToken }),
  );
  await expectError("claim from another launch's kit", "ConstraintTokenOwner", () =>
    claimStake(t, stk.mint, { kitQuote: ata(kitPda(main.mint)) }),
  );
  // One raw token earns under a lamport for days, so this claim finds nothing.
  await stake(s, stk.mint, 1n);
  await expectError("claim before earning a lamport", "NothingToClaim", () => claimStake(s, stk.mint));

  console.log("draw");
  await ensureLocalVrf(env);
  const [drawer] = await funded(env, 1);
  const dr = env.open(drawer);
  await expectError("open a draw on a Burn launch", "WrongKit", () => openDraw(t, main.mint));
  const drw = await launch(dr, config, launchParams(recipient, "draw"));
  const drwInfo = await info(drw.mint);
  const drwKit = ata(kitPda(drw.mint));
  const claimDrawFees = () =>
    crank(k, config, { kind: "kit-fees", launch: drwInfo, account: platformFeeVaultPda(ll, drw.platform, NATIVE_MINT), amount: 1n });
  await buy(dr, drw.platform, drw.mint, 0, null, 3n * SOL);
  await buy(s, drw.platform, drw.mint, 0, null, 100_000_000n);
  await claimDrawFees();
  await expectError("a kit buy on a Draw launch", "WrongKit", () => crank(k, config, kitBuy(drwInfo)));
  await expectError("open a stake pool on a Draw launch", "WrongKit", () => openStakePool(t, drw.mint));
  await openDraw(dr, drw.mint);
  await expectError("open the draw twice", "already in use", () => openDraw(s, drw.mint));
  await expectError("stake without the draw tree", "BadDrawTree", () => stake(dr, drw.mint, 1n, { drawTree: null }));
  await expectError("stake into an account that is not a draw tree", "BadDrawTree", () =>
    stake(dr, drw.mint, 1n, { drawTree: drawPda(drw.mint) }),
  );

  const request: Pending = { kind: "draw-request", launch: drwInfo, account: drwKit, amount: 0n };
  await expectError("a draw before the interval", "DrawNotDue", () => crank(k, config, request));
  await waitForChain(connection, (await readDraw(k, drw.mint)).lastDraw + DRAW.interval);
  await expectError("a draw with no stakers", "NoStakers", () => crank(k, config, request));
  await stake(dr, drw.mint, await tokenAmount(connection, ata(drawer.publicKey, drw.mint)));
  await stake(s, drw.mint, (await tokenAmount(connection, ata(stranger.publicKey, drw.mint))) - 1n);
  await expectError("a draw under the minimum pot", "PotTooSmall", () => crank(k, config, request));
  await buy(dr, drw.platform, drw.mint, 0, null, 6n * SOL);
  await claimDrawFees();
  await crank(k, config, request);
  let draw = await readDraw(k, drw.mint);
  await expectError("a second request while one is pending", "DrawInProgress", () => crank(k, config, request));
  await expectError("stake while a draw is pending", "DrawInProgress", () => stake(s, drw.mint, 1n));
  await expectError("unstake while a draw is pending", "DrawInProgress", () => unstake(s, drw.mint, 1n));
  const settle = (randomness: Buffer): Pending => ({ kind: "draw-settle", launch: drwInfo, account: drwKit, amount: 0n, draw, randomness });
  const cancel = (): Pending => ({ kind: "draw-cancel", launch: drwInfo, account: drwKit, amount: 0n, draw });
  await expectError("settle before ORAO answers", "NotFulfilled", () => crank(k, config, settle(Buffer.alloc(64))));
  await expectError("cancel before the timeout", "NotTimedOut", () => crank(k, config, cancel()));

  await fulfillDraw(env, draw.seed);
  const randomness = (await readRandomness(connection, draw.seed))!;
  const picked = await drawCandidates(k, drw.mint, draw, randomness);
  const positions = [stakePositionPda(drw.mint, drawer.publicKey), stakePositionPda(drw.mint, stranger.publicKey)];
  const wrong = positions.find((p) => !p.equals(picked[0].address))!;
  const settleWith = (remaining: PublicKey[]) => {
    const { launch: launchKey, stakePool, draw: drawKey, drawTree, kit, quoteMint, kitQuote, tokenProgram } = drawAccounts(drw.mint);
    return k.labkit.methods
      .settleDraw()
      .accountsStrict({
        launch: launchKey,
        stakePool,
        draw: drawKey,
        drawTree,
        kit,
        quoteMint,
        kitQuote,
        randomness: oraoRandomnessPda(draw.seed),
        tokenProgram,
      })
      .remainingAccounts(remaining.map((pubkey) => ({ pubkey, isSigner: false, isWritable: true })))
      .rpc();
  };
  await expectError("settle without the picked positions", "BadCandidate", () => settleWith([]));
  await expectError("settle naming the wrong staker", "BadCandidate", () =>
    settleWith([wrong, ...picked.slice(1).map((p) => p.address)]),
  );
  await expectError("settle naming another launch's staker", "BadCandidate", () =>
    settleWith([stakePositionPda(stk.mint, trader.publicKey), ...picked.slice(1).map((p) => p.address)]),
  );
  await waitForChain(connection, draw.requestedAt + DRAW.timeout);
  await expectError("cancel a draw ORAO answered", "AlreadyFulfilled", () => crank(k, config, cancel()));
  await crank(k, config, settle(randomness));
  draw = await readDraw(k, drw.mint);
  expect(!draw.pending && draw.round === 1n, "the late answer still settled the draw");
  await expectError("settle twice", "NoDrawPending", () => crank(k, config, settle(randomness)));

  await waitForChain(connection, draw.lastDraw + DRAW.interval);
  await crank(k, config, request);
  draw = await readDraw(k, drw.mint);
  await waitForChain(connection, draw.requestedAt + DRAW.timeout);
  const potBefore = await tokenAmount(connection, drwKit);
  await crank(k, config, cancel());
  draw = await readDraw(k, drw.mint);
  expect(!draw.pending && draw.round === 1n, "an unanswered draw was cancelled without a round");
  expect((await tokenAmount(connection, drwKit)) === potBefore, "cancelling left the pot alone");
  await stake(s, drw.mint, 1n);
  await expectError("cancel twice", "NoDrawPending", () => crank(k, config, cancel()));

  console.log("negative test passed");
});

function transferAuthority(c: Cranker, to: PublicKey): Promise<string> {
  return c.labkit.methods
    .transferAuthority(to)
    .accountsStrict({ authority: c.payer.publicKey, config: configPda() })
    .rpc();
}

function acceptAuthority(c: Cranker): Promise<string> {
  return c.labkit.methods
    .acceptAuthority()
    .accountsStrict({ newAuthority: c.payer.publicKey, config: configPda() })
    .rpc();
}

/** LaunchLab `initialize_with_token_2022`, the Token-2022 twin of `initializeLaunchIx`, with the creator PDA. */
async function initializeToken2022Ix(
  c: Cranker,
  params: ReturnType<typeof launchParams>,
  baseMint: PublicKey,
  transferFee: { transferFeeBasisPoints: number; maximumFee: BN } | null = null,
): Promise<TransactionInstruction> {
  const ll = c.launchlab.programId;
  const pool = poolStatePda(ll, baseMint, params.quoteMint);
  return c.launchlab.methods
    .initializeWithToken2022(
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
      { totalLockedAmount: new BN(0), cliffPeriod: new BN(0), unlockPeriod: new BN(0) },
      { quoteToken: {} },
      transferFee,
    )
    .accountsStrict({
      payer: c.payer.publicKey,
      creator: creatorPda(baseMint),
      globalConfig: params.globalConfig,
      platformConfig: launchPlatformPda(ll, baseMint),
      authority: vaultAuthorityPda(ll),
      poolState: pool,
      baseMint,
      quoteMint: params.quoteMint,
      baseVault: poolVaultPda(ll, pool, baseMint),
      quoteVault: poolVaultPda(ll, pool, params.quoteMint),
      baseTokenProgram: TOKEN_2022_PROGRAM_ID,
      quoteTokenProgram: params.quoteTokenProgram,
      systemProgram: SystemProgram.programId,
      eventAuthority: eventAuthorityPda(ll),
      program: ll,
    })
    .instruction();
}
