// Pre-build step for `tauri build` / `tauri dev` (beforeBuildCommand / beforeDevCommand).
//
// 1. Builds the elevated helper `fc-helper` and copies it where Tauri expects the
//    `externalBin` sidecar: app/src-tauri/binaries/fc-helper-<target-triple>[.exe].
//    `universal-apple-darwin` (not a real cargo target) builds aarch64 + x86_64 and lipo's them.
// 2. Windows release builds: builds `fc_shellext_win.dll`, and the sparse package
//    `packaging/windows/out/FastCopy.msix` only if it is missing AND FASTCOPY_BUILD_SPARSE=1
//    (self-signed dev cert). Both are bundle resources declared in
//    src-tauri/tauri.windows.conf.json. CI should build/sign the MSIX beforehand; an existing
//    non-empty MSIX is never rebuilt here.
//
// Usage: node scripts/prepare-sidecar.mjs [--debug]
// Honours TAURI_ENV_TARGET_TRIPLE (set by `tauri build --target …`) and CARGO_TARGET_DIR.
import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync, existsSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const appDir = resolve(here, '..');
const repo = resolve(appDir, '..');
const debug = process.argv.includes('--debug');
const profile = debug ? 'debug' : 'release';
const targetDir = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(repo, 'target');
const outDir = join(appDir, 'src-tauri', 'binaries');

const run = (cmd, args) => {
  console.log(`[prepare] ${cmd} ${args.join(' ')}`);
  execFileSync(cmd, args, { cwd: repo, stdio: 'inherit' });
};
const nonEmpty = (p) => existsSync(p) && statSync(p).size > 0;

const host = (() => {
  const m = execFileSync('rustc', ['-vV'], { encoding: 'utf8' }).match(/^host:\s*(\S+)/m);
  if (!m) throw new Error('cannot determine the Rust host triple from `rustc -vV`');
  return m[1];
})();
const triple = process.env.TAURI_ENV_TARGET_TRIPLE || host;

/** Build fc-helper for one real cargo target and return the built file path. */
function buildHelper(t) {
  const cross = t !== host;
  const args = ['build', '-p', 'fc-helper'];
  if (!debug) args.push('--release');
  if (cross) args.push('--target', t);
  run('cargo', args);
  const exe = `fc-helper${t.includes('windows') ? '.exe' : ''}`;
  const built = join(targetDir, ...(cross ? [t] : []), profile, exe);
  if (!existsSync(built)) throw new Error(`fc-helper not found at ${built}`);
  return built;
}

mkdirSync(outDir, { recursive: true });
const ext = triple.includes('windows') ? '.exe' : '';
const dest = join(outDir, `fc-helper-${triple}${ext}`);

if (triple === 'universal-apple-darwin') {
  const parts = ['aarch64-apple-darwin', 'x86_64-apple-darwin'].map(buildHelper);
  run('lipo', ['-create', '-output', dest, ...parts]);
} else {
  copyFileSync(buildHelper(triple), dest);
}
console.log(`[prepare] sidecar ${dest} (${(statSync(dest).size / 1024).toFixed(0)} KiB)`);

// ── Windows 11 Explorer integration resources (release bundles only) ──
if (triple.includes('windows') && !debug) {
  const crossArgs = triple !== host ? ['--target', triple] : [];
  try {
    run('cargo', ['build', '-p', 'fc-shellext-win', '-p', 'fc-cli', '--release', ...crossArgs]);
  } catch (e) {
    console.warn(`[prepare] WARNING: fc-shellext-win / fc-cli build failed (${e.message}); the bundle needs target/release/fc_shellext_win.dll and fcopy.exe`);
  }
  const msix = join(repo, 'packaging', 'windows', 'out', 'FastCopy.msix');
  const script = join(repo, 'packaging', 'windows', 'build-sparse.ps1');
  if (!nonEmpty(msix)) {
    // Building the MSIX with -SelfSigned creates a dev certificate in the user's store:
    // only do it when explicitly asked. CI builds and signs it beforehand.
    if (process.env.FASTCOPY_BUILD_SPARSE === '1' && existsSync(script)) {
      try {
        run('powershell', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', script, '-SelfSigned']);
      } catch (e) {
        console.warn(`[prepare] WARNING: could not build FastCopy.msix (${e.message}).`);
      }
    }
    if (!nonEmpty(msix)) {
      console.log(
        '[prepare] note: no FastCopy.msix — the installer provides the classic context menu only.\n' +
          '  For the Windows 11 top-level menu, build/sign it (packaging/windows/build-sparse.ps1) and add\n' +
          '  --config packaging/windows/tauri.windows.bundle.json (the release workflow does this).',
      );
    }
  }
}
