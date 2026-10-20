# Contributing to Cabin

Thank you for your interest. This file says how to build, test and change
Cabin. It is short on purpose: the detail lives in two other files, and this
file points to them rather than copying them.

## Read these first

- [DESIGN.md](DESIGN.md) explains how Cabin works and records why. Read it
  before you change anything structural. Sections 12 and later are the record
  of faults that were found the hard way. Many of them look like ordinary
  bugs and are not.
- [CLAUDE.md](CLAUDE.md) holds the numbered list of rules that are easy to get
  wrong. It is written for human and AI contributors alike. Every rule there
  applies to your change.

## Set up

[docs/HOST-SETUP.md](docs/HOST-SETUP.md) lists what the host needs, including
the packages for a native build. Run `just doctor` to check the host. It
changes nothing.

## Use `just`

Run `just` alone to list every task. Use the recipes, not bare `cargo`. The
recipes export the FreeRDP sysroot variables, and without them the build
script fails (CLAUDE.md rule 16).

Before you send a change, both of these must pass:

```sh
just lint
just test
```

The live tests need the host Podman and the guest image. CI cannot run them,
so run the ones that touch your change by hand:

```sh
just podman-live
just rdp-live
just guest-image-live    # rebuilds the guest image, takes several minutes
```

## Rules to know before your first change

CLAUDE.md has the full list. These catch new contributors most often:

- **Rule 7.** After any change to `Cargo.lock`, run `just flatpak-sources` and
  commit the new `cargo-sources.json`. The Flatpak build is offline. CI
  regenerates the file and fails if it differs from the one in the commit.
  The recipe needs `uv` (`sudo dnf install uv` on Fedora).
- **Rule 9.** Do not add bindgen to `cabin-freerdp-sys`. The C shim exists for
  a reason.
- **Rule 17.** Never call bare `podman` in a test. Inside a toolbox that is the
  wrong Podman. Use the `host_podman` helper in the live tests.
- **Rule 18.** Building `cabin-freerdp-sys` needs FreeRDP 3. On Fedora,
  `just freerdp-sysroot` gets it without root.
- **Rule 26.** The guest image is built from `guest/`, never pulled. If you add
  a file to `guest/`, add it to `guest/meson.build` too.

## Style

- **Comments explain why, not what.** Many comments in this tree record a
  constraint that was found the hard way. Do not delete them when you
  refactor.
- Write prose, comments and commit messages in plain English with British
  spelling. Code identifiers keep their own spelling.
- Do not use em-dashes.
- Write commit messages in the imperative, for example "Keep the home volume
  when a recreate fails".

## What CI checks

`.github/workflows/ci.yml` runs on every pull request:

- `just lint` and `just test` on Fedora 44.

  CI uses the Rust that Fedora packages, and ignores `rust-toolchain.toml`.
  If you use `rustup`, run `rustup update` before `just lint`. A newer clippy
  can add a lint, and CI then fails on code that passed on your computer.
- That `cargo-sources.json` matches `Cargo.lock`.
- `desktop-file-validate` and `appstreamcli validate` on the generated data
  files.
- ShellCheck and ruff on the release scripts, and that the version agrees
  in every place.
- A Flatpak test build in the GNOME 51 CI image, with the release build script.

## Releases

Cabin has its own signed Flatpak repository on GitHub Pages, at
<https://jaydoubleu.github.io/cabin>. A tag `vX.Y.Z` starts
`.github/workflows/release.yml`. The workflow does these steps:

1. It builds a signed repository that continues the live one, the site, the
   bundle and the release notes.
2. It deploys the site to GitHub Pages and makes the GitHub release.
3. It installs Cabin from the live `.flatpakref` in a clean container.

The scripts are in `build-aux/release/`. The CI workflow runs the same build
script, unsigned, on every pull request.

### One-time setup

The owner does these steps once, before the first release:

1. Make the repository public. Users cannot reach a private Pages site.
2. In Settings > Pages, set the source to "GitHub Actions".
3. In Settings > Environments > github-pages, keep `main` and add the tag rule
   `v*` under "Deployment branches and tags". Without the tag rule, the deploy
   of a tag fails.
4. Add the private signing key as the Actions secret `GPG_PRIVATE_KEY`:

   ```sh
   gh secret set GPG_PRIVATE_KEY < cabin-signing-key.private.asc
   ```

5. Turn on private vulnerability reporting, which SECURITY.md depends on. Use
   Settings > Code security, or
   `gh api -X PUT repos/JayDoubleu/cabin/private-vulnerability-reporting`.

### Make a release

The version is in four places: `Cargo.toml`, `meson.build`, `Cargo.lock` and
the newest release entry in the metainfo. They must agree. Do not edit them by
hand. Use `build-aux/release/release.py`, which changes all four.

1. Set the version and the release notes. The notes go into the metainfo
   file, so GNOME Software shows them. One note becomes a paragraph, more notes
   become a list:

   ```sh
   python3 build-aux/release/release.py bump 0.2.0 \
       --note "Audio in the guest." --note "Clipboard sharing."
   ```

   For 0.1.0, the version and the notes are already there. Set only the date
   of the entry to the day of the tag:

   ```sh
   python3 build-aux/release/release.py date 0.1.0
   ```

2. Run `just lint` and `just test`, commit the change, and merge it to `main`.
3. Tag the commit and push the tag:

   ```sh
   git tag v0.2.0
   git push origin v0.2.0
   ```

The workflow fails if the tag does not match the version, or if the newest
metainfo entry is a different version. `release.py check v0.2.0` does the same
check on your computer.

Each release continues the live repository. The new commit has the previous
release as its parent, so `flatpak update` downloads a static delta. The
repository keeps the last 3 commits.

A manual run of the workflow (Actions > Release > Run workflow) builds and
signs. It deploys to Pages only if you set "publish", and it never makes a
GitHub release.

### Test a release on your computer

Run the scripts in the CI image with Podman. `in-container.sh` uses the host
Podman from a toolbox. Use a separate Flatpak user folder for the test install,
so the test does not touch your own apps:

```sh
export PAGES_URL=http://localhost:8790 GPG_PRIVATE_KEY_FILE=~/cabin-signing-key.private.asc
build-aux/release/in-container.sh build-aux/release/build-repo.sh _release/repo --no-mirror
build-aux/release/in-container.sh build-aux/release/stage-site.sh _release/repo _release/site
build-aux/release/in-container.sh build-aux/release/package-assets.sh _release/repo _release/assets
build-aux/release/in-container.sh build-aux/release/verify-repo.sh _release/repo
python3 -m http.server 8790 --bind 127.0.0.1 --directory _release/site &
CONTAINER_ARGS="--network=host --env FLATPAK_USER_DIR=/tmp/flatpak-user" \
    build-aux/release/in-container.sh build-aux/release/verify-install.sh 0.1.0
```

Without the private key, give `build-repo.sh` the `--unsigned` option. The
first build takes about as long as `just flatpak`. Everything goes into
`_release/`, which git ignores.

### Signing key

The workflow reads the private key from the `GPG_PRIVATE_KEY` Actions secret.
The public key is `build-aux/release/signing-key.asc`, ID `30D419BA26EDF4F1`.
Keep an offline copy of the private key and of its revocation certificate. If
you lose the private key, you must make a new key. Then each user must import
the new key:

```sh
flatpak remote-modify --user --gpg-import=signing-key.asc cabin
```

## Security issues

Do not report a vulnerability in a public issue or pull request. Follow
[SECURITY.md](SECURITY.md).
