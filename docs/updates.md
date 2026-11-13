# In-app updates

Tabler updates itself with the official Tauri v2 updater (`tauri-plugin-updater`). A build that
came from the [Releases page](https://github.com/kungfufafa/tabler/releases) looks for a newer
build of its own channel, downloads it, checks its signature and restarts into it.

- [What the user sees](#what-the-user-sees)
- [Versions and channels](#versions-and-channels)
- [What changed in a version](#what-changed-in-a-version)
- [What a release publishes](#what-a-release-publishes)
- [One-time setup: the signing key](#one-time-setup-the-signing-key)
- [Builds that do not update themselves](#builds-that-do-not-update-themselves)
- [What protects an update](#what-protects-an-update)
- [macOS while the build is ad-hoc signed](#macos-while-the-build-is-ad-hoc-signed)
- [Windows](#windows)
- [Trying an update between two local builds](#trying-an-update-between-two-local-builds)
- [Where the code is](#where-the-code-is)

## What the user sees

- **Check for Updates…** in the app menu on macOS (under About Tabler) and in Help on Windows.
  It opens a small dialog: "Tabler is up to date", or the new version with what changed.
- **Download Update** downloads with a progress bar (Cancel stops it). The download is verified
  before anything else happens.
- **Install and Restart** replaces the app and restarts it. It is refused while a tab or another
  window holds unsaved work, the same way a restart from Settings ▸ Locations is.
- **At launch** the app checks quietly, a few seconds after the first window opens. It says
  nothing unless there is a newer build, and then only shows a notice with an **Update…**
  button. A failed check (offline, a release being published) is not shown.
  Settings ▸ General ▸ Application ▸ **Automatically check for updates** turns this off; the
  menu item always works. The same row names the running build and has **Check Now**.
- **About Tabler** shows the version and, in parentheses, the commit the build was made from:
  `Version 0.1.7 (1a2b3c4)`.

## Versions and channels

Nobody bumps a version by hand. Every build that is published gets the next number:

- Each push to `main` queues a build. After CI and both installers pass, the workflow tags the
  exact commit and publishes a numbered release, then refreshes the rolling Nightly preview.
  Versions increase by one patch from the highest plain `v*` tag. A failed build or CI run leaves
  the previous release available. If publishing stops after tagging, a retry reuses that version.
- To start a new line (`0.2.0`, `1.0.0`), push that tag by hand: `git tag v0.2.0 && git push origin
v0.2.0`. That builds exactly `0.2.0` as a numbered release, and the automatic builds continue
  from it: `0.2.1`, `0.2.2`, …

The version CI computes is written into `package.json`, `src-tauri/Cargo.toml` and
`src-tauri/tauri.conf.json` of the CI checkout before the build, so the installer's name, About
Tabler, the update check and the crash reports (`tabler@0.1.7`) all carry it. Nothing is committed
back: in the repository those three files keep one common base version (`0.1.0`), which only
matters as the floor to count from (raise it in all three to move the floor; a test checks they
agree). A build made on a developer's machine therefore says `0.1.0`.

There are two channels, and a build follows the one it was made for:

| Channel | Builds                                           | Asks                                    |
| ------- | ------------------------------------------------ | --------------------------------------- |
| stable  | automatic builds from `main` and plain `v*` tags | `releases/latest/download/latest.json`  |
| nightly | builds manually published to the Nightly preview | `releases/download/nightly/latest.json` |

Nightly also mirrors the automatic release's installers, so those copies follow the stable
channel. The Nightly manifest is refreshed for existing installs that follow its address. A tag
with a pre-release suffix (`v1.0.0-rc.1`) is published as a GitHub pre-release with signed
installers and has no update channel.

`dev/scripts/build-version.mjs` chooses one version per workflow run and reserves it when the
nightly installers are ready to publish. `dev/scripts/release-config.mjs` applies the version
and writes the Tauri override for the channel's update address and updater bundles.

## What changed in a version

Each version says what changed, where the user meets it: in the update dialog and on the GitHub
release (titled `Tabler 0.1.7 (nightly)` or `Tabler 0.2.0`).

`dev/scripts/release-notes.mjs` writes the notes from the commit subjects between a version's tag
and the tag before it:

| Commit subject starts with           | Listed under |
| ------------------------------------ | ------------ |
| `feat:`                              | New          |
| `fix:`                               | Fixed        |
| `ui:`, `perf:`, `refactor:`          | Improved     |
| anything else (`redis: …`, no type)  | Changed      |
| `ci:`, `test:`, `docs:`, `chore:`, … | left out     |

So the notes are as good as the commit subjects: write them as what changed for the user. A
change reaches `main` as a squash-merged pull request, so its commit subject is the pull request's
title, to which GitHub adds the number (`fix: the sidebar keeps its width (#14)`). The GitHub
release keeps that number, as a link to the pull request; the update dialog leaves it out. A
version whose commits are all left out says "Maintenance build". The release lists the newest
eight versions, newest first; the dialog shows only the ones newer than the running build, so
someone three versions behind reads what all three brought.

## What a release publishes

`.github/workflows/build.yml` builds the installers and, with the signing key, the updater
bundles next to them:

| Platform | Updater bundle                   | Signature                            |
| -------- | -------------------------------- | ------------------------------------ |
| macOS    | `Tabler.app.tar.gz` (universal)  | `Tabler.app.tar.gz.sig`              |
| Windows  | `Tabler_<version>_x64-setup.exe` | `Tabler_<version>_x64-setup.exe.sig` |
| Windows  | `Tabler_<version>_x64_en-US.msi` | `Tabler_<version>_x64_en-US.msi.sig` |

On Windows the installers are their own updater bundles. The publish job then writes
`latest.json` with `dev/scripts/updater-manifest.mjs`, tags the commit `v<version>` (builds of
`main`) and uploads everything to the numbered release. It writes a manifest with matching asset
URLs for the rolling Nightly preview:

```json
{
  "version": "0.1.7",
  "notes": "0.1.7\n\nNew\n- …\n\nFixed\n- …\n\n0.1.6\n\n…",
  "pub_date": "2026-11-12T08:00:00.000Z",
  "platforms": {
    "darwin-aarch64": { "signature": "…", "url": "https://github.com/…/Tabler.app.tar.gz" },
    "darwin-x86_64": { "signature": "…", "url": "https://github.com/…/Tabler.app.tar.gz" },
    "windows-x86_64-nsis": {
      "signature": "…",
      "url": "https://github.com/…/Tabler_…_x64-setup.exe"
    },
    "windows-x86_64-msi": {
      "signature": "…",
      "url": "https://github.com/…/Tabler_…_x64_en-US.msi"
    },
    "windows-x86_64": { "signature": "…", "url": "https://github.com/…/Tabler_…_x64-setup.exe" }
  }
}
```

- The app looks for `{os}-{arch}-{installer}` first and `{os}-{arch}` second, so an MSI install
  is updated with the MSI and a setup `.exe` install with the setup `.exe`. (The macOS entries
  are also listed as `darwin-*-app`.)
- The script refuses to write the file when a signature was made for another version than the
  release's, which the app would refuse as well (see below).

The nightly release is deleted and created again on every publish, so for a few seconds its
`latest.json` does not exist. A check in that window says "No release information was found …
try again in a few minutes"; the quiet check at launch says nothing.

## One-time setup: the signing key

Updates are signed with a minisign key pair made by the Tauri CLI. The **private** key and its
password live only in the repository secrets (and wherever the owner keeps a backup); the
**public** key is in `src-tauri/tauri.conf.json`. Whoever holds the private key can publish
updates every install will accept, and if it is lost no install can be updated any more, so keep
a backup outside the repository (a password manager).

1. Generate the pair. The command asks for a password and prints the public key:

   ```bash
   pnpm tauri signer generate -w ~/.tauri/tabler-updater.key
   ```

   It writes the private key to `~/.tauri/tabler-updater.key` and the public key to
   `~/.tauri/tabler-updater.key.pub`.

2. Put the private key and its password in the repository secrets (the second command asks for
   the value, so the password stays out of the shell history):

   ```bash
   gh secret set TAURI_SIGNING_PRIVATE_KEY --repo kungfufafa/tabler < ~/.tauri/tabler-updater.key
   ```

   ```bash
   gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --repo kungfufafa/tabler
   ```

3. Put the public key (the one line of `~/.tauri/tabler-updater.key.pub`) in
   `plugins.updater.pubkey` of `src-tauri/tauri.conf.json`, commit and push.

From the next push to `main` the nightly carries updater bundles and `latest.json`. Builds
installed **before** that have no updater in them (or no key): they need one last manual install.

## Builds that do not update themselves

Check for Updates… says so instead of failing:

| Build                                           | Why                                                                                                      |
| ----------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| `pnpm tauri dev` (any debug build)              | Its binary sits in `target/`, not in an installed app.                                                   |
| A CI build without the signing key (a fork)     | It gets no update address (`plugins.updater.endpoints` is empty), so it never follows upstream releases. |
| A build whose `plugins.updater.pubkey` is empty | Nothing it downloads could be verified.                                                                  |

A release build made by hand (`pnpm tauri build`) has the stable address and the public key from
`tauri.conf.json`, so it follows the stable channel like a tagged release. It produces no updater
bundles itself: `bundle.createUpdaterArtifacts` is `false` in `tauri.conf.json`, because signing
them needs the private key, and CI turns it on.

## What protects an update

- **The signature.** The download is verified against the public key built into the app before
  it is installed. A file that is not signed by the release key is discarded.
- **The signed version** (`requireSignedVersion`). `tauri build` records the version in the
  signature. `latest.json` itself is not signed, so without this a tampered `latest.json` could
  announce "version 9.9.9" and point at an old, validly signed build to force a downgrade. With
  it the app refuses a bundle whose signed version differs from the announced one.
- **Newer only.** A release is offered only when its version is greater than the running one.
- **HTTPS only**, to GitHub, with the app's own TLS stack (rustls on aws-lc-rs, the system
  proxy). The production CSP is unchanged: the page makes no request, the backend does.
- **Only through the app's commands.** The page reaches the updater through `update_check`,
  `update_download`, `update_cancel` and `update_install`. The plugin's own commands are not
  granted in `src-tauri/capabilities/default.json`.

## macOS while the build is ad-hoc signed

Until the repository has a Developer ID certificate the macOS build is ad-hoc signed
(`signingIdentity: "-"`). In-app updates work, with two things to know:

- **Gatekeeper.** A `.dmg` downloaded with a browser is quarantined, which is why the first
  install needs the right-click ▸ Open step. An update is downloaded by the app itself and is not
  quarantined, so the updated app opens without that step.
- **Keychain.** macOS ties a Keychain item's "Always Allow" to the code signature of the app that
  was allowed. An ad-hoc signature is different for every build, so after each update macOS asks
  again for every saved password the first time it is used ("Tabler wants to use your
  confidential information…", once per connection). The passwords are not lost. This stops once
  the builds are signed with a Developer ID: the signature then stays the same across updates.

If `/Applications/Tabler.app` is not writable by the user (installed by an administrator), macOS
asks for an administrator password during Install and Restart.

## Windows

- The update runs the downloaded installer in passive mode (a progress window, no questions) and
  the installer starts Tabler again. The app must exit for that, so before the installer starts
  it does what a normal quit does: stops exports and backups, closes sessions, uploads SQLite
  over SSH copies.
- The setup `.exe` installs per user; an MSI install updates through the MSI and may ask for
  elevation (UAC), as the first install did.
- The builds are not code-signed, so SmartScreen may warn about the downloaded installer the way
  it does on the first install.

## Trying an update between two local builds

Two release builds with different versions, a local `latest.json`, and a throwaway key (never the
release key). Everything stays in a scratch folder; the test builds use their own identifier so
they do not touch the real app's data.

```bash
mkdir -p /tmp/tabler-update && cd /tmp/tabler-update
pnpm --dir <repo> tauri signer generate --ci -p test -w /tmp/tabler-update/test.key
```

Write `/tmp/tabler-update/old.json` (and `new.json` with a higher `version`):

```json
{
  "version": "0.1.9001",
  "identifier": "com.mekayastudio.tabler.updatetest",
  "bundle": { "createUpdaterArtifacts": true, "targets": ["app"] },
  "plugins": {
    "updater": {
      "pubkey": "<the line in test.key.pub>",
      "endpoints": ["http://127.0.0.1:8765/latest.json"],
      "dangerousInsecureTransportProtocol": true
    }
  }
}
```

Build both (the second only recompiles the app crate), keeping the first `.app` aside:

```bash
TAURI_SIGNING_PRIVATE_KEY=/tmp/tabler-update/test.key TAURI_SIGNING_PRIVATE_KEY_PASSWORD=test pnpm tauri build --config /tmp/tabler-update/old.json
```

Copy `src-tauri/target/release/bundle/macos/Tabler.app` to `/tmp/tabler-update/installed/`, build
again with `new.json`, then copy the new `Tabler.app.tar.gz` and `.sig` to
`/tmp/tabler-update/serve/` and write its `latest.json`:

```bash
node dev/scripts/updater-manifest.mjs --dir /tmp/tabler-update/serve --version 0.1.9002 --base-url http://127.0.0.1:8765
```

Serve the folder (`python3 -m http.server 8765 --directory /tmp/tabler-update/serve`), open the
installed copy and use Check for Updates…. Afterwards delete the folder, the test key included.

## Where the code is

| Part                                  | File                                                                  |
| ------------------------------------- | --------------------------------------------------------------------- |
| Check, download, install              | `src-tauri/src/updater.rs`                                            |
| Tauri commands                        | `src-tauri/src/commands/update.rs`                                    |
| Menu item, About version              | `src-tauri/src/lib.rs` (`shell`)                                      |
| The commit in the build               | `src-tauri/build.rs`                                                  |
| Public key, stable address            | `src-tauri/tauri.conf.json` (`plugins.updater`)                       |
| Dialog, launch check, preference row  | `src/features/updater/`                                               |
| IPC facade and browser mock           | `src/lib/ipc/index.ts` (`ipc.updater`), `src/lib/ipc/mock/updater.ts` |
| Version and updater config of a build | `dev/scripts/release-config.mjs`                                      |
| What changed in a version             | `dev/scripts/release-notes.mjs`                                       |
| `latest.json`                         | `dev/scripts/updater-manifest.mjs`                                    |
| CI                                    | `.github/workflows/build.yml`                                         |

The browser mock (`pnpm dev:mock`) reports no update. Open it with `?mockUpdate=0.2.0` to see the
dialog with a published update.
