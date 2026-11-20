// Rewrites `migrate_to_cpswap_wallet` in a `solana account --output json` dump of a LaunchLab GlobalConfig.
// Usage: node patch-global-config.mjs <account.json> <wallet>
import fs from "node:fs";

// discriminator, epoch, curve_type, index, eight u64 limits, then four pubkeys
const MIGRATE_TO_CPSWAP_WALLET = 8 + 8 + 1 + 2 + 8 * 8 + 32 * 4;
const BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

function decodeBase58(text) {
  let n = 0n;
  for (const ch of text) {
    const digit = BASE58.indexOf(ch);
    if (digit < 0) throw new Error(`invalid base58 ${text}`);
    n = n * 58n + BigInt(digit);
  }
  const bytes = [];
  for (; n > 0n; n >>= 8n) bytes.unshift(Number(n & 0xffn));
  for (const ch of text) {
    if (ch !== "1") break;
    bytes.unshift(0);
  }
  if (bytes.length !== 32) throw new Error(`${text} is not a public key`);
  return Buffer.from(bytes);
}

const [file, wallet] = process.argv.slice(2);
// Edit the text in place: `rentEpoch` is u64::MAX, which a JSON round trip would turn into a float.
const text = fs.readFileSync(file, "utf8");
const encoded = JSON.parse(text).account.data[0];
const data = Buffer.from(encoded, "base64");
decodeBase58(wallet).copy(data, MIGRATE_TO_CPSWAP_WALLET);
fs.writeFileSync(file, text.replace(encoded, data.toString("base64")));
