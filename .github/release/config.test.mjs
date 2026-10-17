import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { Writable } from 'node:stream';
import test from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';
import config from '../../release.config.mjs';

const cwd = fileURLToPath(new URL('../..', import.meta.url));
const require = createRequire(import.meta.url);
const { loadPlugin } = await import(pathToFileURL(join(dirname(require.resolve('semantic-release')), 'lib/plugins/utils.js')).href);
const quiet = () => new Writable({ write(_chunk, _encoding, done) { done(); } });

async function plugin(name) {
  const entry = config.plugins.find((entry) => Array.isArray(entry) && entry[0] === require.resolve(name));
  assert.ok(entry, `Missing explicitly resolved release plugin: ${name}`);
  return [await loadPlugin({ cwd }, entry[0], {}), entry[1]];
}

for (const [message, expected] of [
  ['fix: preserve source ranges', 'patch'],
  ['feat: add an opinionated lint', 'minor'],
  ['feat!: change the rule configuration', 'major'],
  ['docs: explain flow comments', null],
  ['ci: configure semantic-release', null],
]) {
  test(`configured analyzer classifies ${message}`, async () => {
    const [analyzer, options] = await plugin('@semantic-release/commit-analyzer');
    const result = await analyzer.analyzeCommits(options, {
      cwd, commits: [{ hash: 'a'.repeat(40), message }], logger: { log() {} },
    });
    assert.equal(result, expected);
  });
}

test('configured notes generator uses the Conventional Commits preset and version links', async () => {
  const [generator, options] = await plugin('@semantic-release/release-notes-generator');
  const notes = await generator.generateNotes(options, {
    cwd, options: { ...config, repositoryUrl: 'https://github.com/cmeister2/evenflow.git' },
    commits: [{ hash: 'a'.repeat(40), message: 'fix: preserve source ranges' }],
    lastRelease: { gitTag: '1.0.0', gitHead: 'b'.repeat(40) },
    nextRelease: { version: '1.0.1', gitTag: '1.0.1', gitHead: 'a'.repeat(40) },
  });
  assert.match(notes, /preserve source ranges/);
  assert.ok(notes.includes('https://github.com/cmeister2/evenflow/compare/1.0.0...1.0.1'));
});

test('the configured publication guard loads through semantic-release before tagging', async () => {
  const guard = await loadPlugin({ cwd }, './.github/release/plugin.mjs', {});
  assert.equal(typeof guard.verifyRelease, 'function');
  await assert.rejects(guard.verifyRelease({}, {
    cwd, nextRelease: { version: '9999.0.0', gitHead: 'c'.repeat(40) }, stdout: quiet(), stderr: quiet(),
  }));
});
