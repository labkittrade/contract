// Happy path against `scripts/local-validator.sh`. Each launch gets its own platform whose fees
// pay its kit. Covers the fee splits, Burn on the curve and on Raydium, Floor's checkpoint and
// buy under the line, Graduate's curve buys and release, Stake's stream to stakers, Draw's
// weighted draw and rollover, referrals, and creator vesting. Leave `npm run oracle` off: the
// test answers ORAO itself.
import { BN } from "@coral-xyz/anchor";
import { Keypair, LAMPORTS_PER_SOL, PublicKey } from "@solana/web3.js";
import { NATIVE_MINT, getAssociatedTokenAddressSync } from "@solana/spl-token";
import {
  type Cranker,
  type Pending,
  crank,
  describe,
  discover,
  drawTiming,
  findPending,
  highRiseSeconds,
  runOnce,
  selectWork,
} from "./crank.js";
import { decodeCpmmPool } from "./cpmm.js";
import { graduate, fillCurve } from "./graduate.js";
import { createCreatorVestingIx } from "./launch.js";
import {
  AMM_CREATOR_BPS,
  CRANK_BPS,
  CREATOR_FEE_BPS,
  KIT_BPS,
  PLATFORM_FEE_BPS,
  REFERRAL_RATE,
  airdrop,
  ammBuy,
  ata,
  bps,
  buy,
  claimStake,
  deltas,
  ensureConfig,
  ensureLocalVrf,
  expect,
  expectError,
  fulfillDraw,
  funded,
  launch,
  launchParams,
  openEnv,
  readDraw,
  run,
  sell,
  send,
  snapshot,
  stake,
  supply,
  tokenAmount,
  unstake,
  waitForChain,
} from "./test-env.js";
import {
  creatorFeeVaultPda,
  creatorPda,
  drawTreePda,
  feeWalletPda,
  kitPda,
  launchPda,
  platformAdminPda,
  platformFeeVaultPda,
  poolStatePda,
  stakePoolPda,
  stakePositionPda,
  vestingRecordPda,
} from "./ids.js";

const SOL = BigInt(LAMPORTS_PER_SOL);

function readLaunch(c: Cranker, mint: PublicKey) {
  return c.connection.getAccountInfo(launchPda(mint)).then((info) => {
    if (!info) throw new Error(`no launch for ${mint.toBase58()}`);
    const l = c.labkit.coder.accounts.decode("launch", info.data);
    const big = (v: BN) => BigInt(v.toString());
    return {
      kit: l.kit as number,
      registered: l.registered as boolean,
      platformConfig: l.platformConfig as PublicKey,
      recipient: l.recipient as PublicKey,
      highQ64: big(l.highQ64),
      highAt: Number(l.highAt),
      kitFunded: big(l.kitFunded),
      spent: big(l.spent),
      burned: big(l.burned),
      released: big(l.released),
      executions: big(l.executions),
    };
  });
}

const STREAM = 3n * 24n * 60n * 60n;
const DRAW = drawTiming("localnet");
const DRAW_MIN_POT = 50_000_000n;
const DRAW_TREE_LEN_OFFSET = 10;

async function readStakePool(c: Cranker, mint: PublicKey) {
  const p = c.labkit.coder.accounts.decode("stakePool", (await c.connection.getAccountInfo(stakePoolPda(mint)))!.data);
  const big = (v: BN) => BigInt(v.toString());
  return {
    totalStaked: big(p.totalStaked),
    streaming: big(p.streaming),
    streamEnd: big(p.streamEnd),
    lastUpdate: big(p.lastUpdate),
    distributed: big(p.distributed),
    claimed: big(p.claimed),
  };
}

async function readPosition(c: Cranker, mint: PublicKey, owner: PublicKey) {
  const info = await c.connection.getAccountInfo(stakePositionPda(mint, owner));
  const p = c.labkit.coder.accounts.decode("stakePosition", info!.data);
  const big = (v: BN) => BigInt(v.toString());
  return { amount: big(p.amount), earned: big(p.earned), claimed: big(p.claimed) };
}

async function curvePrice(c: Cranker, mint: PublicKey): Promise<bigint> {
  const info = await c.connection.getAccountInfo(poolStatePda(c.launchlab.programId, mint, NATIVE_MINT));
  const s = c.launchlab.coder.accounts.decode("poolState", info!.data);
  const big = (v: BN) => BigInt(v.toString());
  return ((big(s.virtualQuote) + big(s.realQuote)) << 64n) / (big(s.virtualBase) - big(s.realBase));
}

run(async () => {
  const env = await openEnv();
  const { connection } = env;
  const [trader, keeper, dev, team] = await funded(env, 4);
  const config = await ensureConfig(env);
  const t = env.open(trader);
  const k = env.open(keeper);
  const ll = t.launchlab.programId;
  const treasuryToken = ata(config.treasury);
  const keeperToken = ata(keeper.publicKey);
  const devToken = ata(dev.publicKey);
  // Cranks one launch's work, so leftovers of other launches on the chain don't count.
  const crankFor = async (mint: PublicKey, what: string, ...kinds: Pending["kind"][]) => {
    const work = selectWork(await findPending(k, await discover(k)), 1n).filter(
      (p) => p.kind !== "fee-wallet" && p.launch.baseMint.equals(mint),
    );
    const ran = work.map((p) => p.kind).sort();
    expect(ran.join() === [...kinds].sort().join(), `${what} (ran ${ran.join(", ") || "nothing"})`);
    for (const p of work) await crank(k, config, p);
  };

  console.log("burn: launch on its own platform");
  const burn = await launch(t, config, launchParams(dev.publicKey, "burn"));
  const burnKit = ata(kitPda(burn.mint));
  const info = await readLaunch(t, burn.mint);
  expect(info.registered && info.platformConfig.equals(burn.platform), "the launch is registered on its platform");
  const platform = t.launchlab.coder.accounts.decode("platformConfig", (await connection.getAccountInfo(burn.platform))!.data);
  expect((platform.platformFeeWallet as PublicKey).equals(kitPda(burn.mint)), "the platform pays the kit PDA");
  expect(Number(platform.feeRate) === PLATFORM_FEE_BPS * 100, "the platform charges the template fee");
  expect((await connection.getBalance(platformAdminPda(burn.mint))) === 0, "the platform admin kept no SOL");

  await buy(t, burn.platform, burn.mint, 0, null, 2n * SOL);
  const platformFee = await tokenAmount(connection, platformFeeVaultPda(ll, burn.platform, NATIVE_MINT));
  const creatorFee = await tokenAmount(connection, creatorFeeVaultPda(ll, creatorPda(burn.mint), NATIVE_MINT));
  expect(platformFee === bps(2n * SOL, PLATFORM_FEE_BPS), "platform fee is 1% of the buy");
  expect(creatorFee === bps(2n * SOL, CREATOR_FEE_BPS), "creator fee is 0.5% of the buy");

  let before = await snapshot(connection, [treasuryToken, keeperToken, devToken, burnKit]);
  await crankFor(burn.mint, "the crank claims the platform and creator fees", "kit-fees", "creator-vault");
  let d = await deltas(connection, before);
  const kitShare = bps(platformFee, KIT_BPS);
  expect(d[3] === kitShare && d[0] === platformFee - kitShare, "platform fees split 70% kit, 30% treasury");
  expect(d[1] === bps(creatorFee, CRANK_BPS) && d[2] === creatorFee - d[1], "creator fees split 95% dev, 5% keeper");
  console.log(`  kit +${d[3]}, treasury +${d[0]}, dev +${d[2]}, keeper +${d[1]}`);

  let supplyBefore = await supply(connection, burn.mint);
  await crankFor(burn.mint, "the crank runs the Burn kit", "kit-buy");
  let l = await readLaunch(t, burn.mint);
  expect(l.spent === kitShare && l.burned > 0n, "Burn spent the kit on the curve");
  expect(supplyBefore - (await supply(connection, burn.mint)) === l.burned, "Burn burned what it bought");
  expect((await tokenAmount(connection, burnKit)) === 0n, "the kit is empty");
  console.log(`  burned ${l.burned} for ${l.spent} lamports`);

  console.log("referral");
  const feeWalletToken = ata(feeWalletPda());
  await buy(t, burn.platform, burn.mint, REFERRAL_RATE, feeWalletToken);
  const referral = await tokenAmount(connection, feeWalletToken);
  before = await snapshot(connection, [treasuryToken]);
  const split: Pending = { kind: "fee-wallet", account: feeWalletToken, amount: referral };
  await crank(k, config, split);
  d = await deltas(connection, before);
  expect(referral > 0n && d[0] === referral, "referral fees go 100% to the treasury");

  console.log("floor: the high rises at a capped rate, buy under the line");
  const floor = await launch(t, config, launchParams(dev.publicKey, "floor", 1_000));
  const opening = await curvePrice(t, floor.mint);
  l = await readLaunch(t, floor.mint);
  expect(l.highQ64 === opening, "registration set the high to the opening price");
  await buy(t, floor.platform, floor.mint, 0, null, 10n * SOL);
  const pumped = await curvePrice(t, floor.mint);
  await waitForChain(connection, l.highAt + highRiseSeconds("localnet"));
  await crankFor(floor.mint, "the crank claims and checkpoints", "kit-fees", "creator-vault", "kit-checkpoint");
  l = await readLaunch(t, floor.mint);
  expect(
    l.highQ64 > opening && l.highQ64 <= (opening * 11_000n) / 10_000n && l.executions === 0n,
    "one checkpoint raised the high by at most 10%, without buying",
  );
  while ((l.highQ64 * 10_100n) / 10_000n < pumped) {
    await waitForChain(connection, l.highAt + highRiseSeconds("localnet"));
    await crankFor(floor.mint, "the crank checkpoints again", "kit-checkpoint");
    l = await readLaunch(t, floor.mint);
  }
  const high = l.highQ64;
  expect(high <= pumped, `checkpoints walked the high up to the price (${(high * 100n) / opening}% of opening)`);
  const held = await tokenAmount(connection, getAssociatedTokenAddressSync(floor.mint, trader.publicKey));
  await sell(t, floor.platform, floor.mint, held / 2n);
  const dipped = await curvePrice(t, floor.mint);
  expect(dipped < (high * 9_000n) / 10_000n, "the price fell under the 10% line");
  await crankFor(floor.mint, "the crank claims and runs Floor", "kit-fees", "creator-vault", "kit-buy");
  l = await readLaunch(t, floor.mint);
  expect(l.executions === 1n && l.burned > 0n, "Floor bought under the line");
  expect((await curvePrice(t, floor.mint)) > dipped, "the buy lifted the price");
  expect(l.highQ64 === high, "the high stayed where it was");
  console.log(`  spent ${l.spent}, burned ${l.burned}`);

  console.log("graduate: buy the curve, then release to the dev");
  const grad = await launch(t, config, launchParams(dev.publicKey, "graduate"));
  await buy(t, grad.platform, grad.mint, 0, null, 5n * SOL);
  await crankFor(grad.mint, "the crank claims", "kit-fees", "creator-vault");
  await crankFor(grad.mint, "the crank runs Graduate", "kit-buy");
  l = await readLaunch(t, grad.mint);
  expect(l.executions === 1n && l.burned > 0n, "Graduate bought on the curve");
  await fillCurve(env, grad.mint);
  await crankFor(grad.mint, "the crank claims the whale's fees", "kit-fees", "creator-vault");
  const gradKit = await tokenAmount(connection, ata(kitPda(grad.mint)));
  expect(gradKit > 0n, "the whale's fees funded the kit");
  before = await snapshot(connection, [devToken]);
  await crankFor(grad.mint, "the crank releases Graduate", "kit-release");
  d = await deltas(connection, before);
  l = await readLaunch(t, grad.mint);
  expect(d[0] === gradKit && l.released === gradKit, "Graduate released the kit to the dev");
  console.log(`  released ${l.released}`);

  console.log("burn on Raydium");
  const poolAddress = await graduate(env, burn.mint);
  await crankFor(burn.mint, "the crank claims the whale's fees", "kit-fees", "creator-vault");
  expect((await tokenAmount(connection, burnKit)) > 0n, "the whale's fees funded the kit");
  supplyBefore = await supply(connection, burn.mint);
  const spentBefore = (await readLaunch(t, burn.mint)).spent;
  await crankFor(burn.mint, "the crank runs Burn on Raydium", "kit-buy");
  l = await readLaunch(t, burn.mint);
  expect(l.spent > spentBefore, "Burn bought on Raydium");
  expect((await supply(connection, burn.mint)) < supplyBefore, "Burn burned on Raydium");
  console.log(`  spent ${l.spent - spentBefore} on Raydium`);

  const pool = decodeCpmmPool(poolAddress, await connection.getAccountInfo(poolAddress));
  if (!pool) throw new Error("no CPMM pool");
  await airdrop(connection, trader.publicKey, 5);
  await ammBuy(t, pool, burn.mint, 2n * SOL);
  before = await snapshot(connection, [devToken, keeperToken, burnKit, treasuryToken]);
  await crankFor(burn.mint, "the crank collects Raydium creator fees", "amm-creator");
  d = await deltas(connection, before);
  const collected = d.reduce((sum, x) => sum + x, 0n);
  const creatorSide = bps(collected, AMM_CREATOR_BPS);
  const keeperCut = bps(creatorSide, CRANK_BPS);
  const kitCut = bps(collected - creatorSide, KIT_BPS);
  expect(collected > 0n, "Raydium creator fees were collected");
  expect(d[1] === keeperCut && d[0] === creatorSide - keeperCut, "half of it split 95/5 like a creator fee");
  expect(d[2] === kitCut && d[3] === collected - creatorSide - kitCut, "half of it split 70/30 like a platform fee");
  console.log(`  ${collected}: dev +${d[0]}, keeper +${d[1]}, kit +${d[2]}, treasury +${d[3]}`);

  console.log("stake: the kit streams its SOL to stakers");
  const stk = await launch(t, config, launchParams(dev.publicKey, "stake"));
  const stkKit = ata(kitPda(stk.mint));
  const traderBase = getAssociatedTokenAddressSync(stk.mint, trader.publicKey);
  await buy(t, stk.platform, stk.mint, 0, null, 2n * SOL);
  await crankFor(stk.mint, "the crank claims and has no Stake work", "kit-fees", "creator-vault");
  const income = await tokenAmount(connection, stkKit);
  const bought = await tokenAmount(connection, traderBase);
  await stake(t, stk.mint, bought / 2n);
  let sp = await readStakePool(t, stk.mint);
  expect(sp.totalStaked === bought / 2n, "the stake moved into the pool");
  expect(sp.streaming === income && sp.streamEnd === sp.lastUpdate + STREAM, "the first stake starts streaming the kit");
  await new Promise((resolve) => setTimeout(resolve, 5_000));
  before = await snapshot(connection, [ata(trader.publicKey), stkKit]);
  await claimStake(t, stk.mint);
  d = await deltas(connection, before);
  const after = await readStakePool(t, stk.mint);
  const elapsed = after.lastUpdate - sp.lastUpdate;
  expect(elapsed > 0n && after.distributed === (income * elapsed) / STREAM, "the stream paid at its rate");
  expect(d[0] > 0n && d[0] === -d[1] && d[0] <= after.distributed, "the staker claimed its share of it");
  expect(
    (await tokenAmount(connection, stkKit)) === after.streaming + after.distributed - after.claimed,
    "the kit holds exactly what the pool still owes",
  );
  console.log(`  ${elapsed}s streamed ${after.distributed} of ${income}, claimed ${d[0]}`);

  const second = env.open(team);
  await buy(second, stk.platform, stk.mint, 0, null, SOL);
  await crankFor(stk.mint, "the crank claims the second buy", "kit-fees", "creator-vault");
  const secondHeld = await tokenAmount(connection, getAssociatedTokenAddressSync(stk.mint, team.publicKey));
  await stake(second, stk.mint, secondHeld);
  sp = await readStakePool(t, stk.mint);
  expect(sp.totalStaked === bought / 2n + secondHeld, "both stakes count");
  expect(sp.streamEnd === sp.lastUpdate + STREAM, "the new fees restarted the window");
  await unstake(t, stk.mint, bought / 2n);
  expect((await tokenAmount(connection, traderBase)) === bought, "unstaking returned the tokens");
  let position = await readPosition(t, stk.mint, trader.publicKey);
  expect(position.amount === 0n && position.earned > 0n, "unstaking kept what was earned");
  await claimStake(t, stk.mint);
  position = await readPosition(t, stk.mint, trader.publicKey);
  expect(position.earned === 0n && position.claimed > d[0], "the earnings were claimed after unstaking");

  console.log("draw: a random staker wins the pot, weighted by stake");
  await ensureLocalVrf(env);
  const [alice, bob, carol] = await funded(env, 3);
  const [da, db, dc] = [env.open(alice), env.open(bob), env.open(carol)];
  const drw = await launch(da, config, launchParams(dev.publicKey, "draw"));
  const drwKit = ata(kitPda(drw.mint));
  await buy(da, drw.platform, drw.mint, 0, null, 8n * SOL);
  await buy(db, drw.platform, drw.mint, 0, null, SOL);
  await crankFor(drw.mint, "the crank claims and has no Draw work before anyone stakes", "kit-fees", "creator-vault");
  const aliceStake = (await tokenAmount(connection, ata(alice.publicKey, drw.mint))) - 1_000n;
  const bobStake = await tokenAmount(connection, ata(bob.publicKey, drw.mint));
  await stake(da, drw.mint, aliceStake);
  await stake(db, drw.mint, bobStake);
  const tree = await connection.getAccountInfo(drawTreePda(drw.mint));
  expect(tree!.data.readUInt32LE(DRAW_TREE_LEN_OFFSET) === 2, "each staker took a slot in the draw tree");
  let draw = await readDraw(da, drw.mint);
  await crankFor(drw.mint, "nothing is due before the interval");
  await waitForChain(connection, draw.lastDraw + DRAW.interval);

  expect((await tokenAmount(connection, drwKit)) >= DRAW_MIN_POT, "the pot reached the minimum");
  before = await snapshot(connection, [keeperToken, drwKit]);
  const keeperLamports = await connection.getBalance(keeper.publicKey);
  await crankFor(drw.mint, "the crank requests a draw", "draw-request");
  d = await deltas(connection, before);
  const refund = d[0];
  expect(refund > 0n && refund === -d[1], "the pot refunded the keeper's VRF cost in WSOL");
  draw = await readDraw(da, drw.mint);
  expect(draw.pending, "the draw waits for ORAO");
  await expectError("stake while a draw is pending", "DrawInProgress", () => stake(da, drw.mint, 1_000n));
  await expectError("unstake while a draw is pending", "DrawInProgress", () => unstake(da, drw.mint, 1n));
  await crankFor(drw.mint, "nothing to settle before ORAO answers");

  await fulfillDraw(env, draw.seed);
  const keeperNet = BigInt((await connection.getBalance(keeper.publicKey)) - keeperLamports) + refund;
  expect(keeperNet === -5_000n, "once ORAO returned the spare rent, the keeper was out only its transaction fee");
  const pot = await tokenAmount(connection, drwKit);
  await crankFor(drw.mint, "the crank settles the draw", "draw-settle");
  draw = await readDraw(da, drw.mint);
  const aliceWon = draw.lastWinner.equals(alice.publicKey);
  expect(aliceWon || draw.lastWinner.equals(bob.publicKey), "one of the stakers won");
  expect(!draw.pending && draw.round === 1n && draw.lastPrize === pot && draw.won === pot, "the draw paid the whole pot");
  const winner = aliceWon ? da : db;
  expect((await readPosition(winner, drw.mint, draw.lastWinner)).earned === pot, "the winner earned the pot");
  expect((await readStakePool(winner, drw.mint)).distributed === pot, "the pool owes the pot");
  before = await snapshot(connection, [ata(draw.lastWinner), drwKit]);
  await claimStake(winner, drw.mint);
  d = await deltas(connection, before);
  expect(d[0] === pot && d[1] === -pot, "the winner claimed the pot");
  console.log(`  ${aliceWon ? "alice" : "bob"} won ${pot} (stakes ${aliceStake} : ${bobStake}); the request cost ${refund}`);

  console.log("draw: picks too new to win roll the pot over");
  await unstake(da, drw.mint, aliceStake);
  await unstake(db, drw.mint, bobStake);
  await buy(dc, drw.platform, drw.mint, 0, null, 8n * SOL);
  await crankFor(drw.mint, "the crank claims", "kit-fees", "creator-vault");
  await waitForChain(connection, draw.lastDraw + DRAW.interval);
  await stake(dc, drw.mint, await tokenAmount(connection, ata(carol.publicKey, drw.mint)));
  await crankFor(drw.mint, "the crank requests a draw", "draw-request");
  draw = await readDraw(da, drw.mint);
  await fulfillDraw(env, draw.seed);
  const rolled = await tokenAmount(connection, drwKit);
  await crankFor(drw.mint, "the crank settles the draw", "draw-settle");
  draw = await readDraw(da, drw.mint);
  expect(draw.round === 2n && draw.lastWinner.equals(PublicKey.default) && draw.lastPrize === 0n, "nobody won");
  expect((await tokenAmount(connection, drwKit)) === rolled, "the pot stayed in the kit");
  expect((await readPosition(dc, drw.mint, carol.publicKey)).earned === 0n, "the new stake earned nothing");

  console.log("vesting");
  const locked = new BN("10000000000000");
  const vested = await launch(
    t,
    config,
    launchParams(dev.publicKey, "burn", 0, {
      totalLockedAmount: locked,
      cliffPeriod: new BN(0),
      unlockPeriod: new BN(30 * 86_400),
    }),
  );
  const c = env.open(dev);
  await send(c, [await createCreatorVestingIx(c, dev.publicKey, vested.mint, NATIVE_MINT, team.publicKey, locked)], []);
  const record = await connection.getAccountInfo(
    vestingRecordPda(ll, poolStatePda(ll, vested.mint, NATIVE_MINT), team.publicKey),
  );
  expect(record !== null, "create_creator_vesting created the record");
  expect((await connection.getBalance(creatorPda(vested.mint))) === 0, "the creator PDA kept no SOL");

  console.log("keeper loop");
  await runOnce(k, await discover(k), 1n);
  // Kit buys in the pass leave fees under 0.01 SOL, and kits the pass just funded buy next pass.
  const left = selectWork(await findPending(k, await discover(k)), 10_000_000n).filter(
    (p) => (!p.kind.startsWith("kit-") && !p.kind.startsWith("draw-")) || p.kind === "kit-fees",
  );
  expect(left.length === 0, `one keeper pass claimed every fee worth a transaction (left ${left.map(describe).join(", ")})`);

  console.log("local test passed");
});
