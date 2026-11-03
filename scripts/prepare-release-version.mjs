import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const tag = process.argv[2];
if (!/^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-rc\.(0|[1-9]\d*))?$/.test(tag ?? '')) {
  throw new Error('Expected vMAJOR.MINOR.PATCH or vMAJOR.MINOR.PATCH-rc.N without leading zeros');
}
const version = tag.slice(1);
const baseVersion = version.replace(/-rc\.\d+$/, '');
const root = process.cwd();
const read = (path) => readFileSync(join(root, path), 'utf8');
const packageJson = JSON.parse(read('package.json'));
const packageLock = JSON.parse(read('package-lock.json'));
const tauriConfig = JSON.parse(read('src-tauri/tauri.conf.json'));
const cargoManifest = read('src-tauri/Cargo.toml');
const cargoLock = read('Cargo.lock');
const manifestPattern = /^(version = ")([^"]+)(")/m;
const lockPattern = /(\[\[package\]\]\r?\nname = "work-review"\r?\nversion = ")([^"]+)(")/;
const versions = [
  packageJson.version,
  packageLock.version,
  packageLock.packages?.['']?.version,
  tauriConfig.version,
  cargoManifest.match(manifestPattern)?.[2],
  cargoLock.match(lockPattern)?.[2],
];
if (!versions.every((value) => value === packageJson.version)) {
  throw new Error('The five version sources (including the package-lock root) must agree before release');
}
if (packageJson.version !== version && packageJson.version !== baseVersion) {
  throw new Error(`Tag ${tag} does not match application version ${packageJson.version}`);
}
if (packageJson.version !== version) {
  // RC 包必须自报完整 RC 版本，才能正确比较后续 RC 和同版本正式版。
  packageJson.version = version;
  packageLock.version = version;
  packageLock.packages[''].version = version;
  tauriConfig.version = version;
  const json = (value) => `${JSON.stringify(value, null, 2)}\n`;
  const replaceVersion = (_match, prefix, _old, suffix) => `${prefix}${version}${suffix}`;
  const files = [
    ['package.json', json(packageJson)],
    ['package-lock.json', json(packageLock)],
    ['src-tauri/tauri.conf.json', json(tauriConfig)],
    ['src-tauri/Cargo.toml', cargoManifest.replace(manifestPattern, replaceVersion)],
    ['Cargo.lock', cargoLock.replace(lockPattern, replaceVersion)],
  ];
  for (const [path, content] of files) writeFileSync(join(root, path), content, 'utf8');
}
console.log(`Validated release application version: ${version}`);
