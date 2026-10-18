import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { mkdir, mkdtemp, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
import test from 'node:test';

import { readInputs, releaseArchive, runAction, verifyChecksum } from './run.mjs';

const api = 'https://api.github.com/repos/cmeister2/evenflow';
const linuxRoot = 'evenflow-1.2.3-x86_64-unknown-linux-gnu';
const windowsRoot = 'evenflow-1.2.3-x86_64-pc-windows-msvc';
const archiveBytes = Buffer.from('Verified release archive fixture');
const digest = createHash('sha256').update(archiveBytes).digest('hex');

/** Use real install directories while controlling only the network and processes. */
async function fixture(t, options = {}) {
  const root = await mkdtemp(join(tmpdir(), 'evenflow-action-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const workspace = join(root, 'checkout');
  const runnerTemp = join(root, "runner temp ; '$fixture");
  await Promise.all([mkdir(workspace), mkdir(runnerTemp)]);
  const windows = options.platform === 'win32';
  const archiveRoot = windows ? windowsRoot : linuxRoot;
  const archiveName = `${archiveRoot}.${windows ? 'zip' : 'tar.gz'}`;
  const binaryName = windows ? 'evenflow.exe' : 'evenflow';
  const env = {
    GITHUB_WORKSPACE: workspace,
    RUNNER_TEMP: runnerTemp,
    INPUT_TOKEN: 'release-read-token',
    ...options.env,
  };
  const release = {
    tag_name: '1.2.3', draft: false, prerelease: false,
    assets: [{ name: archiveName, id: 31 }, { name: 'SHA256SUMS', id: 32 }],
    ...options.release,
  };
  const requests = [];
  const executions = [];
  let output = '';
  const request = async (url, init) => {
    requests.push({ url, init });
    if (options.requestError) throw options.requestError;
    if (options.httpStatus) return new Response('', { status: options.httpStatus });
    if (url === `${api}/releases/latest` || url === `${api}/releases/tags/1.2.3`) {
      return Response.json(release);
    }
    if (url === `${api}/releases/assets/31`) return new Response(archiveBytes);
    if (url === `${api}/releases/assets/32`) {
      return new Response(options.checksums ?? `${digest}  ${archiveName}\n`);
    }
    assert.fail(`Unexpected request: ${url}`);
  };
  const execute = (command, args, commandOptions) => {
    executions.push({ command, args, options: commandOptions });
    if (executions.length === 1) {
      const extractionDirectory = commandOptions.cwd;
      assert.deepEqual(readFileSync(join(extractionDirectory, archiveName)), archiveBytes);
      if (options.extractError) return { error: options.extractError, status: null };
      if (options.extractStatus) return { status: options.extractStatus };
      if (!options.noBinary) {
        const binary = join(extractionDirectory, archiveRoot, binaryName);
        mkdirSync(dirname(binary));
        writeFileSync(binary, 'installed executable fixture');
      }
      return { status: 0 };
    }
    if (options.spawnError) return { error: options.spawnError, status: null };
    return { status: Object.hasOwn(options, 'status') ? options.status : 0, signal: options.signal };
  };
  return {
    env, workspace, runnerTemp, archiveName, archiveRoot, requests, executions,
    output: () => output,
    run: () => runAction({
      env, platform: options.platform ?? 'linux', arch: 'x64', request, execute,
      output: { write: (text) => { output += text; } },
    }),
  };
}

test('input defaults scan the checkout strictly using the latest release', () => {
  assert.deepEqual(readInputs({ GITHUB_WORKSPACE: '/consumer/checkout' }), {
    version: 'latest', strict: true, paths: ['.'], cwd: resolve('/consumer/checkout'), token: undefined,
  });
  const inputs = readInputs({
    GITHUB_WORKSPACE: '/consumer/checkout', 'INPUT_WORKING-DIRECTORY': ' crates/service ',
    INPUT_VERSION: ' 1.2.3 ', INPUT_STRICT: ' false ', INPUT_TOKEN: ' token ',
    INPUT_PATHS: ' src with spaces/\r\n\n--help\n$(touch escaped).rs\nname;echo nope.rs\n',
  });
  assert.deepEqual(inputs, {
    version: '1.2.3', strict: false,
    paths: ['src with spaces/', '--help', '$(touch escaped).rs', 'name;echo nope.rs'],
    cwd: resolve('/consumer/checkout/crates/service'), token: 'token',
  });
  assert.deepEqual(readInputs({ INPUT_PATHS: ' \r\n ' }).paths, ['.']);
});

test('inputs reject ambiguous versions and boolean spellings', () => {
  for (const version of ['v1.2.3', '1.2', '1.2.3-beta.1', '01.2.3', '1.2.3/../latest']) {
    assert.throws(() => readInputs({ INPUT_VERSION: version }), /version must/);
  }
  for (const strict of ['yes', 'TRUE', '0']) {
    assert.throws(() => readInputs({ INPUT_STRICT: strict }), /strict must/);
  }
});

test('supported runners select the published archive layout', () => {
  for (const [platform, arch, target, extension, binary] of [
    ['linux', 'x64', 'x86_64-unknown-linux-gnu', 'tar.gz', 'evenflow'],
    ['darwin', 'arm64', 'aarch64-apple-darwin', 'tar.gz', 'evenflow'],
    ['win32', 'x64', 'x86_64-pc-windows-msvc', 'zip', 'evenflow.exe'],
  ]) {
    const root = `evenflow-1.2.3-${target}`;
    assert.deepEqual(releaseArchive('1.2.3', platform, arch), { root, name: `${root}.${extension}`, binary });
  }
  for (const [platform, arch] of [['linux', 'arm64'], ['darwin', 'x64'], ['win32', 'arm64'], ['freebsd', 'x64']]) {
    assert.throws(() => releaseArchive('1.2.3', platform, arch), /Unsupported runner/);
  }
});

test('checksum verification requires exactly one matching entry and unmodified bytes', () => {
  const name = `${linuxRoot}.tar.gz`;
  const entry = `${digest}  ${name}`;
  assert.doesNotThrow(() => verifyChecksum(archiveBytes, `${'0'.repeat(64)}  other.crate\r\n${entry}\r\n`, name));
  assert.throws(() => verifyChecksum(Buffer.from('tampered'), `${entry}\n`, name), /verification failed/);
  assert.throws(() => verifyChecksum(archiveBytes, `${digest}  unrelated.tar.gz\n`, name), /verification failed/);
  assert.throws(() => verifyChecksum(archiveBytes, `${entry}\n${entry}\n`, name), /verification failed/);
});

test('runs against the caller checkout with literal argv and preserves all CLI exit codes', async (t) => {
  for (const status of [0, 1, 2]) {
    await t.test(`CLI exit ${status}`, async (t) => {
      const f = await fixture(t, { status, env: {
        INPUT_VERSION: '1.2.3', 'INPUT_WORKING-DIRECTORY': 'service',
        INPUT_PATHS: 'src with spaces\n--help\n$(touch escaped).rs',
      } });
      await mkdir(join(f.workspace, 'service'));
      assert.equal(await f.run(), status);
      assert.equal(f.requests[0].url, `${api}/releases/tags/1.2.3`);
      for (const { url, init } of f.requests) {
        assert.ok(url.startsWith(`${api}/releases/`));
        assert.equal(init.headers.Authorization, 'Bearer release-read-token');
      }
      assert.equal(f.requests[1].init.headers.Accept, 'application/octet-stream');
      const [extraction, lint] = f.executions;
      assert.equal(extraction.command, 'tar');
      assert.deepEqual(extraction.args, ['-xzf', join(extraction.options.cwd, f.archiveName), '-C', extraction.options.cwd]);
      assert.equal(lint.command, join(extraction.options.cwd, f.archiveRoot, 'evenflow'));
      assert.deepEqual(lint.args, ['check', '--strict', '--', 'src with spaces', '--help', '$(touch escaped).rs']);
      assert.equal(lint.options.cwd, join(f.workspace, 'service'));
      assert.equal(lint.options.env, f.env);
      for (const call of f.executions) {
        assert.equal(call.options.shell, false);
        assert.equal(call.options.stdio, 'inherit');
      }
      const stopped = /::stop-commands::([^\n]+)\n/.exec(f.output());
      assert.ok(stopped);
      assert.ok(f.output().endsWith(`::${stopped[1]}::\n`));
      assert.deepEqual(await readdir(f.runnerTemp), []);
    });
  }
});

test('Windows extracts with literal environment paths and respects strict false', async (t) => {
  const f = await fixture(t, { platform: 'win32', env: { INPUT_STRICT: 'false' } });
  assert.equal(await f.run(), 0);
  assert.equal(f.requests[0].url, `${api}/releases/latest`);
  const [extraction, lint] = f.executions;
  assert.equal(extraction.command, 'powershell.exe');
  assert.deepEqual(extraction.args.slice(0, 4), ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command']);
  assert.equal(extraction.args.length, 5);
  assert.match(extraction.args[4], /-LiteralPath \$env:EVENFLOW_ARCHIVE -DestinationPath \$env:EVENFLOW_EXTRACT/);
  assert.ok(!extraction.args[4].includes(extraction.options.cwd));
  assert.equal(extraction.options.env.EVENFLOW_ARCHIVE, join(extraction.options.cwd, f.archiveName));
  assert.equal(extraction.options.env.EVENFLOW_EXTRACT, extraction.options.cwd);
  assert.equal(extraction.options.env.INPUT_TOKEN, f.env.INPUT_TOKEN);
  assert.equal(basename(lint.command), 'evenflow.exe');
  assert.deepEqual(lint.args, ['check', '--', '.']);
  assert.equal(lint.options.cwd, f.workspace);
  assert.equal(lint.options.shell, false);
  assert.deepEqual(await readdir(f.runnerTemp), []);
});

test('release and download failures never extract or execute code', async (t) => {
  for (const [name, options, expected] of [
    ['network failure', { requestError: new Error('network unavailable') }, /network unavailable/],
    ['HTTP failure', { httpStatus: 404 }, /HTTP 404/],
    ['missing asset', { release: { assets: [] } }, /must contain exactly one/],
    ['ambiguous assets', { release: { assets: [
      { name: `${linuxRoot}.tar.gz`, id: 31 },
      { name: 'SHA256SUMS', id: 32 }, { name: 'SHA256SUMS', id: 33 },
    ] } }, /must contain exactly one SHA256SUMS/],
    ['checksum mismatch', { checksums: `${'0'.repeat(64)}  ${linuxRoot}.tar.gz\n` }, /verification failed/],
    ['draft release', { release: { draft: true } }, /matching stable/],
    ['prerelease', { release: { prerelease: true } }, /matching stable/],
    ['tag mismatch', { env: { INPUT_VERSION: '1.2.3' }, release: { tag_name: '1.2.4' } }, /matching stable/],
  ]) {
    await t.test(name, async (t) => {
      const f = await fixture(t, options);
      await assert.rejects(f.run, expected);
      assert.deepEqual(f.executions, []);
      assert.deepEqual(await readdir(f.runnerTemp), []);
    });
  }
});

test('working-directory must exist and be a directory before any request', async (t) => {
  for (const file of [false, true]) {
    const f = await fixture(t, { env: { 'INPUT_WORKING-DIRECTORY': 'missing-or-file' } });
    if (file) await writeFile(join(f.workspace, 'missing-or-file'), 'not a directory');
    await assert.rejects(f.run, file ? /not a directory/ : /ENOENT/);
    assert.deepEqual(f.requests, []);
    assert.deepEqual(f.executions, []);
  }
});

test('failed extraction and execution always remove the temporary installation', async (t) => {
  for (const [name, options, expected, executionCount] of [
    ['extraction exit', { extractStatus: 7 }, /Cannot extract.*exit 7/, 1],
    ['extraction spawn error', { extractError: new Error('tar unavailable') }, /Cannot execute.*tar unavailable/, 1],
    ['missing executable', { noBinary: true }, /ENOENT/, 1],
    ['linter spawn error', { spawnError: new Error('permission denied') }, /Cannot execute.*permission denied/, 2],
    ['linter signal', { signal: 'SIGTERM', status: null }, /terminated before completing/, 2],
    ['unexpected linter exit', { status: 9 }, /exited unexpectedly.*exit 9/, 2],
  ]) {
    await t.test(name, async (t) => {
      const f = await fixture(t, options);
      await assert.rejects(f.run, expected);
      assert.equal(f.executions.length, executionCount);
      assert.deepEqual(await readdir(f.runnerTemp), []);
      if (executionCount === 2) {
        const stopped = /::stop-commands::([^\n]+)\n/.exec(f.output());
        assert.ok(stopped);
        assert.ok(f.output().endsWith(`::${stopped[1]}::\n`));
      }
    });
  }
});
