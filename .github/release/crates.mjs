import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFile, lstat, mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { validateVersion } from './version.mjs';

const indexUrl = 'https://index.crates.io/ev/en/evenflow';

/** Validate every archive member before extracting ordinary package files. */
const extractScript = String.raw`
import pathlib, shutil, sys, tarfile
archive, destination, expected = sys.argv[1:]
root = pathlib.Path(destination)
with tarfile.open(archive, 'r:gz') as source:
    members = source.getmembers()
    seen = set()
    for member in members:
        parts = member.name.split('/')
        if (not member.isfile() or '\\' in member.name or
            len(parts) < 2 or parts[0] != expected or
            any(part in ('', '.', '..', '.git') for part in parts) or
            member.name in seen):
            raise ValueError('Unsafe or unexpected crate archive member: ' + member.name)
        seen.add(member.name)
    if expected + '/Cargo.toml' not in seen or expected + '/Cargo.lock' not in seen:
        raise ValueError('Crate archive must contain Cargo.toml and Cargo.lock')
    for member in members:
        target = root.joinpath(*member.name.split('/'))
        target.parent.mkdir(parents=True, exist_ok=True)
        with source.extractfile(member) as source_file, target.open('xb') as output:
            shutil.copyfileobj(source_file, output)
package = root / expected
original = package / 'Cargo.toml.orig'
if original.exists():
    shutil.copyfile(original, package / 'Cargo.toml')
    original.unlink()
# Cargo regenerates packaging metadata; these reserved files cannot be repackaged.
(package / '.cargo_vcs_info.json').unlink(missing_ok=True)
`;

/** Avoid including Cargo credentials in child-process failures or logs. */
function failure(error, env) {
  let message = error.stderr?.toString() || error.message || String(error);
  for (const token of [env.CARGO_REGISTRY_TOKEN, env.CARGO_REGISTRIES_CRATES_IO_TOKEN]) {
    if (token) message = message.split(token).join('[redacted]');
  }
  return new Error(message.trim());
}

/** Keep credentials out of validation and dry-run subprocesses. */
function withoutToken(env) {
  const copy = { ...env };
  delete copy.CARGO_REGISTRY_TOKEN;
  delete copy.CARGO_REGISTRIES_CRATES_IO_TOKEN;
  return copy;
}

/** Run Cargo or Python with captured output and no interactive input. */
async function command(run, file, args, cwd, env) {
  try {
    const output = await run(file, args, { cwd, env, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
    return typeof output === 'string' || Buffer.isBuffer(output) ? output.toString() : output?.stdout ?? '';
  } catch (error) {
    throw failure(error, env);
  }
}

/** Compute the registry checksum of a nonempty ordinary archive file. */
async function checksum(path) {
  const stat = await lstat(path);
  if (!stat.isFile() || stat.size === 0) throw new Error('The Cargo release archive must be a nonempty regular file.');
  return createHash('sha256').update(await readFile(path)).digest('hex');
}

/** Stage and validate source exclusively from the tested Cargo source artifact. */
export async function prepareCrate({
  cwd = process.cwd(), version, env = process.env, run = execFileSync,
} = {}) {
  validateVersion(version);
  const archivePath = join(resolve(cwd), 'dist', `evenflow-${version}.crate`);
  await checksum(archivePath);
  const temporary = await mkdtemp(join(tmpdir(), 'evenflow-crates-publish-'));
  const directory = join(temporary, `evenflow-${version}`);
  const manifestPath = join(directory, 'Cargo.toml');
  const targetDirectory = join(temporary, 'target');
  const candidatePath = join(targetDirectory, 'package', `evenflow-${version}.crate`);
  const cleanEnv = withoutToken(env);
  const cleanup = () => rm(temporary, { recursive: true, force: true });
  try {
    await command(run, 'python3', ['-c', extractScript, archivePath, temporary, `evenflow-${version}`], cwd, cleanEnv);
    const metadata = JSON.parse(await command(run, 'cargo', [
      'metadata', '--no-deps', '--locked', '--format-version', '1', '--manifest-path', manifestPath,
    ], directory, cleanEnv));
    if (metadata.packages?.length !== 1 || metadata.packages[0].name !== 'evenflow'
        || metadata.packages[0].version !== version || resolve(metadata.packages[0].manifest_path) !== manifestPath) {
      throw new Error('The tested source archive must contain exactly the planned evenflow package and version.');
    }
    const publishArgs = [
      'publish', '--registry', 'crates-io', '--locked', '--manifest-path', manifestPath,
      '--target-dir', targetDirectory,
    ];
    await command(run, 'cargo', [...publishArgs, '--dry-run'], directory, cleanEnv);
    return {
      directory, manifestPath, archivePath, candidatePath, publishArgs, cleanup,
      checksum: await checksum(candidatePath),
    };
  } catch (error) {
    await cleanup();
    throw failure(error, env);
  }
}

/** Look up the immutable checksum of this version in the official sparse index. */
async function publishedChecksum(version, fetchIndex) {
  const response = await fetchIndex(indexUrl, {
    headers: { 'User-Agent': 'evenflow-release (https://github.com/cmeister2/evenflow)', 'Cache-Control': 'no-cache' },
    signal: AbortSignal.timeout(15000),
  });
  if (response.status === 404) return null;
  if (!response.ok) throw new Error(`Failed to read the crates.io index (HTTP ${response.status}).`);
  const versions = (await response.text()).split('\n').filter((line) => line.trim()).map((line) => JSON.parse(line));
  const matches = versions.filter((entry) => entry.vers === version);
  if (matches.length === 0) return null;
  if (matches.length !== 1 || matches[0].name !== 'evenflow' || !/^[0-9a-f]{64}$/.test(matches[0].cksum)) {
    throw new Error('The crates.io index returned an invalid package checksum.');
  }
  return matches[0].cksum;
}

/** Publish the tested source, accepting an existing version only if its bytes match. */
export async function publishCrate({
  cwd = process.cwd(), version, env = process.env, run = execFileSync, fetch: fetchIndex = globalThis.fetch,
} = {}) {
  const staged = await prepareCrate({ cwd, version, env, run });
  try {
    const existing = await publishedChecksum(version, fetchIndex);
    if (existing && existing !== staged.checksum) {
      throw new Error(`crates.io already contains evenflow ${version} with a different checksum.`);
    }
    if (!existing) {
      const token = env.CARGO_REGISTRY_TOKEN?.trim() || env.CARGO_REGISTRIES_CRATES_IO_TOKEN?.trim();
      if (!token) throw new Error('Set the CARGO_REGISTRY_TOKEN GitHub Actions secret before publishing to crates.io.');
      const publishEnv = { ...withoutToken(env), CARGO_REGISTRY_TOKEN: token };
      try {
        await command(run, 'cargo', staged.publishArgs, staged.directory, publishEnv);
      } catch (error) {
        // Cargo can upload successfully and then time out while waiting for its index entry.
        let afterFailure;
        try {
          afterFailure = await publishedChecksum(version, fetchIndex);
        } catch {
          throw error;
        }
        if (afterFailure !== staged.checksum) throw error;
      }
    }
    if (await checksum(staged.candidatePath) !== staged.checksum) {
      throw new Error('Cargo changed the validated archive while publishing.');
    }
    // GitHub Releases and SHA256SUMS must identify the actual registry payload.
    await copyFile(staged.candidatePath, staged.archivePath);
    return { alreadyPublished: Boolean(existing), checksum: staged.checksum };
  } catch (error) {
    throw failure(error, env);
  } finally {
    await staged.cleanup();
  }
}
