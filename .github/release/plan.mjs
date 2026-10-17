import { execFileSync } from 'node:child_process';
import { appendFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import semanticRelease from 'semantic-release';
import releaseConfig from '../../release.config.mjs';
import { manifestVersion, stampManifests, validateVersion } from './version.mjs';

/** Plan without publishing plugins, then save versioned build inputs separately. */
export async function createPlan({
  cwd = process.cwd(), config = releaseConfig, env = process.env,
  stdout = process.stdout, stderr = process.stderr, output = env.GITHUB_OUTPUT,
  releaseEngine = semanticRelease,
} = {}) {
  const commit = execFileSync('git', ['rev-parse', 'HEAD'], {
    cwd, env, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
  }).trim();
  const manifest = await readFile(join(cwd, 'Cargo.toml'), 'utf8');
  const lockfile = await readFile(join(cwd, 'Cargo.lock'), 'utf8');
  const result = await releaseEngine({
    ...config, dryRun: true, plugins: config.plugins.slice(0, 2),
  }, { cwd, env, stdout, stderr });
  const releasing = Boolean(result?.nextRelease);
  const version = releasing ? validateVersion(result.nextRelease.version) : manifestVersion(manifest);
  if (releasing && result.nextRelease.gitHead !== commit) {
    throw new Error('The planned release commit differs from the producing checkout.');
  }
  const stamped = stampManifests(manifest, lockfile, version);
  const plan = { version, commit, releasing };
  const directory = join(cwd, 'release-prepared');
  await mkdir(directory, { recursive: true });
  await writeFile(join(directory, 'Cargo.toml'), stamped.manifest);
  await writeFile(join(directory, 'Cargo.lock'), stamped.lockfile);
  await writeFile(join(directory, 'plan.json'), `${JSON.stringify(plan, null, 2)}\n`);
  if (output) {
    await appendFile(output, `version=${version}\nreleasing=${releasing}\ncommit=${commit}\n`);
  }
  return plan;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  if (process.argv.length !== 2) {
    console.error('Usage: node .github/release/plan.mjs');
    process.exitCode = 1;
  } else {
    await createPlan();
  }
}
