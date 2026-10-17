/** Accept stable, bare SemVer versions for Cargo manifests and release filenames. */
export function validateVersion(version) {
  if (typeof version !== 'string' || !/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version)) {
    throw new Error(`Expected a stable major.minor.patch release version: ${version}`);
  }
  return version;
}

/** Locate TOML tables without reserializing their comments or whitespace. */
function tables(source, header) {
  const headers = [...source.matchAll(/^[ \t]*(\[\[?[^\r\n]+?\]\]?)[ \t]*(?:#[^\r\n]*)?\r?$/gm)];
  return headers.flatMap((match, index) => match[1] === header ? [{
    start: match.index,
    end: headers[index + 1]?.index ?? source.length,
    source: source.slice(match.index, headers[index + 1]?.index ?? source.length),
  }] : []);
}

/** Read a single ordinary quoted TOML property from a package table. */
function property(table, name) {
  const matches = [...table.source.matchAll(new RegExp(
    `^[ \\t]*${name}[ \\t]*=[ \\t]*"([^"\\r\\n]+)"[ \\t]*(?:#[^\\r\\n]*)?\\r?$`, 'gm',
  ))];
  if (matches.length !== 1) throw new Error(`Expected exactly one ${name} in the package table.`);
  return matches[0][1];
}

/** Find the root package while rejecting ambiguous or workspace-inherited metadata. */
function manifestPackage(manifest) {
  const packages = tables(manifest, '[package]');
  if (packages.length !== 1 || property(packages[0], 'name') !== 'evenflow') {
    throw new Error('Cargo.toml must contain exactly one evenflow [package] table.');
  }
  return packages[0];
}

/** Read the checked-in package version without consulting Cargo or the network. */
export function manifestVersion(manifest) {
  return validateVersion(property(manifestPackage(manifest), 'version'));
}

/** Replace just the quoted version, preserving every other byte of the TOML text. */
function replaceVersion(source, table, version) {
  const stamped = table.source.replace(
    /^([ \t]*version[ \t]*=[ \t]*")([^"\r\n]+)("[^\r\n]*\r?)$/m,
    (_, prefix, _previous, suffix) => `${prefix}${version}${suffix}`,
  );
  return source.slice(0, table.start) + stamped + source.slice(table.end);
}

/** Stamp only Evenflow's manifest and lockfile versions, after checking they agree. */
export function stampManifests(manifest, lockfile, version) {
  validateVersion(version);
  const root = manifestPackage(manifest);
  const previous = validateVersion(property(root, 'version'));
  const packages = tables(lockfile, '[[package]]').filter((table) => property(table, 'name') === 'evenflow');
  if (packages.length !== 1 || /^[ \t]*source[ \t]*=/m.test(packages[0]?.source ?? '')) {
    throw new Error('Cargo.lock must contain exactly one local evenflow package.');
  }
  if (validateVersion(property(packages[0], 'version')) !== previous) {
    throw new Error('Evenflow versions in Cargo.toml and Cargo.lock do not match.');
  }
  return {
    manifest: replaceVersion(manifest, root, version),
    lockfile: replaceVersion(lockfile, packages[0], version),
  };
}
