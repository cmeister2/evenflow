import assert from 'node:assert/strict';
import test from 'node:test';
import { manifestVersion, stampManifests, validateVersion } from './version.mjs';

const manifest = '# release fixture\r\n[package]\r\nname = "evenflow"\r\nversion = "0.1.0" # retained\r\n\r\n[dependencies]\r\nversion = "9.8.7"\r\n';
const lockfile = '# retained\r\nversion = 4\r\n\r\n[[package]]\r\nname = "dependency"\r\nversion = "0.1.0"\r\nsource = "registry+https://example.invalid"\r\n\r\n[[package]]\r\nname = "evenflow"\r\nversion = "0.1.0"\r\ndependencies = ["dependency"]\r\n';

test('stamping changes only the root package version and preserves original bytes', () => {
  const result = stampManifests(manifest, lockfile, '1.2.3');
  assert.equal(result.manifest, manifest.replace('version = "0.1.0"', 'version = "1.2.3"'));
  assert.equal(result.lockfile, lockfile.replace('name = "evenflow"\r\nversion = "0.1.0"', 'name = "evenflow"\r\nversion = "1.2.3"'));
  assert.equal(manifestVersion(result.manifest), '1.2.3');
  assert.deepEqual(stampManifests(result.manifest, result.lockfile, '1.2.3'), result);
});

test('stamping rejects missing, ambiguous, inherited and mismatched package metadata', () => {
  assert.throws(() => stampManifests(manifest, lockfile.replace('name = "evenflow"', 'name = "another"'), '1.0.0'), /local evenflow package/);
  assert.throws(() => stampManifests(manifest, lockfile + '\n[[package]]\nname = "evenflow"\nversion = "0.1.0"\n', '1.0.0'), /local evenflow package/);
  assert.throws(() => stampManifests(manifest, lockfile.replace('name = "evenflow"\r\nversion = "0.1.0"', 'name = "evenflow"\r\nversion = "0.2.0"'), '1.0.0'), /do not match/);
  assert.throws(() => stampManifests(manifest.replace('version = "0.1.0" # retained', 'version.workspace = true'), lockfile, '1.0.0'), /exactly one version/);
  assert.throws(() => stampManifests(manifest.replace('name = "evenflow"', 'name = "another"'), lockfile, '1.0.0'), /evenflow \[package\]/);
  assert.throws(() => stampManifests(manifest, lockfile.replace('name = "evenflow"', 'name = "evenflow"\r\nsource = "registry+https://example.invalid"'), '1.0.0'), /local evenflow package/);
});

test('release versions accept stable bare SemVer only', () => {
  for (const value of ['0.1.0', '1.0.0', '12.34.56']) assert.equal(validateVersion(value), value);
  for (const value of ['', 'v1.0.0', '1.2', '01.2.3', '1.2.3-beta.1', '1.2.3+build', '1.2.3\n', '../../evil', undefined]) {
    assert.throws(() => validateVersion(value), /stable major.minor.patch/);
  }
});
