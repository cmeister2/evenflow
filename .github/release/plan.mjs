import { execFileSync } from 'node:child_process';
import { appendFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import semanticRelease from 'semantic-release';
import releaseConfig from '../../release.config.mjs';
import { previewRelease } from './preview.mjs';
import { manifestVersion, stampManifests, validateVersion } from './version.mjs';

/** Prepare versioned build inputs from a publication plan or a local release preview. */
export async function createPlan({
  cwd = process.cwd(), config = releaseConfig, env = process.env,
  stdout = process.stdout, stderr = process.stderr, output = env.GITHUB_OUTPUT,
  releaseEngine = semanticRelease, previewEngine = previewRelease, preview = false,
} = {}) {
  const commit = execFileSync('git', ['rev-parse', 'HEAD'], {
    cwd, env, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
  }).trim();
  const manifest = await readFile(join(cwd, 'Cargo.toml'), 'utf8');
  const lockfile = await readFile(join(cwd, 'Cargo.lock'), 'utf8');
  const result = preview ? await previewEngine({ cwd, config, env, stdout, stderr }) : await releaseEngine({
    ...config, dryRun: true, plugins: config.plugins.slice(0, 2),
  }, { cwd, env, stdout, stderr });
  const nextRelease = result?.nextRelease;
  const releasing = !preview && Boolean(nextRelease);
  const version = nextRelease ? validateVersion(nextRelease.version) : manifestVersion(manifest);
  if (nextRelease && nextRelease.gitHead !== commit) {
    throw new Error('The planned release commit differs from the producing checkout.');
  }
  const stamped = stampManifests(manifest, lockfile, version);
  const plan = { version, commit, releasing };
  const directory = join(cwd, 'release-prepared');
  await mkdir(directory, { recursive: true });
  await writeFile(join(directory, 'Cargo.toml'), stamped.manifest);
  await writeFile(join(directory, 'Cargo.lock'), stamped.lockfile);
  await writeFile(join(directory, 'plan.json'), `${JSON.stringify(plan, null, 2)}\n`);
  const notes = nextRelease
    ? nextRelease.notes || `Planned version: ${version}.\n`
    : 'No release-worthy changes.\n';
  await writeFile(join(directory, 'RELEASE_NOTES.md'), notes.endsWith('\n') ? notes : `${notes}\n`);
  if (output) {
    await appendFile(output, `version=${version}\nreleasing=${releasing}\ncommit=${commit}\n`);
  }
  if (env.GITHUB_STEP_SUMMARY) {
    const heading = preview ? 'Release preview' : 'Publication plan';
    const description = nextRelease
      ? `Proposed version: **${version}**.`
      : `No release-worthy changes. Build version: **${version}**.`;
    await appendFile(env.GITHUB_STEP_SUMMARY, `## ${heading}\n\n${description}\n\n${notes}\n`);
  }
  return plan;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const args = process.argv.slice(2);
  if (args.length > 1 || (args.length === 1 && args[0] !== '--preview')) {
    console.error('Usage: node .github/release/plan.mjs [--preview]');
    process.exitCode = 1;
  } else {
    await createPlan({ preview: args.length === 1 });
  }
}
