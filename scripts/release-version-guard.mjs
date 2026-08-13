// The one rule that makes updates work (docs/release.md): the version the
// updater compares is tauri.conf.json's; the tag only triggers the
// workflow. When they disagree the release publishes under one number
// while advertising another and clients see no update, silently (v0.4.0
// shipped 0.2.0 artifacts exactly this way). Fail before spending a build.
//
//   node scripts/release-version-guard.mjs v26.7.0
import { readFileSync } from "node:fs";

const tag = (process.argv[2] ?? "").replace(/^v/, "");
if (!tag) {
  console.error("usage: release-version-guard.mjs <tag>");
  process.exit(2);
}
// Calendar versioning (docs/release.md): YY.MM.PATCH, month without a
// leading zero. An old-style or zero-padded tag fails before the build
// spends anything.
if (!/^\d{2}\.([1-9]|1[0-2])\.\d+$/.test(tag)) {
  console.error(
    `::error::tag ${tag} is not YY.MM.PATCH calendar versioning (e.g. 26.7.0; no leading zero on the month — see docs/release.md)`,
  );
  process.exit(1);
}
const conf = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8")).version;
// [package] is the first table, so its version line is the first match.
const cargo = readFileSync("src-tauri/Cargo.toml", "utf8").match(/^version = "(.+)"$/m)?.[1];

let bad = false;
if (conf !== tag) {
  console.error(`::error::tauri.conf.json is ${conf}, tag is ${tag}`);
  bad = true;
}
if (cargo !== tag) {
  console.error(`::error::src-tauri/Cargo.toml is ${cargo}, tag is ${tag}`);
  bad = true;
}
if (bad) {
  console.error(`::error::Bump the version files to ${tag} and re-tag (see docs/release.md).`);
  process.exit(1);
}
console.log(`version ${tag} agrees across tag, tauri.conf.json, and Cargo.toml`);
