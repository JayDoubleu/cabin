# Cabin: working notes

Read [DESIGN.md](DESIGN.md) before changing anything structural. Section 12
records what the milestone 1 prototype proved, including several traps that
cost real time.

## Layout

| Path | Holds |
|---|---|
| `crates/cabin-core` | Domain model. No I/O. Serialises to container labels. |
| `crates/cabin-podman` | libpod REST client over the rootless UNIX socket. |
| `crates/cabin-freerdp-sys` | C shim and FFI bindings to FreeRDP 3. Hand-written, no bindgen (rule 9). |
| `crates/cabin-rdp` | RDP client. No GTK types, so it tests without a display. |
| `crates/cabin-gtk` | The application. Owns the `cabin` binary. |
| `guest/` | The guest desktop image: Containerfile, units, scripts. |
| `build-aux/` | Flatpak manifest, cargo wrapper, RDP probe. |
| `build-aux/screenshots/` | The end-to-end test in a private GNOME Shell. DESIGN.md section 23. |
| `data/` | Desktop entry, metainfo, gschema, icon. |

## Commands

Use `just`. Run `just` alone to list every task.

`just test` must pass before any commit. `just lint` must be clean.

`build-aux/screenshots/e2e.sh` tests the installed Flatpak through real input.
Run it on the host, or through `flatpak-spawn --host` from a toolbox, after
`just flatpak` and `just guest`.

The live tests need the host Podman and the guest image, so CI does not run
them. Run them with `just podman-live`, `just rdp-live` and
`just guest-image-live`. Do not run `cargo test -- --ignored` across a whole
crate: that also runs `make_demo`, which replaces any container called
`demo`, and `live_images`, which rebuilds the guest image.

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
    the home volume, which the user shares and backs up. The key is not hidden
    from the session: `gnome-remote-desktop` runs as the session user and must
    read it, so that user owns it.
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
23. **A resize makes the framebuffer description lie.** FreeRDP's `drdynvc`
    worker thread ran `ResetGraphics`, which reallocates the primary buffer,
    while the session thread copied it. A shrink then reported the new width
    with the old, larger stride. That passes every bounds check and reads past
    the end of the allocation. Keep `FreeRDP_SynchronousDynamicChannels` set,
    which removes the race. Keep the copy inside the shim under the update
    lock. Keep the check that the stride is exactly width * 4: FreeRDP's GDI
    does not pad rows, and a stride longer than the row is the dangerous one.
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
28. **The shipped guest recipe is read-only, and it never disappears.** It
    lives inside the installation, under `/app/share` in the Flatpak. A user
    who wants to change it takes a copy, and that copy is a named recipe with
    its own tag under `<config>/cabin/images/`. An earlier version let one
    copy take the shipped recipe's own tag, which hid the shipped recipe: a
    user whose copy stopped building then had no way back, and the shipped
    image vanished from the New Desktop dropdown. Never let two recipes build
    one tag.
29. **A custom recipe builds `FROM` the base, never from Fedora.** Repeating
    the base costs 57 seconds and 1.65 GB per image instead of 11 seconds and
    about 30 MB, and it drifts from the base as Cabin updates.
30. **Keep the `[REQUIRED]` and `[YOURS]` notes in `guest/Containerfile`
    accurate.** Users edit copies of that file, and the notes are the only
    thing telling them which line holds the session up. If you add something
    load-bearing, mark it and say how it fails. The label is the one that
    fails quietly: the image builds and then appears nowhere in Cabin.
31. **Offer every recipe, built or not.** The New Desktop dropdown lists
    recipes, not images. Listing only built images leaves a fresh
    installation with an empty dropdown, and hides a recipe the user has
    just made. An unbuilt choice says so and offers to build that recipe.
32. **A name must fit a dropdown.** `AdwComboRow` shows the selected item on
    the title's line, which leaves a few characters in a dialogue, so set
    `use-subtitle` for anything longer than a word. Keep qualifiers such as
    "your copy" or "default" out of the name and put them beside the row.
33. **Text from Podman or the filesystem is data, not markup.**
    `AdwPreferencesRow` parses its title and subtitle as Pango markup, so an
    image called `<untagged> abc` or a path holding an ampersand makes GTK
    drop the label and warn. Call `set_use_markup(false)` on any row that
    carries such text.
34. **Do not make a desktop a Toolbx container.** It was considered and it
    cannot work: toolbox runs with `--pid=host`, and systemd as PID 1 needs
    its own PID namespace. DESIGN.md section 19.4 has the full comparison.
    Use `podman exec` to get a shell, which is what the Open a shell dialogue
    shows.
35. **A desktop runs as `container_engine_t`, not the default
    `container_t`.** Fedora decodes every image through glycin, which runs
    each decode in a `bwrap` sandbox that mounts its own `devpts`.
    `container_t` may not, so every decode fails and the session shows blank
    squares in place of icons. Text still draws, so it reads as a theme fault
    rather than a policy one, and no AVC is logged because the rule is
    `dontaudit`. Do not "fix" this with `label=disable`: the type keeps the
    container confined and keeps its MCS categories, and disabling labelling
    gives up both. The label needs `selinux_opts` **and** the
    `io.podman.annotations.label` annotation. The option alone is accepted and
    then discarded, because the systemd integration rewrites the type and a
    container running `/sbin/init` always gets that integration. Nothing
    errors, so only the label on a real container shows it.
36. **polkit needs a drop-in, or the desktop shows no applications.** Its
    unit hardens `polkitd` with directives that need `CAP_SYS_ADMIN` to apply
    alongside `User=`, which a rootless container lacks. Nothing says so: the
    shell asks malcontent whether an application may be shown, malcontent
    asks accountsservice, accountsservice asks polkit, and a dead polkit
    filters out every application while the desktop files stay perfect. Keep
    `NoNewPrivileges=yes` in that drop-in. malcontent cannot be removed
    instead, because `gnome-shell` requires it.
37. **The guest's home is a volume, so the build cannot seed it.** Anything
    written to `/home/cabin` during a build is hidden at run time. The
    Flathub repository is therefore fetched at build time and added to the
    session by `cabin-flathub.service`. A system wide repository is not a
    substitute: installing into one needs polkit, which cannot start here.
38. **Install the fonts GNOME asks for by name, not just "a monospace
    font".** `org.gnome.desktop.interface` names Adwaita Sans and Adwaita
    Mono, Fedora ships those as weak dependencies, and the recipe turns weak
    dependencies off. A desktop asking for a face it does not have lays every
    glyph on a grid its fallback does not fit, so text comes out spaced
    apart, and nothing reports why. `adwaita-fonts-all` is the package.
    Checking `fc-match monospace` does not test this: it answered "Noto Sans
    Mono" while the terminal was still broken. Match the configured name
    against what fontconfig returns. Note the variable Noto Sans Mono is not
    tagged fixed pitch, here or on a Fedora desktop.
39. **Keep the dash favourites to applications the recipe installs.**
    Fedora's default names Firefox, Calendar and Calculator, none of which
    are installed. GNOME drops an entry that names a missing application
    without saying so, so the dash came up empty and the desktop looked
    broken. `95-cabin-favourites.gschema.override` sets the list.
40. **Store interface names for remote access, never addresses.** An address
    changes with the network, and a stored one would leave the desktop unable
    to start after a DHCP lease changed. Names resolve to addresses when the
    container is created, and again before each start or restart. If they
    changed, the container is recreated first. Loopback is always bound as
    well.
41. **Delete removes the volumes. Recreate must keep them.** `delete_desktop`
    removes the container and then `cabin-<name>-home` and
    `cabin-<name>-state`, or a new desktop with the same name takes over the
    old files and certificate. A recreate keeps both volumes, and keeps the
    old container under `<name>-cabin-old` until the new one exists. The old
    container is the only copy of the configuration and the password, so a
    recreate that removes it first loses the desktop when Podman refuses the
    new configuration. Use `recreate_desktop` and `delete_desktop`, never a
    bare `remove` followed by a create.
42. **Chosen networks never publishes on a global IPv6 address.** It uses the
    IPv4 and unique-local (`fc00::/7`) addresses of the named interfaces. A
    global address can be reachable from the whole internet, which would make
    chosen networks as wide as every network. Skip link-local, temporary and
    deprecated addresses too. DESIGN.md section 7.5 has the reasons.
43. **Never give Podman a document portal path.** In the Flatpak the file
    chooser returns a path under `/run/user/<uid>/doc/`, and the desktop's
    user cannot read it. Turn it back into the real host path, or refuse the
    share. Do not widen the sandbox instead (rule 6).
44. **The display inhibits the host shortcuts while it has the focus.**
    Without that, Super opens the overview of the host and of the guest
    together, and Alt+Tab leaves the window. Restore them when the focus
    leaves, so the sidebar and the dialogues keep the usual shortcuts.
45. **A desktop keeps the time zone of the host.** The create request sets
    `timezone` to `local`. Without it the guest runs in UTC and its clock
    shows another hour.
46. **A desktop's start time, not only its state, shows a restart.** Podman
    can send a stop and the next start in one batch of events, so every list
    says "running". After a refused recreate the viewer then stayed on
    "disconnected" while the old desktop ran again. `started_again` in
    `viewer.rs` compares `started_at`.
47. **The shim's FreeRDP context starts with `rdpClientContext`.** A bare
    `rdpContext` works, but FreeRDP then logs "[APPLICATION BUG]" on every
    connection, and each client helper that needs the larger struct refuses
    to run.
48. **The end-to-end test must not see the user's desktops.** At start-up
    Cabin opens the first running desktop, and the guest serves one session
    at a time, so a test would take over a session in use.
    `podman-proxy.py` hides every desktop whose name does not start with
    `e2e-` or `shot-`. Name every test desktop that way.

## Environment notes

Building works from a Fedora toolbox. `flatpak-builder`, `flatpak`, and `bwrap`
are all available inside it, and the GNOME SDK in the user Flatpak installation
is visible through the shared home directory.

Pass `--disable-rofiles-fuse` to `flatpak-builder`. `rofiles-fuse` can fail to
mount inside a toolbox container, for example when the checkout is on a second
disk outside the home directory. The `just flatpak` recipe already does this.

## Style

Write prose, comments and commit messages in Simplified Technical English,
with British spelling. Do not use em-dashes anywhere. Code identifiers keep
their original spelling.

Comments explain why, not what. Several comments in this tree record a
constraint that was found the hard way. Do not delete those when refactoring.
