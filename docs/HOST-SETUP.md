# Host setup

Everything Cabin needs on the host, and why each item is here. Run
`just doctor` to check all of it at once. That command changes nothing and
never asks for a password.

Every entry below exists because something actually went wrong, not because it
seemed prudent.

---

## Required

### The Podman user socket

Cabin manages desktops through the rootless Podman REST API. Without the socket
the application starts, reports the problem, and can do nothing else.

```sh
systemctl --user enable --now podman.socket
```

### The inotify instance limit

**This is the one that will catch you.** Every desktop runs systemd, and
systemd needs several inotify instances. Fedora allows 128 for your whole user
account, so after a handful of desktops the next one fails.

The failure gives you nothing to work with. systemd cannot allocate its manager
object, so it exits before it can log anything:

```
Failed to create control group inotify object: Too many open files
Failed to allocate manager object: Too many open files
```

The container dies with status 255 and an **empty log**, and Podman reports the
start as successful, because handing the container to the runtime did succeed.
Nothing anywhere points at inotify. Cabin now detects this and says so, but the
fix needs root:

```sh
sudo sysctl -w fs.inotify.max_user_instances=1024
```

That lasts until you restart. To keep it:

```sh
echo fs.inotify.max_user_instances=1024 | sudo tee /etc/sysctl.d/99-cabin.conf
```

Note that stopped containers still hold their instances until they are removed,
so `podman rm` reclaims more than `podman stop`.

### The guest image

```sh
just guest
```

About 1.6 GB. It holds Fedora, GNOME, and `gnome-remote-desktop`.

---

## Required to build

### The Flatpak runtime

```sh
flatpak install flathub org.gnome.Platform//50 org.gnome.Sdk//50
flatpak install flathub org.freedesktop.Sdk.Extension.rust-stable//25.08
```

The Flatpak bundles FreeRDP itself, so nothing else is needed to build or run
the packaged application.

### FreeRDP, for a native build only

`cargo build` links FreeRDP 3, which the Flatpak provides but a native build
does not. Either install it:

```sh
sudo dnf install freerdp-devel
```

or build a local sysroot, which needs no root at all:

```sh
just freerdp-sysroot
```

It downloads the packages, extracts them under `~/.local/freerdp-sysroot`, and
prints the three variables to export. The `just` recipes pick that sysroot up
automatically when it exists, so `just build` and `just test` work with no
further setup.

---

## Worth knowing

### Building from a toolbox works

`flatpak-builder`, `flatpak` and `bwrap` all work inside a Fedora toolbox, and
the GNOME SDK in the user Flatpak installation is visible through the shared
home directory.

Two things differ inside a toolbox, and `just` handles both:

- **`podman` inside a toolbox is not the host's.** It has its own storage, so a
  desktop started there would be invisible to the host and would run nested.
  The recipes detect the toolbox and reach the host with `flatpak-spawn`.
- **`rofiles-fuse` cannot mount under `/mnt/data`.** `just flatpak` passes
  `--disable-rofiles-fuse` for this reason.

### Kernel settings must be changed on the host

`/proc/sys` is read-only inside a toolbox, so `sudo sysctl` there fails with a
permission error even as root. Run it in a host terminal, or through
`flatpak-spawn --host`.

### Running two copies while developing

GTK applications are single instance: a second launch hands off to the first
and exits, with no output, which is confusing when you are trying to read logs.

```sh
CABIN_DEV_INSTANCE=1 just run
```

### The settings schema, for a native run

GLib aborts when an application's schema is missing, so a native run of a GNOME
application exits immediately and silently unless the schema is installed or
`GSETTINGS_SCHEMA_DIR` points at a compiled copy. `just run` compiles one into
`target/glib-schemas` and points at it, so this needs no action. The Flatpak
installs the schema properly and is unaffected.

### SELinux

Cabin runs desktops with SELinux labelling intact. The one exception is a
desktop that exposes the host container socket, which cannot work otherwise:
the socket is labelled `user_tmp_t` and a confined container may not open it.
That exception applies only to desktops which asked for a host socket.
