import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { access, appendFile, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { devNull, tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { Writable } from 'node:stream';
import test from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';
import config from '../../release.config.mjs';
import { previewRelease } from './preview.mjs';

const fixtureEnv = {
  PATH: process.env.PATH, HOME: process.env.HOME,
  GIT_CONFIG_GLOBAL: devNull, GIT_CONFIG_NOSYSTEM: '1', GIT_ALLOW_PROTOCOL: 'file',
};
const git = (cwd, ...args) => execFileSync('git', [
  '-c', 'user.name=Release Preview Test', '-c', 'user.email=preview@example.invalid',
  '-c', 'commit.gpgSign=false', '-c', `core.hooksPath=${devNull}`, ...args,
], { cwd, env: fixtureEnv, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();

/** Create a real source repository without requiring a network remote. */
async function fixture(t, { tag, remote = false } = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'evenflow-release-preview-test-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const cwd = join(directory, 'source');
  await mkdir(cwd);
  git(cwd, 'init', '--quiet', '--initial-branch=main');
  await writeFile(join(cwd, 'Cargo.toml'), '[package]\nname = "evenflow"\nversion = "0.1.0"\n');
  // A checked-out temporary config must never be loaded in the isolated preview.
  await writeFile(join(cwd, 'release.config.mjs'), 'throw new Error("Do not load temporary source configuration");\n');
  git(cwd, 'add', 'Cargo.toml', 'release.config.mjs');
  await commit(cwd, 'feat: initial Rust linter');
  if (tag) {
    git(cwd, 'tag', tag);
    git(cwd, 'notes', `--ref=semantic-release-${tag}`, 'add', '-m', '{"channels":[null]}', tag);
  }
  let origin;
  if (remote) {
    origin = join(directory, 'origin.git');
    git(directory, 'init', '--quiet', '--bare', '--initial-branch=main', origin);
    git(cwd, 'remote', 'add', 'origin', pathToFileURL(origin).href);
    git(cwd, 'push', '--quiet', 'origin', 'main', '--tags');
  }
  await writeFile(join(cwd, 'scratch.txt'), 'Untracked local file\n');
  return { cwd, origin };
}

/** Add a real Conventional Commit to the fixture history. */
async function commit(cwd, message) {
  await appendFile(join(cwd, 'fixture.txt'), `${message}\n`);
  git(cwd, 'add', 'fixture.txt');
  git(cwd, 'commit', '--quiet', '-m', message);
}

/** Capture the source checkout and its original remote before or after a preview. */
async function snapshot({ cwd, origin }) {
  return {
    head: git(cwd, 'rev-parse', 'HEAD'),
    branch: git(cwd, 'rev-parse', '--abbrev-ref', 'HEAD'),
    refs: git(cwd, 'for-each-ref', '--format=%(refname) %(objectname)'),
    status: git(cwd, 'status', '--porcelain=v1', '--untracked-files=all'),
    gitConfig: git(cwd, 'config', '--local', '--list'),
    source: await readFile(join(cwd, 'fixture.txt'), 'utf8'),
    manifest: await readFile(join(cwd, 'Cargo.toml'), 'utf8'),
    scratch: await readFile(join(cwd, 'scratch.txt'), 'utf8'),
    originRefs: origin ? git(origin, 'for-each-ref', '--format=%(refname) %(objectname)') : undefined,
  };
}

/** Capture real engine output and verify disposal of the isolated local remote. */
async function preview(cwd, extraEnv = {}, extraConfig = config) {
  let logs = '';
  const output = new Writable({ write(chunk, _encoding, done) { logs += chunk.toString(); done(); } });
  try {
    const result = await previewRelease({
      cwd, env: { ...fixtureEnv, ...extraEnv }, stdout: output, stderr: output, config: extraConfig,
    });
    const remote = logs.match(/file:\/\/[^\s]+\/remote\.git/)?.[0];
    assert.ok(remote, 'The preview did not report its isolated local remote.');
    await assert.rejects(access(dirname(fileURLToPath(remote))), { code: 'ENOENT' });
    return { result, logs };
  } finally {
    output.end();
  }
}

test('a detached fork PR genuinely previews its first 1.0.0 without credentials or source changes', async (t) => {
  const source = await fixture(t);
  git(source.cwd, 'checkout', '--quiet', '--detach');
  await appendFile(join(source.cwd, 'fixture.txt'), 'Uncommitted local edit\n');
  const before = await snapshot(source);
  const { result, logs } = await preview(source.cwd, {
    CI: 'true', GITHUB_ACTIONS: 'true', GITHUB_EVENT_NAME: 'pull_request',
    GITHUB_REF: 'refs/pull/42/merge', GITHUB_HEAD_REF: 'contributor-feature',
    GITHUB_BASE_REF: 'main', GITHUB_REPOSITORY: 'contributor/evenflow-fork',
    GITHUB_SERVER_URL: 'https://github.com', GITHUB_TOKEN: 'invalid-read-only-test-token',
    GH_TOKEN: 'invalid-test-token', GIT_CREDENTIALS: 'invalid-test-credentials',
  });
  assert.equal(result.nextRelease.version, '1.0.0');
  assert.equal(result.nextRelease.gitTag, '1.0.0');
  assert.equal(result.nextRelease.gitHead, before.head);
  assert.match(result.nextRelease.notes, /initial Rust linter/);
  assert.ok(result.nextRelease.notes.includes('https://github.com/cmeister2/evenflow/commit/'));
  assert.match(logs, /Preview complete: 1\.0\.0\. No release was published\./);
  assert.deepEqual(await snapshot(source), before);
  assert.equal(git(source.cwd, 'remote'), '');
  assert.equal(git(source.cwd, 'tag', '--list'), '');
});

test('an existing release tag produces a real patch preview with original GitHub links', async (t) => {
  const source = await fixture(t, { tag: '1.2.3', remote: true });
  git(source.cwd, 'checkout', '--quiet', '-b', 'fix/preview');
  await commit(source.cwd, 'fix: preserve source ranges (#27)');
  const before = await snapshot(source);
  const { result, logs } = await preview(source.cwd, {
    CI: 'true', GITHUB_ACTIONS: 'true', GITHUB_EVENT_NAME: 'workflow_dispatch',
    GITHUB_REF: 'refs/heads/fix/preview', GITHUB_TOKEN: 'invalid-read-only-test-token',
  });
  assert.equal(result.lastRelease.version, '1.2.3');
  assert.equal(result.nextRelease.version, '1.2.4');
  assert.equal(result.nextRelease.gitHead, before.head);
  assert.ok(result.nextRelease.notes.includes('https://github.com/cmeister2/evenflow/compare/1.2.3...1.2.4'));
  assert.ok(result.nextRelease.notes.includes('https://github.com/cmeister2/evenflow/issues/27'));
  assert.match(logs, /preserve source ranges/);
  assert.deepEqual(await snapshot(source), before);
  assert.equal(git(source.cwd, 'tag', '--list'), '1.2.3');
});

test('a documentation-only branch genuinely analyzes history and returns no release', async (t) => {
  const source = await fixture(t, { tag: '1.2.3', remote: true });
  git(source.cwd, 'checkout', '--quiet', '-b', 'docs/preview');
  await commit(source.cwd, 'docs: explain source installation');
  const before = await snapshot(source);
  const { result, logs } = await preview(source.cwd, {
    CI: 'true', GITHUB_ACTIONS: 'true', GITHUB_EVENT_NAME: 'pull_request',
    GITHUB_REF: 'refs/pull/42/merge',
  });
  assert.equal(result, false);
  assert.match(logs, /Preview complete: no release-worthy changes\./);
  assert.deepEqual(await snapshot(source), before);
});

test('preview reuses configured analysis options and repository URL', async (t) => {
  const source = await fixture(t, { tag: '1.2.3' });
  await commit(source.cwd, 'docs: explain custom release policy');
  const changedConfig = {
    ...config, repositoryUrl: 'git+https://github.com/cmeister2/evenflow-custom.git',
    plugins: config.plugins.map((entry, index) => index === 0
      ? [entry[0], { ...entry[1], releaseRules: [{ type: 'docs', release: 'patch' }] }] : entry),
  };
  const { result } = await preview(source.cwd, {}, changedConfig);
  assert.equal(result.nextRelease.version, '1.2.4');
  assert.ok(result.nextRelease.notes.includes('https://github.com/cmeister2/evenflow-custom/compare/1.2.3...1.2.4'));
});

test('preview rejects incomplete history instead of guessing a release version', async (t) => {
  const source = await fixture(t, { tag: '1.2.3' });
  await commit(source.cwd, 'fix: require complete release history');
  const shallow = join(dirname(source.cwd), 'shallow');
  git(dirname(source.cwd), 'clone', '--quiet', '--depth', '1', pathToFileURL(source.cwd).href, shallow);
  await assert.rejects(previewRelease({ cwd: shallow, env: fixtureEnv }), /requires full Git history/);
});

test('preview rejects missing configured analysis plugins', async (t) => {
  const source = await fixture(t);
  await assert.rejects(previewRelease({
    cwd: source.cwd, env: fixtureEnv, config: { ...config, plugins: [] },
  }), /requires the configured @semantic-release\/commit-analyzer plugin/);
});
