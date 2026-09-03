/**
 * Stage the DeepSeek Harness that gets bundled into the installer.
 *
 * Without this, the first launch has to download ~250 MB from npm before the
 * app is usable: twenty seconds of nothing, a hard dependency on the network at
 * exactly the wrong moment, and no way to report progress (this app has no UI of
 * its own by design). Shipping it inside the installer makes the install the
 * only thing that needs the network.
 *
 * Downloading during installation instead is not an option that generalises:
 * a macOS install is a drag into /Applications, with no install-time script at
 * all. Bundling behaves identically on all three platforms.
 *
 * The version is pinned. Bump it when cutting a release; the app updates itself
 * from npm afterwards regardless, so this is only the starting point.
 *
 * Run automatically by `tauri build` via `beforeBuildCommand`.
 */
import { execFileSync } from 'node:child_process'
import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

/** The harness version shipped in the installer. */
const HARNESS_VERSION = '0.1.1-rc.2'
const PACKAGE = '@deepseek-ai/dsh'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const OUT_DIR = join(ROOT, 'src-tauri', 'harness')
const PNPM = join(ROOT, 'node_modules', 'pnpm', 'bin', 'pnpm.cjs')
const BIN = join(OUT_DIR, 'node_modules', '@deepseek-ai', 'dsh', 'lib', 'bin.js')
const STAMP = join(OUT_DIR, 'version.txt')
const STAGED = join(OUT_DIR, '.staged')

/**
 * Bump when *what* gets staged changes, not just which version.
 *
 * version.txt records the harness version, so keying the skip on it alone means
 * a change to the pruning below leaves an already-staged tree untouched and
 * quietly ships the old contents. The target triple is part of the key too: a
 * cross-build must not reuse a tree pruned for another platform.
 */
const STAGE_REVISION = 3
const want = `${HARNESS_VERSION} r${STAGE_REVISION} ${process.env.TAURI_ENV_TARGET_TRIPLE || 'host'}`

if (existsSync(BIN) && existsSync(STAGED) && readFileSync(STAGED, 'utf8').trim() === want) {
  console.log(`fetch-harness: ${want} already staged`)
  process.exit(0)
}

if (!existsSync(PNPM)) {
  console.error('fetch-harness: pnpm is missing; run `npm install` first')
  process.exit(1)
}

console.log(`fetch-harness: staging ${PACKAGE}@${HARNESS_VERSION}`)
rmSync(OUT_DIR, { recursive: true, force: true })
mkdirSync(OUT_DIR, { recursive: true })
writeFileSync(join(OUT_DIR, 'package.json'), '{"name":"dsh-runtime","version":"0.0.0","private":true}')

// npm cannot install this graph in a usable time - see the pnpm module for the
// measurements - which is also why pnpm is what the app itself ships.
//
// A hoisted layout is required, not a preference: the bundler copies these files
// into the installer, and a default pnpm layout would be symlinks into a store
// that does not travel with them.
execFileSync(
  process.execPath,
  [
    PNPM,
    'add',
    `${PACKAGE}@${HARNESS_VERSION}`,
    '--dir',
    OUT_DIR,
    '--config.node-linker=hoisted',
    '--reporter=append-only',
  ],
  { stdio: 'inherit' },
)

if (!existsSync(BIN)) {
  console.error(`fetch-harness: install finished but ${BIN} is missing`)
  process.exit(1)
}

pruneForeignPlatforms(OUT_DIR)
prune(OUT_DIR)

/**
 * Drop native binaries built for platforms this installer will never run on.
 *
 * npm packages ship prebuilt `.node` addons for every platform they support, and
 * an installer for one platform carries all of them: sharp alone brings macOS,
 * Windows and musl-libc builds. They are dead weight everywhere, and on Linux
 * they are worse than that - linuxdeploy walks every ELF in the AppDir and
 * resolving the musl build fails outright ("Could not find dependency:
 * libc.musl-x86_64.so.1"), taking the whole AppImage down with it.
 *
 * Only directories whose names explicitly encode a foreign platform or
 * architecture are removed, so a package that merely happens to contain the word
 * is untouched.
 */
function pruneForeignPlatforms(root) {
  // `musl` earns its place separately from `linuxmusl`: packages also mark the
  // libc on its own, as in koffi's `linux_x64` beside `musl_x64`. Without the
  // bare token that directory reads as "our architecture" and survives, and its
  // musl-linked addon then breaks the AppImage exactly like sharp's did.
  const OS_TOKENS = ['darwin', 'win32', 'linux', 'linuxmusl', 'musl', 'android', 'freebsd', 'openbsd', 'sunos']
  const ARCH_TOKENS = ['x64', 'arm64', 'ia32', 'arm', 'x86', 'ppc64', 's390x', 'riscv64']

  const target = (() => {
    const triple = process.env.TAURI_ENV_TARGET_TRIPLE || ''
    const arch = /aarch64|arm64/.test(triple)
      ? 'arm64'
      : /x86_64/.test(triple)
        ? 'x64'
        : process.arch === 'arm64'
          ? 'arm64'
          : 'x64'
    const os = /windows|msvc/.test(triple)
      ? 'win32'
      : /darwin|apple/.test(triple)
        ? 'darwin'
        : /linux/.test(triple)
          ? 'linux'
          : process.platform
    return { os, arch }
  })()

  const dropped = { dirs: 0, bytes: 0 }

  const sizeOf = (dir) => {
    let total = 0
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const full = join(dir, entry.name)
      total += entry.isDirectory() ? sizeOf(full) : statSync(full).size
    }
    return total
  }

  /** Is this directory name a build for some platform other than the target? */
  const isForeign = (name) => {
    const tokens = name.toLowerCase().split(/[-_.]/)
    const os = tokens.filter(t => OS_TOKENS.includes(t))
    const arch = tokens.filter(t => ARCH_TOKENS.includes(t))
    if (os.length === 0 && arch.length === 0) return false
    // `linuxmusl` splits to `linuxmusl`, never to `linux`, so a glibc target
    // keeps `linux` and drops `linuxmusl` as intended.
    if (os.length > 0 && !os.includes(target.os)) return true
    return arch.length > 0 && !arch.includes(target.arch)
  }

  const walk = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue
      const full = join(dir, entry.name)
      if (isForeign(entry.name)) {
        dropped.bytes += sizeOf(full)
        dropped.dirs += 1
        rmSync(full, { recursive: true, force: true })
        continue
      }
      walk(full)
    }
  }
  walk(root)
  console.log(
    `fetch-harness: dropped ${dropped.dirs} foreign-platform directories ` +
      `(${Math.round(dropped.bytes / 1048576)} MB), keeping ${target.os}-${target.arch}`,
  )
}

/**
 * Drop what npm packages carry for developers but a runtime never reads.
 *
 * This is about file *count*, not bytes: an MSI installs every file as its own
 * journalled entry and the virus scanner opens each one, so ~29k files made
 * installation take over a minute. Type declarations, source maps and docs are
 * half of them.
 *
 * Deliberately conservative - only these three kinds, matched by extension.
 * Licence files stay: they must ship. Compiled `.js`, `.json`, `.node` and
 * anything else a module might `require` at runtime is untouched.
 */
function prune(root) {
  const dropped = { files: 0, bytes: 0 }
  const isJunk = (name) =>
    name.endsWith('.map') ||
    /\.d\.[cm]?ts$/.test(name) ||
    (name.toLowerCase().endsWith('.md') && !name.toLowerCase().startsWith('license'))

  const walk = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const full = join(dir, entry.name)
      if (entry.isDirectory()) {
        walk(full)
      } else if (entry.isFile() && isJunk(entry.name)) {
        dropped.bytes += statSync(full).size
        rmSync(full, { force: true })
        dropped.files += 1
      }
    }
  }
  walk(root)
  console.log(
    `fetch-harness: pruned ${dropped.files} dev-only files (${Math.round(dropped.bytes / 1048576)} MB)`,
  )
}

// The app reads this to know which version it is shipping.
writeFileSync(STAMP, `${HARNESS_VERSION}\n`)
// The build reads this to know whether re-staging is needed.
writeFileSync(STAGED, `${want}\n`)
console.log(`fetch-harness: staged ${want}`)
