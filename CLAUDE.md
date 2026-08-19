# Cabin: working notes

Read [DESIGN.md](DESIGN.md) before changing anything structural. Section 12
records what the milestone 1 prototype proved, including several traps that
cost real time.

## Layout

| Path | Holds |
|---|---|
| `crates/cabin-core` | Domain model. No I/O. Serialises to container labels. |
| `crates/cabin-podman` | libpod REST client over the rootless UNIX socket. |
| `crates/cabin-rdp` | RDP client. No GTK types, so it tests without a display. |
| `crates/cabin-gtk` | The application. Owns the `cabin` binary. |
| `guest/` | The guest desktop image: Containerfile, units, scripts. |
| `build-aux/` | Flatpak manifest, cargo wrapper, RDP probe. |
| `data/` | Desktop entry, metainfo, gschema, icon. |

## Commands

Use `just`. Run `just` alone to list every task.

`just test` must pass before any commit. `just lint` must be clean.

## Rules that are easy to get wrong

1. **Podman holds all desktop state.** Cabin has no database. A desktop's
   configuration lives in container labels under
   `io.github.jaydoubleu.cabin.`. Do not add a state file.
2. **Never infer a GPU driver from the render node number.** On the
   development host `renderD128` is Intel and `renderD129` is NVIDIA, which is
   the reverse of the card numbering. Read the driver from
   `/sys/class/drm/<node>/device/driver`.
3. **`gnome-remote-desktop` requires NLA.** It refuses plain TLS with
   `HYBRID_REQUIRED_BY_SERVER`. Any client work must implement CredSSP.
   `just guest-probe` checks this.
4. **Do not duplicate stock systemd units in the guest.** An early version
   shipped a unit that claimed `org.gnome.RemoteDesktop.Headless`, which is the
   stock unit's bus name, and `grdctl rdp enable` then failed. Configure with
   `grdctl`, then start the stock unit.
5. **Keep `NoNewPrivileges=yes` in guest unit overrides.** Turning it off stops
   seccomp filters installing, because a filter needs that flag or
   `CAP_SYS_ADMIN`, and a rootless container has neither by default.
6. **The Flatpak must not gain `--filesystem=home` or
   `--talk-name=org.freedesktop.Flatpak`.** Mount paths come from the file
   chooser portal. Adding either would defeat the sandbox design.
7. **Regenerate `cargo-sources.json` after any `Cargo.lock` change**, with
   `just flatpak-sources`. The Flatpak build is offline and will fail otherwise.
8. **The client must never advertise `cliprdr`.** `gnome-remote-desktop` mounts
   a FUSE filesystem when it sees that channel, the mount needs `CAP_SYS_ADMIN`
   which a rootless container cannot have, and the daemon calls `abort()` when
   it fails. CLIPRDR is a static channel, so it does not appear in the "Loading
   Dynamic Virtual Channel" log lines. Disable it by name.
9. **Do not add bindgen to `cabin-freerdp-sys`.** It was tried and it silently
   produces opaque structs for everything the client needs. DESIGN.md section
   15.1 has the detail. The C shim exists for this reason.
10. **Building `cabin-freerdp-sys` needs FreeRDP 3.** If `freerdp-devel` is not
    installed, run `just freerdp-sysroot` and export the variables it prints.
    No root is needed.

## Environment notes

Building works from a Fedora toolbox. `flatpak-builder`, `flatpak`, and `bwrap`
are all available inside it, and the GNOME SDK in the user Flatpak installation
is visible through the shared home directory.

Pass `--disable-rofiles-fuse` to `flatpak-builder`. `rofiles-fuse` cannot mount
under `/mnt/data` from inside a toolbox container. The `just flatpak` recipe
already does this.

## Style

The repository follows the global instructions in `~/.claude/CLAUDE.md`:
Simplified Technical English, British spelling in prose, and no em-dashes
anywhere. Code identifiers keep their original spelling.

Comments explain why, not what. Several comments in this tree record a
constraint that was found the hard way. Do not delete those when refactoring.
