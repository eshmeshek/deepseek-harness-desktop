/**
 * Stage the Node.js runtime that gets bundled into the installer.
 *
 * The harness is a Node application, so without this the user has to install
 * Node themselves and "download, install, run" is a lie. Bundling is preferred
 * over downloading at first launch: it needs no network at runtime, adds no
 * failure path to startup, and makes the installer self-contained.
 *
 * Only the `node` binary is taken, not the whole distribution - npm is not used
 * anywhere (pnpm is bundled separately), and the binary is self-contained.
 *
 * The version is pinned rather than tracked: a build should produce the same
 * artefact tomorrow as today. Bump NODE_VERSION deliberately.
 *
 * Run automatically by `tauri build` via `beforeBuildCommand`.
 */
import { createHash } from 'node:crypto'
import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

/** Current Node.js LTS. Satisfies the harness's `^22.19.0 || >=24` engine range. */
const NODE_VERSION = 'v24.20.0'
const DIST = `https://nodejs.org/dist/${NODE_VERSION}`

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const OUT_DIR = join(ROOT, 'src-tauri', 'node')

/**
 * Which archive to fetch, and which member of it is the binary.
 *
 * Keyed by Rust target triple so cross-builds (the CI matrix builds both macOS
 * architectures) stage the right one rather than the host's.
 */
const TARGETS = {
  'x86_64-pc-windows-msvc': { slug: 'win-x64', ext: 'zip', member: 'node.exe', out: 'node.exe' },
  'aarch64-pc-windows-msvc': { slug: 'win-arm64', ext: 'zip', member: 'node.exe', out: 'node.exe' },
  'aarch64-apple-darwin': { slug: 'darwin-arm64', ext: 'tar.gz', member: 'bin/node', out: 'node' },
  'x86_64-apple-darwin': { slug: 'darwin-x64', ext: 'tar.gz', member: 'bin/node', out: 'node' },
  'x86_64-unknown-linux-gnu': { slug: 'linux-x64', ext: 'tar.gz', member: 'bin/node', out: 'node' },
  'aarch64-unknown-linux-gnu': { slug: 'linux-arm64', ext: 'tar.gz', member: 'bin/node', out: 'node' },
}

/** Fall back to the host when Tauri did not name a target (a plain local build). */
function hostTriple() {
  const arch = process.arch === 'arm64' ? 'aarch64' : 'x86_64'
  if (process.platform === 'win32') return `${arch}-pc-windows-msvc`
  if (process.platform === 'darwin') return `${arch}-apple-darwin`
  return `${arch}-unknown-linux-gnu`
}

const triple = process.env.TAURI_ENV_TARGET_TRIPLE || hostTriple()
const target = TARGETS[triple]
if (!target) {
  console.error(`fetch-node: unsupported target ${triple}`)
  process.exit(1)
}

const archive = `node-${NODE_VERSION}-${target.slug}.${target.ext}`
const stamp = join(OUT_DIR, '.staged')
const binary = join(OUT_DIR, target.out)
const want = `${NODE_VERSION} ${triple}`

// Staging is the slow part of a rebuild; skip it when the wanted binary is
// already there. The stamp records what was staged, so a version bump or a
// different target still re-stages.
if (existsSync(binary) && existsSync(stamp) && readFileSync(stamp, 'utf8').trim() === want) {
  console.log(`fetch-node: ${want} already staged`)
  process.exit(0)
}

const tmp = join(OUT_DIR, '.tmp')
rmSync(tmp, { recursive: true, force: true })
mkdirSync(tmp, { recursive: true })

async function download(url) {
  const response = await fetch(url)
  if (!response.ok) throw new Error(`${url} -> HTTP ${response.status}`)
  return Buffer.from(await response.arrayBuffer())
}

console.log(`fetch-node: staging ${want}`)

// Verify against the published checksums. An unverified runtime binary is not
// something to ship inside an installer, however convenient the download was.
const sums = (await download(`${DIST}/SHASUMS256.txt`)).toString('utf8')
const expected = sums
  .split('\n')
  .map(line => line.trim().split(/\s+/))
  .find(([, name]) => name === archive)?.[0]
if (!expected) throw new Error(`no checksum published for ${archive}`)

const blob = await download(`${DIST}/${archive}`)
const actual = createHash('sha256').update(blob).digest('hex')
if (actual !== expected) {
  throw new Error(`checksum mismatch for ${archive}\n  expected ${expected}\n  got      ${actual}`)
}

const archivePath = join(tmp, archive)
writeFileSync(archivePath, blob)

// bsdtar ships with Windows 10+ as well as macOS and Linux, and reads zip and
// tar.gz alike, so one extraction path covers every platform.
//
// Addressed absolutely on Windows rather than through PATH: in a Git Bash shell
// `tar` resolves to GNU tar, which reads `C:\...` as a remote `host:path` and
// fails with "Cannot connect to C". The system bsdtar has no such ambiguity.
const tarBin =
  process.platform === 'win32'
    ? join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'tar.exe')
    : 'tar'
const member = `node-${NODE_VERSION}-${target.slug}/${target.member}`
execFileSync(tarBin, ['-xf', archivePath, '-C', tmp, member], { stdio: 'inherit' })

rmSync(binary, { force: true })
renameSync(join(tmp, member), binary)
rmSync(tmp, { recursive: true, force: true })
writeFileSync(stamp, `${want}\n`)

console.log(`fetch-node: staged ${binary}`)
