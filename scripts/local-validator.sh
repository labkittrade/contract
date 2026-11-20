#!/usr/bin/env bash
# Local validator with LabKit, plus the Raydium LaunchLab, CPMM and LP lock programs and
# ORAO VRF cloned from mainnet. `npm run oracle` in the cranker answers VRF requests.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
ledger="${LABKIT_LEDGER:-/tmp/labkit-validator}"
url="${LABKIT_CLONE_URL:-https://api.mainnet-beta.solana.com}"
# `initialize` must be signed by the upgrade authority, so load LabKit as upgradeable.
authority="${LABKIT_UPGRADE_AUTHORITY:-$HOME/.config/solana/id.json}"

# Only LaunchLab's migrate wallet may move a filled curve to Raydium. The local SOL
# GlobalConfig names the local key instead, so `npm run graduate` can do it.
global_config=6s1xP3hpbAfFoNtUNF8mfHsjr2Bd97JxFJRWLbL6aHuX
migrator="$(solana-keygen pubkey "$authority")"
fixture="$ledger-global-config.json"
solana account "$global_config" --url "$url" --output json --output-file "$fixture" >/dev/null
node "$root/scripts/patch-global-config.mjs" "$fixture" "$migrator"

# The default keeps 10,000 shreds, about ten minutes of slots, which prunes transactions
# the indexer and explorer still need.
exec solana-test-validator \
  --reset \
  --ledger "$ledger" \
  --limit-ledger-size 50000000 \
  --url "$url" \
  --clone-upgradeable-program LanMV9sAd7wArD4vJFi2qDdfnVhFxYSUg6eADduJ3uj \
  --clone-upgradeable-program metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s \
  --clone-upgradeable-program CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C \
  --clone-upgradeable-program LockrWmn6K5twhz3y9w1dQERbmgSaRkfnTeTKbpofwE \
  --clone-upgradeable-program VRFzZoJdhFWL8rkvu87LpKM3RbcVezpMEc6X5GVDr7y \
  --account "$global_config" "$fixture" \
  --clone D4FPEruKEHrG5TenZ2mpDGEfu1iUvTiqBxvpU8HLBvC2 \
  --clone CRRS5ieQmBrZjWhcj99JuGrT5tyuWDaGAXLXLFjbAtjQ \
  --clone DNXgeM9EiiaAbaWvwjHj9fQQLAX5ZsfHyvmYUNRAdNC8 \
  --clone 7NrevmQXcRTRGsgruwTCwWkn9qwA29rXcMwfenWunHP \
  --upgradeable-program 133fUn4mjvLVKoe6oR7BWwAiF4qtFgmkg35BdegqgMG3 \
    "$root/target/deploy/labkit.so" "$authority" \
  "$@"
