# LabKit

LabKit is an on-chain launchpad layer for [Raydium LaunchLab](https://raydium.io/launchpad/). Every launch picks a kit, and the launch's trading fees fund that kit for good: on the bonding curve, and on Raydium CPMM after the token graduates.

Program id: `133fUn4mjvLVKoe6oR7BWwAiF4qtFgmkg35BdegqgMG3`

## Kits

| Kit | What the kit's SOL does |
| --- | --- |
| Burn | Buys the token and burns it, on the curve and on Raydium. |
| Floor | Buys and burns only while the price is below a line set under the highest price seen. The launch picks how far below (1% to 90%). |
| Graduate | Buys on the curve to help it fill. Once it graduates, the kit pays out to the creator. |
| Stake | Streams to holders who stake the token, pro rata, over 3 days from each deposit. |
| Draw | Once a day, when the pot holds at least 0.05 SOL, pays the whole pot to one staker picked at random by [ORAO VRF](https://orao.network/), weighted by stake. Only stakes older than a day can win. |

## How a launch works

1. `create_launch`, signed by the base mint keypair, creates the launch's own LaunchLab platform from the config's template and fixes the kit. The platform's fee wallet is the kit PDA `["kit", base_mint]`. Its admin is the PDA `["platform_admin", base_mint]`, which LabKit never signs for again, so nobody can change the platform afterwards.
2. LaunchLab `initialize_v2` on that platform, with `creator` set to the PDA `["creator", base_mint]`.
3. `register_launch` checks that the pool matches the launch: its platform, base mint, creator, and a SOL quote. Token-2022 pools and pools taking Raydium creator fees in both tokens are refused.

`cranker/src/launch.ts` builds these. A frontend builds the same transactions.

No wallet can withdraw from a LabKit PDA. Funds leave only through the instructions below, and every split follows fixed rules.

## Fees

| Fee | Where it goes |
| --- | --- |
| Platform fee on curve trades | `kit_bps` to the kit, the rest to the treasury. |
| Creator fee on curve trades | `crank_bps` to whoever cranks the claim, the rest to the launch's recipient. |
| Raydium creator fee after graduation | `amm_creator_bps` of it is split like a creator fee; the rest like a platform fee. |
| Referral fees sent to the fee wallet `["fee_wallet"]` | The treasury. |

The splits are copied into each launch when it is created, so `update_config` never changes the terms of an existing launch. `crank_bps` is at most 500.

| Instruction | What it does |
| --- | --- |
| `claim_kit_fees` | Claims the launch platform's fees from LaunchLab and splits them. |
| `claim_creator_fees` | Claims the curve's creator fees from LaunchLab and splits them. |
| `split_creator_fees` | Splits what sits in the creator PDA's WSOL account, which is Raydium creator fees after graduation. |
| `split_fees` | Sends the fee wallet's referral fees to the treasury. |
| `set_launch_recipient` | Changes a launch's recipient. Only the current recipient signs. |
| `create_creator_vesting` | Allots locked supply to a beneficiary through LaunchLab's vesting. Only the recipient signs. |

## Kit instructions

Anyone can send these. The keeper does it automatically.

| Instruction | What it does |
| --- | --- |
| `execute_curve`, `execute_swap` | Spend the kit's SOL on a buy, on the curve or on Raydium CPMM, and burn the tokens (Burn, Floor, Graduate). One buy spends at most 1% of the pool's SOL. |
| `checkpoint` | Raises Floor's recorded high, by at most 10% per hour, so a one-off price spike can't set it. |
| `release` | Pays a Graduate kit to the recipient once the curve has completed. |
| `open_stake_pool`, `stake`, `unstake`, `claim_stake_rewards` | Stake kits, and Draw stakers and winners. |
| `open_draw`, `request_draw`, `settle_draw`, `cancel_draw` | Draw kits. `request_draw` asks ORAO for randomness and freezes stakes until `settle_draw`. `cancel_draw` drops a request ORAO hasn't answered within an hour. |

## Admin

| Instruction | Signer |
| --- | --- |
| `initialize` | The program upgrade authority, once. Sets the treasury, the fee splits, and the platform template. |
| `update_config` | The LabKit authority. Existing launches keep their terms. |
| `transfer_authority` / `accept_authority` | The current authority, then the new one. Pass the default key to cancel. |

## Build

Anchor 0.30.1, from the repo root:

```bash
anchor build
cp target/idl/labkit.json cranker/idl/labkit.json
```

Features:

- `devnet` targets the devnet LaunchLab and CPMM programs.
- `local-timing` shortens the draw, stake-age, and Floor timers to seconds for local testing. Never deploy it.

`Cargo.lock` pins a few crates (see `Cargo.toml`) so Anchor 0.30.1 builds.

The verified mainnet build uses the classic SBF format (v0). Newer Solana CLIs default to a newer format, so to test locally what mainnet runs, build with `cargo build-sbf --manifest-path programs/labkit/Cargo.toml --arch v0`.

`cranker/idl/raydium_launchpad.json` is the LaunchLab IDL from mainnet (`anchor idl fetch LanMV9sAd7wArD4vJFi2qDdfnVhFxYSUg6eADduJ3uj --provider.cluster mainnet`). LabKit reads a few LaunchLab and CPMM account fields at fixed offsets (`programs/labkit/src/launchlab.rs`, `cpmm.rs`), so a layout change in either program would need an update here.

## Verifying the deployed program

The mainnet program is built with [`solana-verify`](https://github.com/Ellipsis-Labs/solana-verifiable-build) in the `solanafoundation/solana-verifiable-build:4.3.0` image, so anyone can rebuild it from this repository and compare hashes:

```bash
solana-verify verify-from-repo -u https://api.mainnet-beta.solana.com \
  --program-id 133fUn4mjvLVKoe6oR7BWwAiF4qtFgmkg35BdegqgMG3 \
  --base-image solanafoundation/solana-verifiable-build:4.3.0 \
  https://github.com/labkittrade/contract --library-name labkit
```

Or by hand, with Docker running:

```bash
solana-verify build --library-name labkit --base-image solanafoundation/solana-verifiable-build:4.3.0
solana-verify get-executable-hash target/deploy/labkit.so
solana-verify get-program-hash -u https://api.mainnet-beta.solana.com 133fUn4mjvLVKoe6oR7BWwAiF4qtFgmkg35BdegqgMG3
```

## Local validator and tests

`scripts/local-validator.sh` clones LaunchLab, Metaplex Token Metadata, Raydium CPMM, the Raydium LP lock program, ORAO VRF, and the accounts they need from mainnet. It loads `target/deploy/labkit.so` as an upgradeable program owned by `~/.config/solana/id.json`, and patches LaunchLab's `GlobalConfig` (`scripts/patch-global-config.mjs`) so that key can graduate launches. It always starts from a clean ledger.

```bash
anchor build -- --features local-timing
bash scripts/local-validator.sh
```

In another terminal:

```bash
cd cranker
npm install
npm run negative-test
npm run local-test
npm run raydium-test
```

- `negative-test` needs a fresh validator because it initializes LabKit itself. It checks that every instruction refuses the wrong signer, out-of-range config values, foreign pools, vaults, and PDAs, Token-2022 pools, token accounts that don't belong to the treasury or recipient, and more.
- `local-test` covers the fee splits, every kit on the curve, Burn on Raydium, Draw's weighted pick and rollover, referrals, and creator vesting.
- `raydium-test` covers the kits after graduation: Floor's checkpoint and buy on CPMM, Graduate releasing later Raydium fees, and Stake and Draw paid from Raydium creator fees.
- Unit tests: `cargo test --manifest-path programs/labkit/Cargo.toml`, with and without `--features local-timing`.

To test without disturbing a running validator, start a second one on other ports:

```bash
LABKIT_LEDGER=/tmp/labkit-test-validator bash scripts/local-validator.sh \
  --rpc-port 8999 --faucet-port 9999 --gossip-port 8990 --dynamic-port-range 9100-9200
LABKIT_RPC_URL=http://127.0.0.1:8999 npm run negative-test
```

`npm run graduate -- <base mint>` fills a launch's curve and moves it to Raydium the way LaunchLab's migration wallet does on mainnet. `npm run oracle` answers ORAO requests on localnet, so Draw launches can settle.

## Keeper

The keeper claims and splits fees and runs the kits for every launch. Anyone can run one, and it earns `crank_bps` of creator-side fees.

```bash
cd cranker
npm run crank -- status
npm run crank -- once
npm run crank -- run
```

Options:

- `--cluster` is `mainnet` (the default), `devnet`, or `localnet`. `--url` overrides the RPC.
- `--keypair` defaults to `~/.config/solana/id.json`.
- `--min` is the smallest balance, in lamports, worth a transaction. The default is 1000000.

The keeper pays rent for any token account it needs to create. A pass that hangs exits the process, so run it under a supervisor (systemd, pm2) that restarts it.

Setup, run once by the program upgrade authority:

```bash
npm run crank -- initialize --treasury <TREASURY> --crank-bps 500 --kit-bps 7000 \
  --amm-creator-bps 5000 --fee-bps 100 --creator-fee-bps 50 --web <SITE> --img <LOGO URL>
```

`npm run crank -- help` lists every option.

## Layout

```
programs/labkit/   Anchor program
cranker/           Keeper, admin CLI, launch builder, local tests, and IDLs
scripts/           Local validator and its GlobalConfig patch
```

## Security

See [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE)
