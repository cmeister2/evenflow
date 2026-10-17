import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { lstat, readFile, readdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { validateVersion } from './version.mjs';

/** Every release carries all supported executables and its Cargo source package. */
export function expectedAssets(version) {
  validateVersion(version);
  return [
    `evenflow-${version}-x86_64-unknown-linux-gnu.tar.gz`,
    `evenflow-${version}-aarch64-apple-darwin.tar.gz`,
    `evenflow-${version}-x86_64-pc-windows-msvc.zip`,
    `evenflow-${version}.crate`,
  ].sort();
}

/** Validate ordinary, nonempty files without following symlinks. */
async function regularFile(path) {
  const stat = await lstat(path);
  if (!stat.isFile() || stat.size === 0) throw new Error(`Release input must be a nonempty regular file: ${path}`);
  return readFile(path);
}

/** Validate the exact release asset set and compute a deterministic checksum file. */
export async function verifyAssets(directory, version) {
  const stat = await lstat(directory);
  if (!stat.isDirectory()) throw new Error('Release assets must reside in an ordinary directory.');
  const names = (await readdir(directory)).sort();
  const expected = expectedAssets(version);
  const actual = names.filter((name) => name !== 'SHA256SUMS');
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`Release assets must contain exactly the three target archives and Cargo source package for ${version}.`);
  }
  const lines = [];
  for (const name of expected) {
    const bytes = await regularFile(join(directory, name));
    lines.push(`${createHash('sha256').update(bytes).digest('hex')}  ${name}\n`);
  }
  const checksums = lines.join('');
  if (names.includes('SHA256SUMS')) {
    const previous = (await regularFile(join(directory, 'SHA256SUMS'))).toString('utf8');
    if (previous !== checksums) throw new Error('The existing SHA256SUMS does not match the release assets.');
  }
  return checksums;
}

/** Bind publication to the planned checkout and artifacts before any tag is created. */
async function verifyPlan(context) {
  const cwd = context.cwd ?? process.cwd();
  const plan = JSON.parse(await regularFile(join(cwd, 'release-prepared', 'plan.json')));
  const commit = execFileSync('git', ['rev-parse', 'HEAD'], {
    cwd, env: context.env ?? process.env, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
  }).trim();
  const version = validateVersion(context.nextRelease.version);
  if (plan.releasing !== true || plan.version !== version || plan.commit !== commit
      || context.nextRelease.gitHead !== commit || !/^[\da-f]{40,64}$/.test(commit)) {
    throw new Error('Release version and commit must match the tested release plan and current checkout.');
  }
  return { directory: join(cwd, 'dist'), version };
}

/** semantic-release runs this guard before preparing or creating a release tag. */
export async function verifyRelease(_options, context) {
  if (context.options?.dryRun) return;
  const { directory, version } = await verifyPlan(context);
  await verifyAssets(directory, version);
}

/** Recheck the inputs, then add the checksums attached by the GitHub plugin. */
export async function prepare(_options, context) {
  const { directory, version } = await verifyPlan(context);
  const checksums = await verifyAssets(directory, version);
  await writeFile(join(directory, 'SHA256SUMS'), checksums);
}
