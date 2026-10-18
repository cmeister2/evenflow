import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { devNull, tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { expectedAssets, prepare, verifyAssets, verifyRelease } from './plugin.mjs';

const version = '1.2.3';
const env = { ...process.env, GIT_CONFIG_GLOBAL: devNull, GIT_CONFIG_NOSYSTEM: '1' };

async function fixture(t) {
  const cwd = await mkdtemp(join(tmpdir(), 'evenflow-release-assets-'));
  t.after(() => rm(cwd, { recursive: true, force: true }));
  const git = (...args) => execFileSync('git', args, { cwd, env, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  git('init', '--quiet', '--initial-branch=main');
  git('-c', 'user.name=Release tests', '-c', 'user.email=release@example.invalid', '-c', 'commit.gpgSign=false', 'commit', '--quiet', '--allow-empty', '-m', 'feat: initial source');
  const commit = git('rev-parse', 'HEAD');
  const directory = join(cwd, 'dist');
  await mkdir(directory);
  await mkdir(join(cwd, 'release-prepared'));
  await writeFile(join(cwd, 'release-prepared', 'plan.json'), JSON.stringify({ version, commit, releasing: true }));
  for (const name of expectedAssets(version)) await writeFile(join(directory, name), `artifact: ${name}\n`);
  return { cwd, directory, commit, context: { cwd, env, nextRelease: { version, gitHead: commit } } };
}

test('all four artifacts are required and receive deterministic content checksums', async (t) => {
  const { cwd, directory, context } = await fixture(t);
  await verifyRelease({}, context);
  let published = 0;
  const publisher = { publish: async (options) => {
    assert.equal(options.cwd, cwd);
    assert.equal(options.version, version);
    assert.equal(options.env, context.env);
    published += 1;
  } };
  await prepare({}, context, publisher);
  assert.equal(published, 1);
  const checksums = await readFile(join(directory, 'SHA256SUMS'), 'utf8');
  const expected = expectedAssets(version).map((name) => {
    const digest = createHash('sha256').update(`artifact: ${name}\n`).digest('hex');
    return `${digest}  ${name}\n`;
  }).join('');
  assert.equal(checksums, expected);
  await prepare({}, context, publisher);
  assert.equal(published, 2);
  assert.equal(await readFile(join(cwd, 'dist', 'SHA256SUMS'), 'utf8'), checksums);
  await writeFile(join(directory, expectedAssets(version)[0]), 'changed artifact');
  await assert.rejects(verifyRelease({}, context), /SHA256SUMS does not match/);
});

test('a registry publication failure stops preparation before checksums are written', async (t) => {
  const { directory, context } = await fixture(t);
  const failure = new Error('Registry publication failed');
  let calls = 0;
  await assert.rejects(prepare({}, context, { publish: async () => {
    calls += 1;
    await assert.rejects(readFile(join(directory, 'SHA256SUMS')), { code: 'ENOENT' });
    throw failure;
  } }), (error) => error === failure);
  assert.equal(calls, 1);
  await assert.rejects(readFile(join(directory, 'SHA256SUMS')), { code: 'ENOENT' });
});

test('final checksums cover the canonical Cargo archive produced by publication', async (t) => {
  const { directory, context } = await fixture(t);
  const name = `evenflow-${version}.crate`;
  const canonical = Buffer.from('Canonical Cargo publication archive\n');
  const original = await readFile(join(directory, name));
  let calls = 0;
  await prepare({}, context, { publish: async () => {
    calls += 1;
    await assert.rejects(readFile(join(directory, 'SHA256SUMS')), { code: 'ENOENT' });
    await writeFile(join(directory, name), canonical);
  } });
  assert.equal(calls, 1);
  const checksums = await readFile(join(directory, 'SHA256SUMS'), 'utf8');
  const digest = createHash('sha256').update(canonical).digest('hex');
  const originalDigest = createHash('sha256').update(original).digest('hex');
  assert.ok(checksums.includes(`${digest}  ${name}\n`));
  assert.ok(!checksums.includes(`${originalDigest}  ${name}\n`));
  assert.equal(checksums.trim().split('\n').length, 4);
  await verifyRelease({}, context);
});

test('invalid release assets prevent the registry publisher from running', async (t) => {
  const { directory, context } = await fixture(t);
  await rm(join(directory, `evenflow-${version}.crate`));
  let calls = 0;
  await assert.rejects(prepare({}, context, { publish: async () => { calls += 1; } }), /exactly the three target archives/);
  assert.equal(calls, 0);
  await assert.rejects(readFile(join(directory, 'SHA256SUMS')), { code: 'ENOENT' });
});

test('publication rejects a different planned version, planned commit, checkout, or no-release plan', async (t) => {
  const { cwd, context, commit } = await fixture(t);
  for (const plan of [
    { version: '9.9.9', commit, releasing: true },
    { version, commit: 'a'.repeat(40), releasing: true },
    { version, commit, releasing: false },
  ]) {
    await writeFile(join(cwd, 'release-prepared', 'plan.json'), JSON.stringify(plan));
    await assert.rejects(verifyRelease({}, context), /must match the tested release plan/);
    await assert.rejects(prepare({}, context), /must match the tested release plan/);
  }
  await writeFile(join(cwd, 'release-prepared', 'plan.json'), JSON.stringify({ version, commit, releasing: true }));
  await assert.rejects(verifyRelease({}, { ...context, nextRelease: { version, gitHead: 'b'.repeat(40) } }), /must match the tested release plan/);
});

test('a dry-run preview does not need planned builds or artifacts', async () => {
  await verifyRelease({}, { options: { dryRun: true } });
});

test('missing, extra, stale and empty archives prevent publication', async (t) => {
  const { directory } = await fixture(t);
  const name = expectedAssets(version)[0];
  const path = join(directory, name);
  await rm(path);
  await assert.rejects(verifyAssets(directory, version), /exactly the three target archives/);
  await writeFile(path, 'restored');
  await writeFile(join(directory, 'evenflow-0.1.0-stale.zip'), 'stale');
  await assert.rejects(verifyAssets(directory, version), /exactly the three target archives/);
  await rm(join(directory, 'evenflow-0.1.0-stale.zip'));
  await writeFile(path, '');
  await assert.rejects(verifyAssets(directory, version), /nonempty regular file/);
});

test('directories and symlinks cannot masquerade as release artifacts', async (t) => {
  const { cwd, directory } = await fixture(t);
  const path = join(directory, expectedAssets(version)[0]);
  await rm(path);
  await mkdir(path);
  await assert.rejects(verifyAssets(directory, version), /nonempty regular file/);
  await rm(path, { recursive: true });
  const outside = join(cwd, 'outside.tar.gz');
  await writeFile(outside, 'outside');
  await symlink(outside, path);
  await assert.rejects(verifyAssets(directory, version), /nonempty regular file/);
});
