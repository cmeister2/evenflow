import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { chmod, lstat, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const repositoryApi = 'https://api.github.com/repos/cmeister2/evenflow';
const stableVersion = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const targets = {
  'linux-x64': 'x86_64-unknown-linux-gnu',
  'darwin-arm64': 'aarch64-apple-darwin',
  'win32-x64': 'x86_64-pc-windows-msvc',
};

/** Read action inputs without interpreting paths as shell arguments. */
export function readInputs(env) {
  const version = env.INPUT_VERSION?.trim() || 'latest';
  if (version !== 'latest' && !stableVersion.test(version)) {
    throw new Error('version must be latest or a bare major.minor.patch release version (without v).');
  }
  const strict = env.INPUT_STRICT?.trim() || 'true';
  if (strict !== 'true' && strict !== 'false') {
    throw new Error('strict must be true or false.');
  }
  const paths = (env.INPUT_PATHS ?? '.').split(/\r?\n/).map((path) => path.trim()).filter(Boolean);
  const cwd = resolve(env.GITHUB_WORKSPACE || process.cwd(), env['INPUT_WORKING-DIRECTORY']?.trim() || '.');
  return { version, strict: strict === 'true', paths: paths.length ? paths : ['.'], cwd, token: env.INPUT_TOKEN?.trim() };
}

/** Bind each platform to one of the archives produced by the release pipeline. */
export function releaseArchive(version, platform, arch) {
  const target = targets[`${platform}-${arch}`];
  if (!target) {
    throw new Error(`Unsupported runner ${platform}/${arch}. Evenflow supports Linux x64, macOS ARM64, and Windows x64.`);
  }
  const root = `evenflow-${version}-${target}`;
  return {
    name: `${root}.${platform === 'win32' ? 'zip' : 'tar.gz'}`,
    root,
    binary: platform === 'win32' ? 'evenflow.exe' : 'evenflow',
  };
}

/** Reject missing, ambiguous, or mismatched checksums before extracting code. */
export function verifyChecksum(archive, checksums, name) {
  const entries = checksums.split(/\r?\n/).flatMap((line) => {
    const entry = /^([a-f\d]{64})  (.+)$/.exec(line);
    return entry?.[2] === name ? [entry[1]] : [];
  });
  const actual = createHash('sha256').update(archive).digest('hex');
  if (entries.length !== 1 || entries[0] !== actual) {
    throw new Error(`SHA256SUMS verification failed for ${name}.`);
  }
}

/** Download from the release API, authenticating only requests to GitHub. */
async function githubRequest(url, token, request, accept = 'application/vnd.github+json') {
  const headers = { Accept: accept, 'User-Agent': 'evenflow-action', 'X-GitHub-Api-Version': '2022-11-28' };
  if (token) headers.Authorization = `Bearer ${token}`;
  const response = await request(url, { headers, signal: AbortSignal.timeout(60_000) });
  if (!response.ok) {
    throw new Error(`GitHub release request failed (HTTP ${response.status}): ${url}`);
  }
  return response;
}

/** Fetch one named asset using the API so authenticated private releases work too. */
async function downloadAsset(release, name, token, request) {
  const assets = release.assets?.filter((asset) => asset.name === name) ?? [];
  if (assets.length !== 1 || !Number.isSafeInteger(assets[0].id) || assets[0].id <= 0) {
    throw new Error(`Release ${release.tag_name} must contain exactly one ${name} asset.`);
  }
  const response = await githubRequest(`${repositoryApi}/releases/assets/${assets[0].id}`, token, request, 'application/octet-stream');
  return Buffer.from(await response.arrayBuffer());
}

/** Treat failed extraction and execution as action errors, preserving lint exits. */
function executeCommand(execute, command, args, options) {
  const result = execute(command, args, { ...options, stdio: 'inherit', shell: false });
  if (result.error) throw new Error(`Cannot execute ${command}: ${result.error.message}`);
  if (result.signal || result.status === null) throw new Error(`${command} terminated before completing.`);
  return result.status;
}

/** Install a verified binary in temporary storage and check the caller's checkout. */
export async function runAction({
  env = process.env, platform = process.platform, arch = process.arch,
  request = fetch, execute = spawnSync, output = process.stdout,
} = {}) {
  const inputs = readInputs(env);
  // Reject unsupported runners and invalid working directories before downloading.
  releaseArchive(inputs.version, platform, arch);
  if (!(await lstat(inputs.cwd)).isDirectory()) throw new Error(`working-directory is not a directory: ${inputs.cwd}`);
  const releasePath = inputs.version === 'latest' ? 'latest' : `tags/${inputs.version}`;
  const response = await githubRequest(`${repositoryApi}/releases/${releasePath}`, inputs.token, request);
  const release = await response.json();
  if (!stableVersion.test(release.tag_name) || release.draft || release.prerelease
      || (inputs.version !== 'latest' && inputs.version !== release.tag_name)) {
    throw new Error('The selected GitHub release must have a matching stable major.minor.patch tag.');
  }
  const archive = releaseArchive(release.tag_name, platform, arch);
  const [bytes, checksums] = await Promise.all([
    downloadAsset(release, archive.name, inputs.token, request),
    downloadAsset(release, 'SHA256SUMS', inputs.token, request),
  ]);
  verifyChecksum(bytes, checksums.toString('utf8'), archive.name);
  const directory = await mkdtemp(join(env.RUNNER_TEMP || tmpdir(), 'evenflow-action-'));
  try {
    const archivePath = join(directory, archive.name);
    await writeFile(archivePath, bytes);
    let extractionStatus;
    if (platform === 'win32') {
      extractionStatus = executeCommand(execute, 'powershell.exe', [
        '-NoLogo', '-NoProfile', '-NonInteractive', '-Command',
        '$ErrorActionPreference = "Stop"; Expand-Archive -LiteralPath $env:EVENFLOW_ARCHIVE -DestinationPath $env:EVENFLOW_EXTRACT -Force',
      ], { cwd: directory, env: { ...env, EVENFLOW_ARCHIVE: archivePath, EVENFLOW_EXTRACT: directory } });
    } else {
      extractionStatus = executeCommand(execute, 'tar', ['-xzf', archivePath, '-C', directory], { cwd: directory, env });
    }
    if (extractionStatus !== 0) throw new Error(`Cannot extract ${archive.name} (exit ${extractionStatus}).`);
    const binary = join(directory, archive.root, archive.binary);
    if (!(await lstat(binary)).isFile()) throw new Error('The release archive does not contain an ordinary Evenflow executable.');
    if (platform !== 'win32') await chmod(binary, 0o755);
    const args = ['check', ...(inputs.strict ? ['--strict'] : []), '--', ...inputs.paths];
    output.write(`Evenflow ${release.tag_name}: checking ${inputs.cwd}\n`);
    // Source filenames and diagnostic prose must not become runner commands.
    const commandToken = randomUUID();
    output.write(`::stop-commands::${commandToken}\n`);
    let status;
    try {
      status = executeCommand(execute, binary, args, { cwd: inputs.cwd, env });
    } finally {
      output.write(`::${commandToken}::\n`);
    }
    if (![0, 1, 2].includes(status)) throw new Error(`Evenflow exited unexpectedly (exit ${status}).`);
    return status;
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}
