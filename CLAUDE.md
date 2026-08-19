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
10. **`update->DesktopResize` must be set.** FreeRDP calls it without a null
    check when the graphics pipeline sends `ResetGraphics`, which happens on
    every connection. Leaving it unset segfaults the moment frames arrive.
11. **Do not hard-code a codec.** The guest sends RemoteFX Progressive only
    because the container has no GPU. Give a desktop a render node and it may
    negotiate AVC444 instead.
12. **Named volumes go in libpod's `volumes` field, not `mounts`.** `mounts` is
    for OCI mounts whose source is a host path. A volume name put there creates
    successfully and then fails to start with "No such device", so the mistake
    looks like a runtime fault rather than a bad request.
13. **A container that exits 255 with an empty log is the inotify limit.** Every
    desktop runs systemd, which needs several inotify instances, and the default
    is 128 per user. systemd exits before it can log anything. `start_desktop`
    detects this and reports it, so do not replace it with a bare `start`.
14. **GLib aborts when a gschema is missing.** A native run then exits
    immediately with no output at all and nothing to identify the cause. Use
    `just run`, which compiles the schema into `target/glib-schemas`. The
    Flatpak is unaffected, so this breaks development while the packaged build
    looks fine.
15. **The TLS certificate stays in its own volume.** Pinning means nothing if a
    reconfigure regenerates the certificate, and the private key must not sit in
    the home directory where the desktop session could read it.
16. **Run tests with `just`, not bare `cargo`.** The recipes export the FreeRDP
    sysroot variables, without which the build script fails.
17. **Never call bare `podman` in a test.** Inside a toolbox that is the
    container's own podman, with separate storage, and it cannot join the host
    user namespace. Use the `host_podman` helper in the live tests.
18. **Building `cabin-freerdp-sys` needs FreeRDP 3.** If `freerdp-devel` is not
    installed, run `just freerdp-sysroot` and export the variables it prints.
    No root is needed.
19. **The viewer pane owns what it shows.** Do not write to its stack from
    anywhere else. A session thread outlives the moment it is replaced, so a
    dead session would otherwise put its own error over the one that replaced
    it. Every session carries a generation and a stale one returns early.
    `window::open` is the only route into the pane.
20. **The Display Control channel attaches after the connection.** A resize
    asked for before that point cannot be sent, so the session holds it. Never
    record a resize as sent when no session took the command: the widget
    suppresses exact repeats, so one dropped request becomes permanent.
21. **The guest serves one RDP session at a time.** A connection made while the
    previous one tears down is rejected with `ERRINFO_LOGOFF_BY_USER` part way
    through the capabilities exchange, which looks like a client fault.
    `live_session.rs` takes a lock and waits. Separate test binaries still run
    concurrently, so run `live_input.rs` and `live_connect.rs` apart from it.
22. **A desktop is not ready when Podman says it is running.** The RDP port
    binds about two and a half seconds later, so the first connection always
    lands in the gap. The session retries. Retry the case where the guest
    accepts and then hangs up before any frame, not only the refused
    connection. Never retry a rejected password: `cabin-rdp.service` sets the
    credentials before the RDP server starts, so a rejection is real.
23. **A resize makes the framebuffer description lie.** Width updates before
    stride, so a shrink reports the old, larger stride against a smaller
    buffer. That passes every bounds check and reads past the end of the
    allocation. Check the stride against the row before copying. A stride
    longer than the row is the dangerous one.
24. **An `AdwAlertDialog` closes on any response.** So a field it cannot
    accept must be checked as it is typed, with the response disabled, not
    rejected on the click. Rejecting after the fact costs the user the whole
    dialogue. This applies to the desktop name and to a share's location.
25. **Every setting in the settings dialogue needs the container recreated.**
    Podman fixes a container's configuration at create time and Cabin keeps
    that configuration in the labels, so there is no save that keeps the
    desktop running. Do not add a button that implies there is.
26. **The guest image is built here, never pulled.** `guest/` installs to
    `<datadir>/cabin/guest` and ships with the application, because the build
    context has to be readable at run time. Keep `guest/meson.build` in step
    with the files in `guest/`, or the Flatpak builds an image from an
    incomplete tree.
27. **Guest images are marked with a label, and the manager filters on it.**
    `guest/Containerfile` sets `io.github.jaydoubleu.cabin.guest`. The host
    carries plenty of images that belong to other work, so listing everything
    by default turns the manager into a way to delete somebody else's work.
    Pruning is the exception: Podman prunes by danglingness, not by owner, and
    the confirmation has to say so.
28. **The shipped guest recipe is read-only, and that is not a policy.** It
    lives inside the installation, under `/app/share` in the Flatpak. A user
    who wants to change it takes a copy to `<config>/cabin/guest`, and the
    presence of that copy is what makes builds use it. Do not add a setting
    to select between them: presence is the setting, and a second one would
    get out of step with the files.
29. **A custom recipe builds `FROM` the base, never from Fedora.** Repeating
    the base costs 57 seconds and 1.65 GB per image instead of 11 seconds and
    about 30 MB, and it drifts from the base as Cabin updates.
30. **Text from Podman or the filesystem is data, not markup.**
    `AdwPreferencesRow` parses its title and subtitle as Pango markup, so an
    image called `<untagged> abc` or a path holding an ampersand makes GTK
    drop the label and warn. Call `set_use_markup(false)` on any row that
    carries such text.
31. **Do not make a desktop a Toolbx container.** It was considered and it
    cannot work: toolbox runs with `--pid=host`, and systemd as PID 1 needs
    its own PID namespace. DESIGN.md section 19.4 has the full comparison.
    Use `podman exec` to get a shell, which is what the Open a shell dialogue
    shows.
32. **Store interface names for remote access, never addresses.** An address
    changes with the network, and a stored one would leave the desktop unable
    to start after a DHCP lease changed. Names resolve to addresses when the
    container is created. Loopback is always bound as well.

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
