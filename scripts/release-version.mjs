#!/usr/bin/env node
/**
 * Bumps the app version everywhere it is recorded.
 *
 *   node scripts/release-version.mjs patch|minor|major|<x.y.z>
 *
 * package.json is the source of truth: tauri.conf.json reads its version
 * from there. The Cargo workspace version (and the workspace entries in
 * Cargo.lock) are kept in step so `CARGO_PKG_VERSION` matches the app.
 * Prints the new version; inside GitHub Actions it is also written to
 * `$GITHUB_OUTPUT` as `version`.
 */

import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const SEMVER = /^(\d+)\.(\d+)\.(\d+)$/;

/** The next version for a bump kind, or an explicit `x.y.z`. */
export function nextVersion(current, bump) {
  const match = SEMVER.exec(current);
  if (!match) throw new Error(`current version "${current}" is not x.y.z`);
  const [major, minor, patch] = match.slice(1).map(Number);
  switch (bump) {
    case "major":
      return `${major + 1}.0.0`;
    case "minor":
      return `${major}.${minor + 1}.0`;
    case "patch":
      return `${major}.${minor}.${patch + 1}`;
    default:
      if (!SEMVER.test(bump)) throw new Error(`bump must be patch, minor, major or x.y.z, got "${bump}"`);
      return bump;
  }
}

/** Sets `version` in the `[workspace.package]` table of the root Cargo.toml. */
export function setCargoWorkspaceVersion(toml, version) {
  const section = /(\[workspace\.package\][^[]*?\nversion\s*=\s*")[^"]*(")/;
  if (!section.test(toml)) throw new Error("Cargo.toml has no [workspace.package] version");
  return toml.replace(section, `$1${version}$2`);
}

/**
 * Sets the version of the workspace's own packages in Cargo.lock: the
 * `[[package]]` entries without a `source` (path packages) at `from`.
 */
export function setCargoLockVersions(lock, from, to) {
  return lock
    .split("\n[[package]]\n")
    .map((block) => {
      if (/^source = /m.test(block)) return block;
      return block.replace(new RegExp(`^version = "${from.replaceAll(".", "\\.")}"$`, "m"), `version = "${to}"`);
    })
    .join("\n[[package]]\n");
}

function main() {
  const [bump] = process.argv.slice(2);
  if (!bump) {
    console.error("usage: node scripts/release-version.mjs patch|minor|major|<x.y.z>");
    process.exit(2);
  }

  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  const path = (file) => join(root, file);

  const pkg = JSON.parse(readFileSync(path("package.json"), "utf8"));
  const current = pkg.version;
  const version = nextVersion(current, bump);

  pkg.version = version;
  writeFileSync(path("package.json"), `${JSON.stringify(pkg, null, 2)}\n`);
  writeFileSync(path("Cargo.toml"), setCargoWorkspaceVersion(readFileSync(path("Cargo.toml"), "utf8"), version));
  writeFileSync(path("Cargo.lock"), setCargoLockVersions(readFileSync(path("Cargo.lock"), "utf8"), current, version));

  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `version=${version}\n`);
  console.log(version);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
