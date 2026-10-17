import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);

export default {
  branches: ['main'],
  tagFormat: '${version}',
  plugins: [
    [require.resolve('@semantic-release/commit-analyzer'), { preset: 'conventionalcommits' }],
    [require.resolve('@semantic-release/release-notes-generator'), { preset: 'conventionalcommits' }],
    './.github/release/plugin.mjs',
    [require.resolve('@semantic-release/github'), {
      assets: [
        { path: 'dist/evenflow-*.tar.gz' },
        { path: 'dist/evenflow-*.zip' },
        { path: 'dist/evenflow-*.crate' },
        { path: 'dist/SHA256SUMS' },
      ],
      successCommentCondition: false,
      failCommentCondition: false,
      releasedLabels: false,
    }],
  ],
};
