import { execFileSync } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import semanticRelease from 'semantic-release';
import { analyzeCommits } from '@semantic-release/commit-analyzer';
import { generateNotes } from '@semantic-release/release-notes-generator';
import releaseConfig from '../../release.config.mjs';

const root = fileURLToPath(new URL('../..', import.meta.url));
const require = createRequire(import.meta.url);
const manifest = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'));
const packageRepository = typeof manifest.repository === 'string' ? manifest.repository : manifest.repository?.url;

/** Reuse each official analysis plugin's configured options in the preview. */
function pluginOptions(config, name) {
  const resolved = require.resolve(name);
  const entry = config.plugins.find((entry) => {
    const plugin = Array.isArray(entry) ? entry[0] : entry;
    return plugin === name || plugin === resolved;
  });
  if (!entry) throw new Error(`Release preview requires the configured ${name} plugin.`);
  return Array.isArray(entry) ? entry[1] ?? {} : {};
}

/** Analyze real release history without credentials, remote writes, or source edits. */
export async function previewRelease({
  cwd = root, env = process.env, stdout = process.stdout, stderr = process.stderr,
  config = releaseConfig,
} = {}) {
  // semantic-release otherwise skips PRs or verifies write access to GitHub.
  const previewEnv = { ...env };
  for (const name of Object.keys(previewEnv)) {
    if (name.startsWith('GITHUB_') || [
      'CI', 'GH_TOKEN', 'GIT_CREDENTIALS', 'GL_TOKEN', 'GITLAB_TOKEN', 'BB_TOKEN',
      'BITBUCKET_TOKEN', 'BB_TOKEN_BASIC_AUTH', 'BITBUCKET_TOKEN_BASIC_AUTH',
    ].includes(name)) delete previewEnv[name];
  }
  const git = (directory, ...args) => execFileSync('git', args, {
    cwd: directory, env: previewEnv, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
  }).trim();
  if (git(cwd, 'rev-parse', '--is-shallow-repository') !== 'false') {
    throw new Error('Release preview requires full Git history and tags; fetch with depth 0.');
  }
  const head = git(cwd, 'rev-parse', 'HEAD');
  const repositoryUrl = (config.repositoryUrl ?? packageRepository)?.replace(/^git\+/, '');
  if (!repositoryUrl) throw new Error('Release preview requires the original repository URL for release notes.');
  const analyzerOptions = pluginOptions(config, '@semantic-release/commit-analyzer');
  const notesOptions = pluginOptions(config, '@semantic-release/release-notes-generator');
  const temporary = await mkdtemp(join(tmpdir(), 'evenflow-release-preview-'));
  const remote = join(temporary, 'remote.git');
  const checkout = join(temporary, 'checkout');
  const branch = 'release-preview';
  try {
    git(temporary, 'clone', '--quiet', '--mirror', '--no-hardlinks', resolve(cwd), remote);
    git(remote, 'update-ref', `refs/heads/${branch}`, head);
    git(remote, 'symbolic-ref', 'HEAD', `refs/heads/${branch}`);
    // Keep source configuration out of discovery: it may need uninstalled dependencies.
    git(temporary, 'clone', '--quiet', '--no-checkout', '--branch', branch, remote, checkout);

    let analyzed = false;
    const result = await semanticRelease({
      ...config,
      branches: [branch],
      repositoryUrl: pathToFileURL(remote).href,
      ci: false,
      dryRun: true,
      plugins: [
        [{ analyzeCommits: async (options, context) => {
          analyzed = true;
          return analyzeCommits(options, context);
        } }, analyzerOptions],
        [{ generateNotes: (options, context) => generateNotes(options, {
          ...context, options: { ...context.options, repositoryUrl },
        }) }, notesOptions],
      ],
    }, { cwd: checkout, env: previewEnv, stdout, stderr });
    if (!analyzed) throw new Error('Release preview skipped commit analysis.');
    if (result && result.nextRelease?.gitHead !== head) {
      throw new Error('Release preview analyzed a different commit from the source checkout.');
    }
    stdout.write(result
      ? `Preview complete: ${result.nextRelease.version}. No release was published.\n`
      : 'Preview complete: no release-worthy changes.\n');
    return result;
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  await previewRelease();
}
