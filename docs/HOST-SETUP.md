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
flatpak install flathub org.gnome.Platform//51 org.gnome.Sdk//51
flatpak install flathub org.freedesktop.Sdk.Extension.rust-stable//26.08
```

The Flatpak bundles FreeRDP itself, so nothing else is needed to build or run
the packaged application.

### A native build

A native build is faster to iterate on than the Flatpak, and `just test` and
`just lint` use it. It needs Rust 1.85 or later, a C compiler, the GTK 4 and
libadwaita headers, `glib-compile-schemas` for `just run`, and `just`. On
Fedora:

```sh
sudo dnf install rust cargo rustfmt clippy gcc pkgconf-pkg-config \
    gtk4-devel libadwaita-devel glib2-devel just
```

`rustup` works as well as the Fedora Rust packages. CI installs this same list
on Fedora 44, so `.github/workflows/ci.yml` is the tested reference.

### FreeRDP, for a native build only

A native build links FreeRDP 3, which the Flatpak provides but a native build
does not. On Fedora, either install it:

```sh
sudo dnf install freerdp-devel
```

or build a local sysroot, which needs no root at all:

```sh
just freerdp-sysroot
```

It downloads the packages with `dnf`, extracts them under
`~/.local/freerdp-sysroot`, and prints the three variables to export. The
`just` recipes pick that sysroot up automatically when it exists, so
`just build` and `just test` work with no further setup.

The sysroot recipe works on Fedora only, because it uses `dnf`. On another
distribution, install that distribution's FreeRDP 3 development package
instead, for example `freerdp3-dev` on Debian and Ubuntu or `freerdp` on Arch.

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
- **`rofiles-fuse` can fail to mount inside a toolbox.** It needs FUSE, and
  on some file systems, for example a second disk mounted outside the home
  directory, the mount fails. `just flatpak` always passes
  `--disable-rofiles-fuse`, so the build does not depend on it.

### Kernel settings must be changed on the host

`/proc/sys` is read-only inside a toolbox, so `sudo sysctl` there fails with a
permission error even as root. Run it in a host terminal, or through
`flatpak-spawn --host`.

### Running two copies while developing

GTK applications are single instance: a second launch hands off to the first
and exits, with no output, which is confusing when you are trying to read logs.
Set `CABIN_DEV_INSTANCE` to run a second, separate instance:

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

Cabin runs desktops with SELinux labelling intact, as the
`container_engine_t` type (DESIGN.md section 22). The one exception is a
desktop that exposes the host container socket, which cannot work otherwise:
the socket is labelled `user_tmp_t` and a confined container may not open it.
That exception applies only to desktops which asked for a host socket.

A shared folder or file is relabelled with Podman's `z` option by default,
because a Fedora host otherwise denies the desktop every access. **That label
change on the host is permanent.** It stays after the desktop is deleted.
Do not relabel a folder that other services read, such as `~/.ssh`. To put
the default label back, run `restorecon -R <path>` on the host.
