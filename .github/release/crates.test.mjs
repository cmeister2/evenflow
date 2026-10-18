import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { access, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import test from 'node:test';
import { prepareCrate, publishCrate } from './crates.mjs';

const version = '1.2.3';
const token = 'private-test-token-that-must-stay-in-the-environment';
const candidate = Buffer.from('canonical source archive produced by Cargo');
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');
const env = { ...process.env, CARGO_REGISTRY_TOKEN: token };

/** Build actual tar archives, including unsafe members for extraction checks. */
const tarScript = String.raw`
import io, json, sys, tarfile
with tarfile.open(sys.argv[1], 'w:gz') as archive:
    for entry in json.loads(sys.argv[2]):
        info = tarfile.TarInfo(entry['name'])
        if entry.get('type') == 'symlink':
            info.type = tarfile.SYMTYPE
            info.linkname = '../../outside'
            archive.addfile(info)
        elif entry.get('type') == 'directory':
            info.type = tarfile.DIRTYPE
            archive.addfile(info)
        else:
            data = entry.get('content', '').encode()
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))
`;

/** Keep tested source in the archive and unrelated source in the checkout. */
async function fixture(t, { extra = [], archiveVersion = version, omit = [] } = {}) {
  const cwd = await mkdtemp(join(tmpdir(), 'evenflow-crates-test-'));
  t.after(() => rm(cwd, { recursive: true, force: true }));
  await mkdir(join(cwd, 'dist'));
  await writeFile(join(cwd, 'Cargo.toml'), 'unverified checkout manifest');
  const prefix = `evenflow-${archiveVersion}`;
  const manifest = `[package]\nname = "evenflow"\nversion = "${version}"\n# original tested manifest\n`;
  const entries = [
    { name: `${prefix}/Cargo.toml`, content: `${manifest}# normalized packaging manifest\n` },
    { name: `${prefix}/Cargo.toml.orig`, content: manifest },
    { name: `${prefix}/Cargo.lock`, content: 'version = 4\n' },
    { name: `${prefix}/.cargo_vcs_info.json`, content: '{"git":{"sha1":"aabbcc"}}' },
    { name: `${prefix}/src/main.rs`, content: 'fn main() { println!("tested source"); }\n' },
  ].filter((entry) => !omit.includes(entry.name.split('/').at(-1))).concat(extra);
  const archivePath = join(cwd, 'dist', `evenflow-${version}.crate`);
  execFileSync('python3', ['-c', tarScript, archivePath, JSON.stringify(entries)], {
    encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
  });
  return { cwd, manifest, archivePath, original: await readFile(archivePath) };
}

/** Perform real extraction and model Cargo metadata, packaging, and upload. */
function cargoRunner({ metadata, bytes = candidate, uploadError, changeUpload = false } = {}) {
  const calls = [];
  const run = async (file, args, options) => {
    calls.push({ file, args, options: { ...options, env: { ...options.env } } });
    assert.deepEqual(options.stdio, ['ignore', 'pipe', 'pipe']);
    if (file === 'python3') return execFileSync(file, args, options);
    assert.equal(file, 'cargo');
    const manifestPath = args[args.indexOf('--manifest-path') + 1];
    assert.ok((await readFile(manifestPath, 'utf8')).includes('# original tested manifest'));
    assert.ok((await readFile(join(dirname(manifestPath), 'src/main.rs'), 'utf8')).includes('tested source'));
    await assert.rejects(access(join(dirname(manifestPath), 'Cargo.toml.orig')), { code: 'ENOENT' });
    await assert.rejects(access(join(dirname(manifestPath), '.cargo_vcs_info.json')), { code: 'ENOENT' });
    if (args[0] === 'metadata') {
      return JSON.stringify(metadata ?? { packages: [{ name: 'evenflow', version, manifest_path: manifestPath }] });
    }
    assert.equal(args[0], 'publish');
    const target = args[args.indexOf('--target-dir') + 1];
    const archive = join(target, 'package', `evenflow-${version}.crate`);
    if (args.includes('--dry-run')) {
      assert.equal(options.env.CARGO_REGISTRY_TOKEN, undefined);
      assert.equal(options.env.CARGO_REGISTRIES_CRATES_IO_TOKEN, undefined);
      await mkdir(dirname(archive), { recursive: true });
      await writeFile(archive, bytes);
    } else {
      assert.equal(options.env.CARGO_REGISTRY_TOKEN, token);
      if (changeUpload) await writeFile(archive, 'changed during upload');
      if (uploadError) throw new Error(`${uploadError}: ${options.env.CARGO_REGISTRY_TOKEN}`);
    }
    return '';
  };
  return {
    run, calls,
    uploads: () => calls.filter((call) => call.file === 'cargo' && call.args[0] === 'publish' && !call.args.includes('--dry-run')),
  };
}

/** Serve only the official public index endpoint, without accessing the network. */
function registry(...replies) {
  let calls = 0;
  const fetch = async (url, options) => {
    assert.equal(url, 'https://index.crates.io/ev/en/evenflow');
    assert.equal(options.headers.Authorization, undefined);
    assert.ok(!JSON.stringify(options.headers).includes(token));
    const reply = replies[Math.min(calls++, replies.length - 1)];
    if (reply instanceof Error) throw reply;
    return new Response(reply === 404 ? '' : typeof reply === 'string' ? reply : JSON.stringify(reply), {
      status: reply === 404 ? 404 : 200,
    });
  };
  return { fetch, calls: () => calls };
}

/** Verify disposal of staging after both successful and failed publication. */
async function cleaned(runner) {
  const metadata = runner.calls.find((call) => call.file === 'cargo' && call.args[0] === 'metadata');
  if (metadata) await assert.rejects(access(metadata.options.cwd), { code: 'ENOENT' });
}

test('preparation validates tested source and runs the exact credential-free Cargo dry run', async (t) => {
  const source = await fixture(t);
  const runner = cargoRunner();
  const staged = await prepareCrate({ ...source, version, env, run: runner.run });
  assert.equal(staged.checksum, digest(candidate));
  assert.equal(await readFile(staged.manifestPath, 'utf8'), source.manifest);
  assert.deepEqual(staged.publishArgs, [
    'publish', '--registry', 'crates-io', '--locked', '--manifest-path', staged.manifestPath,
    '--target-dir', join(dirname(staged.directory), 'target'),
  ]);
  assert.equal(runner.uploads().length, 0);
  for (const call of runner.calls) assert.equal(call.options.env.CARGO_REGISTRY_TOKEN, undefined);
  assert.deepEqual(await readFile(source.archivePath), source.original);
  assert.equal(await readFile(join(source.cwd, 'Cargo.toml'), 'utf8'), 'unverified checkout manifest');
  await staged.cleanup();
  await cleaned(runner);
});

test('new publication keeps tokens in the environment and copies the registry payload back', async (t) => {
  const source = await fixture(t);
  const runner = cargoRunner();
  const index = registry(404);
  const receipt = await publishCrate({ ...source, version, env, run: runner.run, fetch: index.fetch });
  assert.deepEqual(receipt, { alreadyPublished: false, checksum: digest(candidate) });
  assert.equal(runner.uploads().length, 1);
  assert.equal(index.calls(), 1);
  assert.deepEqual(await readFile(source.archivePath), candidate);
  for (const call of runner.calls) {
    assert.ok(!call.args.some((argument) => argument.includes(token)));
    assert.ok(!call.args.includes('--token'));
  }
  await cleaned(runner);
});

test('a matching existing version succeeds without upload credentials', async (t) => {
  const source = await fixture(t);
  const runner = cargoRunner();
  const receipt = await publishCrate({
    ...source, version, env: { ...env, CARGO_REGISTRY_TOKEN: undefined }, run: runner.run,
    fetch: registry({ name: 'evenflow', vers: version, cksum: digest(candidate) }).fetch,
  });
  assert.deepEqual(receipt, { alreadyPublished: true, checksum: digest(candidate) });
  assert.equal(runner.uploads().length, 0);
  assert.deepEqual(await readFile(source.archivePath), candidate);
  await cleaned(runner);
});

test('a conflicting existing checksum blocks publication and asset replacement', async (t) => {
  const source = await fixture(t);
  const runner = cargoRunner();
  await assert.rejects(publishCrate({
    ...source, version, env, run: runner.run,
    fetch: registry({ name: 'evenflow', vers: version, cksum: 'a'.repeat(64) }).fetch,
  }), /different checksum/);
  assert.equal(runner.uploads().length, 0);
  assert.deepEqual(await readFile(source.archivePath), source.original);
  await cleaned(runner);
});

test('missing credentials fail before uploading and preserve the tested archive', async (t) => {
  const source = await fixture(t);
  const runner = cargoRunner();
  await assert.rejects(publishCrate({
    ...source, version, env: { ...env, CARGO_REGISTRY_TOKEN: '' }, run: runner.run, fetch: registry(404).fetch,
  }), /Set the CARGO_REGISTRY_TOKEN GitHub Actions secret/);
  assert.equal(runner.uploads().length, 0);
  assert.deepEqual(await readFile(source.archivePath), source.original);
  await cleaned(runner);
});

test('upload timeout recovery requires a matching published checksum', async (t) => {
  const source = await fixture(t);
  const runner = cargoRunner({ uploadError: 'index timeout' });
  const index = registry(404, { name: 'evenflow', vers: version, cksum: digest(candidate) });
  const receipt = await publishCrate({ ...source, version, env, run: runner.run, fetch: index.fetch });
  assert.equal(receipt.checksum, digest(candidate));
  assert.equal(runner.uploads().length, 1);
  assert.equal(index.calls(), 2);
  assert.deepEqual(await readFile(source.archivePath), candidate);
  await cleaned(runner);
});

test('unconfirmed upload errors redact credentials and leave the tested asset intact', async (t) => {
  const source = await fixture(t);
  const runner = cargoRunner({ uploadError: 'registry failed' });
  await assert.rejects(publishCrate({
    ...source, version, env, run: runner.run, fetch: registry(404, 404).fetch,
  }), (error) => error.message.includes('registry failed') && error.message.includes('[redacted]') && !error.message.includes(token));
  assert.deepEqual(await readFile(source.archivePath), source.original);
  await cleaned(runner);
});

test('index network errors, malformed records and invalid checksums fail closed', async (t) => {
  for (const response of [new Error('network unavailable'), '{bad json', { name: 'evenflow', vers: version, cksum: 'bad' }]) {
    const source = await fixture(t);
    const runner = cargoRunner();
    await assert.rejects(publishCrate({ ...source, version, env, run: runner.run, fetch: registry(response).fetch }));
    assert.equal(runner.uploads().length, 0);
    assert.deepEqual(await readFile(source.archivePath), source.original);
    await cleaned(runner);
  }
});

test('unsafe members, wrong top directories and missing package files are rejected before Cargo', async (t) => {
  for (const options of [
    { archiveVersion: '9.9.9' }, { omit: ['Cargo.lock'] },
    { extra: [{ name: `evenflow-${version}/../../outside`, content: 'escape' }] },
    { extra: [{ name: `evenflow-${version}/src\\outside`, content: 'escape' }] },
    { extra: [{ name: `evenflow-${version}/src/link.rs`, type: 'symlink' }] },
    { extra: [{ name: `evenflow-${version}/src/directory`, type: 'directory' }] },
    { extra: [{ name: `evenflow-${version}/Cargo.toml`, content: 'duplicate' }] },
  ]) {
    const source = await fixture(t, options);
    const runner = cargoRunner();
    await assert.rejects(prepareCrate({ ...source, version, env, run: runner.run }), /Unsafe|must contain/);
    assert.equal(runner.calls.filter((call) => call.file === 'cargo').length, 0);
    assert.deepEqual(await readFile(source.archivePath), source.original);
  }
});

test('metadata must identify exactly the planned crate and manifest', async (t) => {
  for (const packages of [
    [], [{ name: 'another', version, manifest_path: '/wrong/Cargo.toml' }],
    [{ name: 'evenflow', version: '9.9.9', manifest_path: '/wrong/Cargo.toml' }],
    [{ name: 'evenflow', version, manifest_path: '/unverified/Cargo.toml' }],
  ]) {
    const source = await fixture(t);
    const runner = cargoRunner({ metadata: { packages } });
    await assert.rejects(prepareCrate({ ...source, version, env, run: runner.run }), /exactly the planned evenflow/);
    assert.equal(runner.calls.filter((call) => call.args[0] === 'publish').length, 0);
    await cleaned(runner);
  }
});

test('empty candidates and changed upload payloads cannot be reported as published', async (t) => {
  const source = await fixture(t);
  const empty = cargoRunner({ bytes: Buffer.alloc(0) });
  await assert.rejects(prepareCrate({ ...source, version, env, run: empty.run }), /nonempty regular file/);
  await cleaned(empty);
  const changed = cargoRunner({ changeUpload: true });
  await assert.rejects(publishCrate({
    ...source, version, env, run: changed.run, fetch: registry(404).fetch,
  }), /changed the validated archive/);
  assert.deepEqual(await readFile(source.archivePath), source.original);
  await cleaned(changed);
});
