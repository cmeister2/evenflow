import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { appendFile, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { devNull, tmpdir } from 'node:os';
import { join } from 'node:path';
import { Writable } from 'node:stream';
import test from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';
import config from '../../release.config.mjs';
import { createPlan } from './plan.mjs';

const env = {
  PATH: process.env.PATH, HOME: process.env.HOME, CI: 'false',
  GIT_CONFIG_GLOBAL: devNull, GIT_CONFIG_NOSYSTEM: '1', GIT_ALLOW_PROTOCOL: 'file', GIT_TERMINAL_PROMPT: '0',
};
const manifest = '[package]\nname = "evenflow"\nversion = "0.1.0"\nedition = "2024"\n';
const lockfile = 'version = 4\n\n[[package]]\nname = "evenflow"\nversion = "0.1.0"\n';
const quiet = () => new Writable({ write(_chunk, _encoding, done) { done(); } });

async function fixture(t, message, tag, withRemote = true) {
  const directory = await mkdtemp(join(tmpdir(), 'evenflow-release-plan-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const cwd = join(directory, 'checkout');
  const remote = join(directory, 'remote.git');
  const fixtureEnv = { ...env };
  await mkdir(cwd);
  const git = (...args) => execFileSync('git', args, { cwd, env: fixtureEnv, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
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
  if (withRemote) {
    git('init', '--quiet', '--bare', '--initial-branch=main', remote);
    git('remote', 'add', 'origin', pathToFileURL(remote).href);
    git('push', '--quiet', '--set-upstream', 'origin', 'main', '--tags');
  }
  return {
    cwd, git, env: fixtureEnv, stdout: quiet(), stderr: quiet(),
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

test('preview planning stamps release build inputs and notes without enabling publication', async (t) => {
  const options = await fixture(t, 'feat: prepare Rust builds', undefined, false);
  const head = options.git('rev-parse', 'HEAD');
  const notes = '# 1.0.0\n\n* feat: prepare Rust builds';
  const summary = join(options.cwd, 'summary.md');
  const previewEnv = {
    ...env, CI: 'true', GITHUB_ACTIONS: 'true', GITHUB_EVENT_NAME: 'pull_request',
    GITHUB_REF: 'refs/pull/2/merge', GITHUB_HEAD_REF: 'feature', GITHUB_BASE_REF: 'main',
    GITHUB_OUTPUT: options.output, GITHUB_STEP_SUMMARY: summary,
  };
  let calls = 0;
  const plan = await createPlan({
    ...options, preview: true, env: previewEnv,
    previewEngine: async ({ cwd, env: suppliedEnv, stdout, stderr, config: suppliedConfig }) => {
      calls += 1;
      assert.equal(cwd, options.cwd);
      assert.equal(suppliedEnv, previewEnv);
      assert.equal(suppliedEnv.GITHUB_TOKEN, undefined);
      assert.equal(suppliedEnv.GH_TOKEN, undefined);
      assert.equal(suppliedEnv.GIT_ASKPASS, undefined);
      assert.equal(stdout, options.stdout);
      assert.equal(stderr, options.stderr);
      assert.equal(suppliedConfig, options.config);
      return { nextRelease: { version: '1.0.0', gitHead: head, notes } };
    },
    releaseEngine: async () => { throw new Error('Preview planning must not call the publication planner.'); },
  });
  assert.equal(calls, 1);
  assert.deepEqual(plan, { version: '1.0.0', commit: head, releasing: false });
  assert.equal(await readFile(join(options.cwd, 'Cargo.toml'), 'utf8'), manifest);
  assert.equal(await readFile(join(options.cwd, 'Cargo.lock'), 'utf8'), lockfile);
  assert.equal(await readFile(join(options.cwd, 'release-prepared', 'Cargo.toml'), 'utf8'), manifest.replace('0.1.0', '1.0.0'));
  assert.equal(await readFile(join(options.cwd, 'release-prepared', 'Cargo.lock'), 'utf8'), lockfile.replace('0.1.0', '1.0.0'));
  assert.deepEqual(JSON.parse(await readFile(join(options.cwd, 'release-prepared', 'plan.json'), 'utf8')), plan);
  assert.equal((await readFile(join(options.cwd, 'release-prepared', 'RELEASE_NOTES.md'), 'utf8')).trim(), notes);
  assert.match(await readFile(summary, 'utf8'), /1\.0\.0/);
  assert.ok((await readFile(summary, 'utf8')).includes(notes));
  assert.equal(await readFile(options.output, 'utf8'), `version=1.0.0\nreleasing=false\ncommit=${head}\n`);
  assert.equal(options.git('remote'), '');
  assert.equal(options.git('tag', '--list'), '');
});

test('a preview without a release preserves the development version and explains the result', async (t) => {
  const options = await fixture(t, 'ci: prepare Rust builds', undefined, false);
  const head = options.git('rev-parse', 'HEAD');
  const summary = join(options.cwd, 'summary.md');
  const plan = await createPlan({
    ...options, preview: true, env: { ...env, GITHUB_STEP_SUMMARY: summary },
    previewEngine: async () => false,
    releaseEngine: async () => { throw new Error('Preview planning must not call the publication planner.'); },
  });
  assert.deepEqual(plan, { version: '0.1.0', commit: head, releasing: false });
  assert.equal(await readFile(join(options.cwd, 'release-prepared', 'Cargo.toml'), 'utf8'), manifest);
  assert.equal(await readFile(join(options.cwd, 'release-prepared', 'Cargo.lock'), 'utf8'), lockfile);
  assert.match(await readFile(join(options.cwd, 'release-prepared', 'RELEASE_NOTES.md'), 'utf8'), /no release/i);
  assert.match(await readFile(summary, 'utf8'), /0\.1\.0/);
  assert.match(await readFile(summary, 'utf8'), /no release/i);
  assert.equal(await readFile(options.output, 'utf8'), `version=0.1.0\nreleasing=false\ncommit=${head}\n`);
  assert.equal(options.git('remote'), '');
  assert.equal(options.git('tag', '--list'), '');
});

test('preview planning refuses an engine result for a different checkout', async (t) => {
  const options = await fixture(t, 'feat: prepare Rust builds', undefined, false);
  await assert.rejects(createPlan({
    ...options, preview: true,
    previewEngine: async () => ({ nextRelease: { version: '1.0.0', gitHead: 'a'.repeat(40) } }),
  }), /differs from the producing checkout/);
  await assert.rejects(readFile(join(options.cwd, 'release-prepared', 'plan.json')), { code: 'ENOENT' });
});

test('preview planning refuses invalid release versions', async (t) => {
  const options = await fixture(t, 'feat: prepare Rust builds', undefined, false);
  for (const version of ['v1.0.0', '1.0.0-rc.1', undefined]) {
    await assert.rejects(createPlan({
      ...options, preview: true,
      previewEngine: async () => ({ nextRelease: { version, gitHead: options.git('rev-parse', 'HEAD') } }),
    }), /Expected a stable major.minor.patch release version/);
  }
  await assert.rejects(readFile(join(options.cwd, 'release-prepared', 'plan.json')), { code: 'ENOENT' });
});

test('preview planning still refuses mismatched Cargo package versions', async (t) => {
  const options = await fixture(t, 'feat: prepare Rust builds', undefined, false);
  await writeFile(join(options.cwd, 'Cargo.lock'), lockfile.replace('0.1.0', '0.2.0'));
  await assert.rejects(createPlan({
    ...options, preview: true, previewEngine: async () => false,
    releaseEngine: async () => { throw new Error('Preview planning must not call the publication planner.'); },
  }), /do not match/);
});

test('the real --preview CLI predicts a first release in a credential-free PR without a remote', async (t) => {
  const options = await fixture(t, 'feat: prepare Rust builds', undefined, false);
  const head = options.git('rev-parse', 'HEAD');
  const branches = options.git('for-each-ref', '--format=%(refname)');
  const summary = join(options.cwd, 'summary.md');
  const script = fileURLToPath(new URL('./plan.mjs', import.meta.url));
  execFileSync(process.execPath, [script, '--preview'], {
    cwd: options.cwd,
    env: {
      ...env, CI: 'true', GITHUB_ACTIONS: 'true', GITHUB_EVENT_NAME: 'pull_request',
      GITHUB_REF: 'refs/pull/2/merge', GITHUB_HEAD_REF: 'feature', GITHUB_BASE_REF: 'main',
      GITHUB_OUTPUT: options.output, GITHUB_STEP_SUMMARY: summary,
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const plan = JSON.parse(await readFile(join(options.cwd, 'release-prepared', 'plan.json'), 'utf8'));
  assert.deepEqual(plan, { version: '1.0.0', commit: head, releasing: false });
  assert.equal(await readFile(join(options.cwd, 'release-prepared', 'Cargo.toml'), 'utf8'), manifest.replace('0.1.0', '1.0.0'));
  assert.equal(await readFile(join(options.cwd, 'release-prepared', 'Cargo.lock'), 'utf8'), lockfile.replace('0.1.0', '1.0.0'));
  assert.match(await readFile(join(options.cwd, 'release-prepared', 'RELEASE_NOTES.md'), 'utf8'), /prepare Rust builds/);
  assert.match(await readFile(summary, 'utf8'), /1\.0\.0/);
  assert.equal(await readFile(join(options.cwd, 'Cargo.toml'), 'utf8'), manifest);
  assert.equal(await readFile(join(options.cwd, 'Cargo.lock'), 'utf8'), lockfile);
  assert.equal(options.git('diff', '--exit-code', 'HEAD'), '');
  assert.equal(options.git('rev-parse', 'HEAD'), head);
  assert.equal(options.git('for-each-ref', '--format=%(refname)'), branches);
  assert.equal(options.git('tag', '--list'), '');
  assert.equal(options.git('remote'), '');
});

test('the planner CLI rejects unknown or repeated flags before preparing files', async (t) => {
  const options = await fixture(t, 'feat: prepare Rust builds', undefined, false);
  const script = fileURLToPath(new URL('./plan.mjs', import.meta.url));
  for (const args of [['--unknown'], ['--ci'], ['--preview', '--preview'], ['--preview', '--unknown']]) {
    assert.throws(() => execFileSync(process.execPath, [script, ...args], {
      cwd: options.cwd, env, stdio: ['ignore', 'pipe', 'pipe'],
    }), (error) => error.status === 1 && /Usage: node/.test(error.stderr.toString()));
  }
  await assert.rejects(readFile(join(options.cwd, 'release-prepared', 'plan.json')), { code: 'ENOENT' });
});
