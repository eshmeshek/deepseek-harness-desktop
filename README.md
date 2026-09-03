# DSH Desktop

Desktop launcher, background service and update prompt for
[DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness).

> ### Not affiliated with DeepSeek
>
> This is an independent community project. It is **not** an official DeepSeek
> product, and it is not endorsed by, sponsored by, or affiliated with DeepSeek
> in any way. All credit for the harness itself belongs upstream.
>
> "DeepSeek" and "DeepSeek Harness" are trademarks of their respective owner.
> They are used here only to describe truthfully what this project works with,
> as the upstream [brand guidelines](https://github.com/deepseek-ai/deepseek-harness/blob/master/BRAND_GUIDELINES.md)
> permit. **The application icon is the upstream project's own mark**, reused
> from the MIT-licensed repository; it indicates what this launcher launches and
> is not a claim of authorship or official status. If the upstream project would
> rather it were not used this way, open an issue and it will be changed.

## What it adds

DeepSeek Harness ships a browser UI (`dsh web`) and nothing else: no desktop
app, no background service, and **no updater** — there is no version check and
no "update available" button anywhere in it. DSH Desktop adds exactly those
three things, and deliberately nothing more:

| | |
|---|---|
| **Runs in the background** | The harness host keeps running when you close the window, so a task in flight is not tied to whether the UI is open. |
| **One real stop** | A tray icon whose **Quit** is the only thing that shuts the host down. |
| **Offers updates** | On start it asks npm whether a newer harness exists and offers to install it. Never automatic — upstream is a developer preview that announces breaking changes. |

**It contributes no interface of its own.** The only page it ever shows is the
harness's own web UI. Everything this app has to say — progress, failures,
update offers — it says through native OS dialogs.

## Install

Download an installer from [Releases](../../releases):

| Platform | File |
|---|---|
| Windows | `-setup.exe` |
| Linux | `.AppImage`, `.deb` or `.rpm` |

macOS is not built yet: it needs signing and notarisation on top of a build,
without which Gatekeeper refuses a downloaded `.dmg` outright.

**No `.msi` is published.** It was built at first and dropped: it did not work,
and it was the slow one anyway — measured on one machine with identical content,
a plain file copy of the payload took 7.6 s, the NSIS installer 10 s, and the
MSI over a minute. MSI registers every one of the ~14,000 bundled files as its
own component and journals each for rollback, which no amount of trimming fixes.

On Linux the `.AppImage` needs no installation at all: make it executable and
run it. The `.deb` and `.rpm` additionally add a menu entry.

**Nothing else needs to be installed.** No Node.js, no npm, no pnpm, no git, no
checkout: the app ships its own Node runtime and its own package manager. Install
it and run it.

(If the machine happens to have a suitable Node of its own, and the bundled copy
somehow will not run, the app falls back to it rather than refusing to start.)

## How it works

Nothing is patched, forked or vendored. Upstream is consumed exactly as
published on npm:

```
<app data>/runtimes/<version>/node_modules/@deepseek-ai/dsh/lib/bin.js
```

Each version lands in its own directory, so an update is a directory switch
rather than a mutation, and the previous version stays on disk as a rollback
target. On launch the app starts that binary as `dsh web --no-open --port 0`,
reads the loopback URL the host prints, and opens it in a native window.

### What is bundled, and why

| | size | why it is not left to the machine |
|---|---|---|
| Node.js (pinned LTS) | ~90 MB | The harness is a Node app. Requiring a preinstalled Node makes "download, install, run" untrue. Fetched at build time from nodejs.org and **verified against the published SHA-256** before it goes anywhere near an installer. |
| DeepSeek Harness | ~240 MB | So the first launch is instant and offline. Downloading it at first run instead meant twenty seconds of nothing, with no way to show progress and a hard dependency on the network at the worst moment. Downloading it *during installation* is not portable: a macOS install is a drag into /Applications, with no install-time script at all. |
| pnpm | ~19 MB | See the measurements below. |

All three are staged at build time, so **installation is the only step that needs
the network**. First launch reads the bundled harness in place — verified to work
from a write-denied directory, which is what Program Files is — and starts in
about two seconds. Updates still come from npm, into app data, on top.

### Why pnpm and not npm

The harness is not one package but roughly a hundred separately published
`@deepseek-ai/dsh-*` modules, ~500 packages once resolved. Measured on one
machine, same network and registry:

| installer | result |
|---|---|
| `npm install` | still resolving after 12 minutes, 0 bytes written |
| `pnpm add` | finished in 22 s — 504 packages, 243 MB |

So the package manager cannot be left to whatever is on the machine: with npm,
first launch is not slow, it is effectively broken. A copy of pnpm (~19 MB) is
shipped inside the app and driven directly, which also means the machine needs
no package manager of its own.

Budget about 250 MB per installed harness version; the app keeps the current one
and one previous, and files are hardlinked from a shared store, so two versions
cost far less than twice that.

That is also what keeps updates simple: **updating means fetching a newer
upstream, not rebasing local changes onto it**, because there are no local
changes to rebase.

## Build from source

Needs [Rust](https://rustup.rs/), Node.js and the
[Tauri prerequisites](https://tauri.app/start/prerequisites/) for your platform.

```sh
npm install
npm run dev      # run it
npm run build    # produce installers for the current platform
```

Cross-platform artifacts are built by CI on tag push; a single machine can only
bundle for its own OS.

## License

[MIT](LICENSE). DeepSeek Harness itself is MIT-licensed upstream and is not
redistributed here — it is downloaded from npm at runtime.
