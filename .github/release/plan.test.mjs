import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { appendFile, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { devNull, tmpdir } from 'node:os';
import { join } from 'node:path';
import { Writable } from 'node:stream';
import test from 'node:test';
import { pathToFileURL } from 'node:url';
import config from '../../release.config.mjs';
import { createPlan } from './plan.mjs';

const env = {
  PATH: process.env.PATH, HOME: process.env.HOME, CI: 'false',
  GIT_CONFIG_GLOBAL: devNull, GIT_CONFIG_NOSYSTEM: '1', GIT_ALLOW_PROTOCOL: 'file', GIT_TERMINAL_PROMPT: '0',
};
const manifest = '[package]\nname = "evenflow"\nversion = "0.1.0"\nedition = "2024"\n';
const lockfile = 'version = 4\n\n[[package]]\nname = "evenflow"\nversion = "0.1.0"\n';
const quiet = () => new Writable({ write(_chunk, _encoding, done) { done(); } });

async function fixture(t, message, tag) {
  const directory = await mkdtemp(join(tmpdir(), 'evenflow-release-plan-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const cwd = join(directory, 'checkout');
  const remote = join(directory, 'remote.git');
  await mkdir(cwd);
  const git = (...args) => execFileSync('git', args, { cwd, env, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  git('init', '--quiet', '--initial-branch=main');
  git('config', 'user.name', 'Release tests');
  git('config', 'user.email', 'release@example.invalid');
  await writeFile(join(cwd, 'Cargo.toml'), manifest);
  await writeFile(join(cwd, 'Cargo.lock'), lockfile);
  git('add', 'Cargo.toml', 'Cargo.lock');
  git('-c', 'commit.gpgSign=false', 'commit', '--quiet', '-m', tag ? 'feat: initial source' : message);
  if (tag) {
    git('tag', tag);
    await appendFile(join(cwd, 'Cargo.toml'), '# subsequent change\n');
    git('-c', 'commit.gpgSign=false', 'commit', '--quiet', '-am', message);
  }
  git('init', '--quiet', '--bare', '--initial-branch=main', remote);
  git('remote', 'add', 'origin', pathToFileURL(remote).href);
  git('push', '--quiet', '--set-upstream', 'origin', 'main', '--tags');
  return {
    cwd, git, env, stdout: quiet(), stderr: quiet(),
    config: { ...config, noCi: true, repositoryUrl: pathToFileURL(remote).href },
    output: join(directory, 'outputs.txt'),
  };
}

test('the real release engine plans its first 1.0.0 without creating a tag or changing sources', async (t) => {
  const options = await fixture(t, 'feat: initial Rust linter');
  const head = options.git('rev-parse', 'HEAD');
  const plan = await createPlan(options);
  assert.deepEqual(plan, { version: '1.0.0', commit: head, releasing: true });
  assert.equal(await readFile(join(options.cwd, 'Cargo.toml'), 'utf8'), manifest);
  assert.equal(await readFile(join(options.cwd, 'Cargo.lock'), 'utf8'), lockfile);
  assert.equal(await readFile(join(options.cwd, 'release-prepared', 'Cargo.toml'), 'utf8'), manifest.replace('0.1.0', '1.0.0'));
  assert.equal(await readFile(join(options.cwd, 'release-prepared', 'Cargo.lock'), 'utf8'), lockfile.replace('0.1.0', '1.0.0'));
  assert.deepEqual(JSON.parse(await readFile(join(options.cwd, 'release-prepared', 'plan.json'), 'utf8')), plan);
  assert.equal(await readFile(options.output, 'utf8'), `version=1.0.0\nreleasing=true\ncommit=${head}\n`);
  assert.equal(options.git('tag', '--list'), '');
  assert.equal(options.git('ls-remote', '--tags', 'origin'), '');
});

test('a CI-only initial commit preserves its source version and prepares non-release build inputs', async (t) => {
  const options = await fixture(t, 'ci: configure semantic-release');
  const plan = await createPlan(options);
  assert.equal(plan.releasing, false);
  assert.equal(plan.version, '0.1.0');
  assert.equal(await readFile(join(options.cwd, 'release-prepared', 'Cargo.toml'), 'utf8'), manifest);
  assert.equal(await readFile(join(options.cwd, 'release-prepared', 'Cargo.lock'), 'utf8'), lockfile);
  assert.equal(options.git('tag', '--list'), '');
});

test('subsequent planning uses release history and configured Conventional Commit analysis', async (t) => {
  const options = await fixture(t, 'fix: preserve source ranges', '1.2.3');
  const plan = await createPlan(options);
  assert.equal(plan.version, '1.2.4');
  assert.equal(plan.releasing, true);
  assert.equal(options.git('tag', '--list'), '1.2.3');
});

test('planning refuses an engine result for a different checkout', async (t) => {
  const options = await fixture(t, 'feat: initial source');
  await assert.rejects(createPlan({
    ...options,
    releaseEngine: async () => ({ nextRelease: { version: '1.0.0', gitHead: 'a'.repeat(40) } }),
  }), /differs from the producing checkout/);
});
