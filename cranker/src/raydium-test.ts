// Kits after the move to Raydium, against `scripts/local-validator.sh`. Run after `local-test.ts`
// on the same validator (or a fresh one: it initializes the config if needed). Covers what
// `local-test` leaves out: Floor's checkpoint and buy on the CPMM pool, Graduate releasing
// Raydium fees again, and Stake and Draw paying out kits funded by Raydium creator fees.
// Leave `npm run oracle` off: the test answers ORAO itself.
import type { BN } from "@coral-xyz/anchor";
import { LAMPORTS_PER_SOL, type PublicKey } from "@solana/web3.js";
import { NATIVE_MINT, createAssociatedTokenAccountIdempotentInstruction, getAssociatedTokenAddressSync } from "@solana/spl-token";
import { type Cranker, type Pending, crank, discover, drawTiming, findPending, highRiseSeconds, selectWork } from "./crank.js";
import { type CpmmPool, decodeCpmmPool, sides, swapIx } from "./cpmm.js";
import { graduate } from "./graduate.js";
import {
  airdrop,
  ammBuy,
  ata,
  buy,
  chainTime,
  claimStake,
  deltas,
  ensureConfig,
  ensureLocalVrf,
  expect,
  fulfillDraw,
  funded,
  launch,
  launchParams,
  openEnv,
  readDraw,
  run,
  send,
  snapshot,
  stake,
  supply,
  tokenAmount,
  waitForChain,
} from "./test-env.js";
import { kitPda, launchPda } from "./ids.js";

const SOL = BigInt(LAMPORTS_PER_SOL);
const DRAW = drawTiming("localnet");

function readLaunch(c: Cranker, mint: PublicKey) {
  return c.connection.getAccountInfo(launchPda(mint)).then((info) => {
    const l = c.labkit.coder.accounts.decode("launch", info!.data);
    const big = (v: BN) => BigInt(v.toString());
    return {
      highQ64: big(l.highQ64),
      highAt: Number(l.highAt),
      spent: big(l.spent),
      burned: big(l.burned),
      released: big(l.released),
      executions: big(l.executions),
    };
  });
}

/** Sells `amount` raw tokens on a CPMM pool; the SOL lands in the wallet's WSOL account. */
async function ammSell(c: Cranker, pool: CpmmPool, baseMint: PublicKey, amount: bigint): Promise<void> {
  const user = c.payer.publicKey;
  const { base } = sides(pool, NATIVE_MINT);
  const userBase = getAssociatedTokenAddressSync(baseMint, user);
  await send(
    c,
    [
      createAssociatedTokenAccountIdempotentInstruction(user, ata(user), user, NATIVE_MINT),
      swapIx(c.cluster, pool, user, base, { input: userBase, output: ata(user) }, amount, 0n),
    ],
    [],
  );
}

run(async () => {
  const env = await openEnv();
  const { connection } = env;
  const [trader, keeper, dev, alice] = await funded(env, 4);
  await airdrop(connection, trader.publicKey, 40);
  const config = await ensureConfig(env);
  const t = env.open(trader);
  const k = env.open(keeper);
  const devToken = ata(dev.publicKey);

  /** Runs every pass of one launch's work until none is left. Returns the kinds that ran. */
  const crankAll = async (mint: PublicKey): Promise<Set<Pending["kind"]>> => {
    const ran = new Set<Pending["kind"]>();
    for (let pass = 0; pass < 6; pass++) {
      const work = selectWork(await findPending(k, await discover(k)), 1n).filter(
        (p) => p.kind !== "fee-wallet" && p.launch.baseMint.equals(mint),
      );
      if (work.length === 0) break;
      for (const p of work) {
        await crank(k, config, p);
        ran.add(p.kind);
      }
    }
    return ran;
  };
  const graduated = async (mint: PublicKey) => {
    const address = await graduate(env, mint);
    const pool = decodeCpmmPool(address, await connection.getAccountInfo(address));
    if (!pool) throw new Error("no CPMM pool");
    return pool;
  };

  console.log("floor on Raydium: checkpoints walk the high up, then it buys under the line");
  const floor = await launch(t, config, launchParams(dev.publicKey, "floor", 1_000));
  await buy(t, floor.platform, floor.mint, 0, null, 20n * SOL);
  const floorPool = await graduated(floor.mint);
  await crankAll(floor.mint);
  let l = await readLaunch(t, floor.mint);
  const executionsBefore = l.executions;
  let checkpoints = 0;
  for (;;) {
    await waitForChain(connection, l.highAt + highRiseSeconds("localnet"));
    const ran = await crankAll(floor.mint);
    l = await readLaunch(t, floor.mint);
    if (!ran.has("kit-checkpoint")) break;
    checkpoints++;
  }
  expect(checkpoints > 0, `checkpoints raised the high on the Raydium price (${checkpoints} of them)`);
  expect(l.executions === executionsBefore, "no buy while the price sat above the line");
  const kitFunds = await tokenAmount(connection, ata(kitPda(floor.mint)));
  expect(kitFunds > 0n, "the kit holds SOL to defend with");
  const held = await tokenAmount(connection, getAssociatedTokenAddressSync(floor.mint, trader.publicKey));
  const supplyBefore = await supply(connection, floor.mint);
  let sold = 0;
  for (let part = 4n; part >= 1n && l.executions === executionsBefore; part--) {
    await ammSell(t, floorPool, floor.mint, held / 5n);
    sold++;
    await crankAll(floor.mint);
    l = await readLaunch(t, floor.mint);
  }
  expect(l.executions > executionsBefore, `Floor bought on Raydium after ${sold} sell(s)`);
  expect((await supply(connection, floor.mint)) < supplyBefore, "and burned what it bought");
  console.log(`  ${checkpoints} checkpoints, bought after ${sold} sell(s), spent ${l.spent}, burned ${l.burned}`);

  console.log("graduate on Raydium: later Raydium fees reach the dev too");
  const grad = await launch(t, config, launchParams(dev.publicKey, "graduate"));
  const gradPool = await graduated(grad.mint);
  await crankAll(grad.mint);
  const releasedAtMigration = (await readLaunch(t, grad.mint)).released;
  expect(releasedAtMigration > 0n, "the kit was released after the move");
  await ammBuy(t, gradPool, grad.mint, 5n * SOL);
  let before = await snapshot(connection, [devToken]);
  let ran = await crankAll(grad.mint);
  let d = await deltas(connection, before);
  l = await readLaunch(t, grad.mint);
  expect(ran.has("amm-creator") && ran.has("kit-release"), `the keeper collected and released (ran ${[...ran].join(", ")})`);
  expect(l.released > releasedAtMigration && d[0] > 0n, "the Raydium fees' kit share went to the dev");
  console.log(`  released ${l.released - releasedAtMigration} more, dev +${d[0]}`);

  console.log("stake on Raydium: Raydium fees stream to stakers");
  const stk = await launch(t, config, launchParams(dev.publicKey, "stake"));
  await buy(t, stk.platform, stk.mint, 0, null, 2n * SOL);
  const stkPool = await graduated(stk.mint);
  await crankAll(stk.mint);
  await stake(t, stk.mint, await tokenAmount(connection, getAssociatedTokenAddressSync(stk.mint, trader.publicKey)));
  const stkKit = ata(kitPda(stk.mint));
  const kitBefore = await tokenAmount(connection, stkKit);
  await ammBuy(t, stkPool, stk.mint, 5n * SOL);
  ran = await crankAll(stk.mint);
  expect(ran.has("amm-creator") && (await tokenAmount(connection, stkKit)) > kitBefore, "Raydium fees funded the kit");
  await new Promise((resolve) => setTimeout(resolve, 4_000));
  before = await snapshot(connection, [ata(trader.publicKey)]);
  await claimStake(t, stk.mint);
  d = await deltas(connection, before);
  expect(d[0] > 0n, "the staker claimed from the stream");
  console.log(`  staker claimed ${d[0]}`);

  console.log("draw on Raydium: a pot from Raydium fees goes to a staker");
  await ensureLocalVrf(env);
  const a = env.open(alice);
  const drw = await launch(a, config, launchParams(dev.publicKey, "draw"));
  await buy(a, drw.platform, drw.mint, 0, null, 2n * SOL);
  await stake(a, drw.mint, await tokenAmount(connection, getAssociatedTokenAddressSync(drw.mint, alice.publicKey)));
  const drwPool = await graduated(drw.mint);
  await airdrop(connection, trader.publicKey, 20);
  await ammBuy(t, drwPool, drw.mint, 10n * SOL);
  let draw = await readDraw(a, drw.mint);
  await waitForChain(connection, Math.max(draw.lastDraw + DRAW.interval, (await chainTime(connection)) + 1));
  ran = await crankAll(drw.mint);
  expect(ran.has("amm-creator") && ran.has("draw-request"), `the keeper collected and requested a draw (ran ${[...ran].join(", ")})`);
  draw = await readDraw(a, drw.mint);
  expect(draw.pending, "the draw waits for ORAO");
  await fulfillDraw(env, draw.seed);
  const pot = await tokenAmount(connection, ata(kitPda(drw.mint)));
  ran = await crankAll(drw.mint);
  draw = await readDraw(a, drw.mint);
  expect(ran.has("draw-settle") && draw.lastWinner.equals(alice.publicKey), "the only old-enough staker won");
  expect(draw.lastPrize === pot && pot > 0n, "the prize was the whole pot");
  before = await snapshot(connection, [ata(alice.publicKey)]);
  await claimStake(a, drw.mint);
  d = await deltas(connection, before);
  expect(d[0] === pot, "the winner claimed the pot");
  console.log(`  alice won ${pot}`);

  console.log("raydium test passed");
});
