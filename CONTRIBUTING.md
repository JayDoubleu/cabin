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
- That `cargo-sources.json` matches `Cargo.lock`.
- `desktop-file-validate` and `appstreamcli validate` on the generated data
  files.
- A Flatpak test build in the GNOME 51 CI image.

## Security issues

Do not report a vulnerability in a public issue or pull request. Follow
[SECURITY.md](SECURITY.md).
