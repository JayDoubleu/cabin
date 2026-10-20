# Cabin

Run full Fedora GNOME desktops in containers, and show them in a resizable
window.

Each desktop behaves like a small virtual machine. It has its own process tree,
its own filesystem, its own network stack, and its own GNOME session. It starts
in a few seconds, because it is a rootless Podman container and not a virtual
machine.

Connect from this computer through the application window, or from a phone,
tablet, or another computer with a standard RDP client that has clipboard
sharing turned off. [Connecting from another device](#connecting-from-another-device)
explains why the clipboard matters.

<!-- Screenshots: a later stage adds them here. Suggested set: the sidebar
     with a running desktop, the settings dialogue, and Guest Images. -->
> **Screenshots are coming.**

## Status

**Version 0.1.0. Usable, not finished.** A desktop starts, renders in the
window, and takes keyboard and pointer input. The window can be resized and
the guest follows. Desktops can be created, configured, given shared folders,
reached from other devices, and deleted.

These are not done:

- **Clipboard sharing.** A fault in `gnome-remote-desktop` blocks it. See
  [DESIGN.md section 15.2](DESIGN.md#152-correction-the-fuse-clipboard-crash-is-not-fixed-by-devfuse).
- **Audio.**
- **HiDPI scaling.** On a scaled display the desktop is drawn at the lower
  resolution and scaled up.
- **Password storage in the keyring.** Each desktop has its own generated
  password, but Cabin keeps it in the container's environment, where
  `podman inspect` shows it. The plan is the Secret portal.
- **Containers inside a desktop.** Nested Podman does not work yet, so it is
  not offered. A desktop can use the host's Podman or Docker instead, with a
  clear warning.

Section 15.20 of [DESIGN.md](DESIGN.md) lists everything outstanding.

## How it works

```
Cabin (Flatpak, GTK4)  ──control──►  Podman REST socket
        │
        └──pixels and input──►  RDP over TLS  ──►  container
                                                     systemd
                                                       └─ GNOME Shell (headless mutter)
                                                       └─ gnome-remote-desktop
```

Two channels, kept independent. If Cabin exits, the desktops keep running and
other devices can still reach them.

Cabin keeps no database. A desktop's settings live in its container's labels,
so `podman ps` always shows the truth.

## Install

A Flatpak repository is coming. Until then, build Cabin from source as
described in [Build](#build).

## Requirements

Run `just doctor` to check everything below at once. It changes nothing.
[docs/HOST-SETUP.md](docs/HOST-SETUP.md) explains each item and why it is
there.

### To run

- Linux with Podman 5 and Flatpak. Cabin is developed and tested on Fedora.
- The Podman user socket:
  ```sh
  systemctl --user enable --now podman.socket
  ```
- A raised inotify limit if you want more than a few desktops at once. See
  [Running several desktops at once](#running-several-desktops-at-once).

### To build

- [`just`](https://github.com/casey/just).
- For the Flatpak: `flatpak-builder`, and the GNOME 51 runtime and SDK with
  the Rust extension:
  ```sh
  flatpak install flathub org.gnome.Platform//51 org.gnome.Sdk//51 \
      org.freedesktop.Sdk.Extension.rust-stable//26.08
  ```
- For a native build: Rust 1.85 or later, a C compiler, the GTK 4 and
  libadwaita development packages, and FreeRDP 3. The
  [native build list](docs/HOST-SETUP.md#a-native-build) gives the Fedora
  packages.

## Build

```sh
just flatpak    # build and install the application
just flatpak-run
```

The Flatpak bundles FreeRDP, so it needs nothing installed on the host beyond
the runtime.

On first use, open Guest Images from the main menu and build the guest image.
Cabin builds it on your computer and never pulls it from a registry. The first
build takes several minutes. You can also build it from a terminal with
`just guest`.

For faster iteration, build natively instead:

```sh
just build
just run
just test
```

A native build needs FreeRDP 3. On Fedora, install `freerdp-devel`, or run
`just freerdp-sysroot`, which makes a local copy without root. That recipe
uses `dnf`, so it works on Fedora only. On another distribution, install its
FreeRDP 3 development package, for example `freerdp3-dev` on Debian and
Ubuntu.

## Try the guest on its own

The guest needs no application to be useful. Start one and connect with an RDP
client:

```sh
just guest
just guest-test
just guest-probe     # confirms the RDP protocol really answers
```

Then connect to `127.0.0.1:13389` as user `cabin` with the password you passed
to `just guest-test`, which defaults to `testpass`. Turn clipboard sharing off
in the client first, as the next section explains.

The certificate is self-signed and generated per container, so the client warns
on first connect.

## Connecting from another device

GNOME Connections, Remmina, FreeRDP, Windows Remote Desktop and the Microsoft
Remote Desktop app on iOS and Android can all connect, with one condition:

**Turn clipboard sharing off in the client before you connect.** If a client
offers the clipboard channel, `gnome-remote-desktop` in the guest tries to
mount a FUSE filesystem, which a rootless container cannot do, and the daemon
aborts. Remmina, GNOME Connections and Windows Remote Desktop offer the
clipboard by default. With FreeRDP, pass `-clipboard`. DESIGN.md section 15.2
has the detail.

Each desktop's menu has "Connect from another device". It shows the address,
port, username and password, and the fingerprint of the desktop's certificate
once Cabin has connected to it. Compare that fingerprint with the one your
client shows before you accept the certificate.

## Running several desktops at once

Every desktop runs systemd, which needs several inotify instances, and the
default limit is 128 for your whole user account. After a handful of desktops
the next one fails to start, and the failure is silent unless Cabin names it.

```sh
sudo sysctl -w fs.inotify.max_user_instances=1024
echo fs.inotify.max_user_instances=1024 | sudo tee /etc/sysctl.d/99-cabin.conf
```

[docs/HOST-SETUP.md](docs/HOST-SETUP.md) covers this and the rest of the host
setup.

## Security

Read this before sharing anything into a desktop.

A container shares the host kernel. Cabin is a strong **configuration**
boundary, not a sandbox for hostile code. Use a real virtual machine, such as
GNOME Boxes, when you need to contain something you do not trust.

Defaults are conservative:

- Containers run rootless.
- The RDP port binds to `127.0.0.1`. Reaching a desktop from another device is
  opt-in, per desktop.
- RDP always runs over TLS with NLA, and each desktop has its own generated
  password and certificate. Cabin pins the certificate on first connect.
- No container runtime socket is exposed.

Know these limits:

- **Root in a desktop is your user account.** Rootless Podman maps the guest's
  root to your own user ID, and the desktop user has `sudo` with no password.
  So anything in a desktop can change a shared folder that is writable, as
  you. Share read-only where you can.
- **The password is visible to your other programs.** Any program that runs
  as you can read it with `podman inspect`.

### Reaching a desktop from another device

Turning remote access on asks which networks, not just whether. You can name
the interfaces to listen on, and Cabin publishes the port on the IPv4 and
unique-local IPv6 addresses of those interfaces only. Loopback is always
included. Cabin stores the names, not the addresses, and looks the addresses
up again each time the desktop starts.

This matters on a laptop. Choosing a VPN interface such as `tailscale0` gives a
desktop that your own devices can reach from anywhere and that nobody in a cafe
can reach. Chosen networks never use a global IPv6 address, because that can
be reachable from the whole internet. Listening on every network is still
offered, and it warns that the desktop will then accept connections on public
Wi-Fi as well, on IPv4 and IPv6.

### Container runtime access

Cabin can share the host's Podman or Docker socket with a desktop, because that
is sometimes what you want. The desktop gets no runtime of its own: containers
it starts run outside the desktop, and outside its sandbox.

Both options warn first, and the warning is accurate. The Podman socket grants
a desktop the power of your user account. The Docker socket grants root on the
host. Running Podman inside the desktop would be the safe alternative, and it
does not work yet, so it is not offered. DESIGN.md section 15.17 records how
far it got.

### Shared files and folders

A desktop can be given host folders and single files. Both are Podman bind
mounts, so read-only is enforced by the kernel and behaves the same however you
connect. You choose where each one appears inside the desktop.

**Relabelling changes your files' SELinux label for good.** Fedora denies a
container access to your files unless they are relabelled, so the relabel
switch is on by default. The new label stays after the desktop is deleted.
Do not relabel a folder that other programs read, such as `~/.ssh`. Cabin
warns when you pick one. To restore the default label, run
`restorecon -R <path>`.

Sharing a single file has one catch worth knowing. The mount follows the file
itself, and most editors replace a file rather than change it, so edits made on
this computer can stop reaching the desktop. Share the folder when you plan to
edit.

The application itself asks for very little. It does **not** request
`--filesystem=home`, and it does **not** request `org.freedesktop.Flatpak`.
Host directories reach a desktop through the file chooser portal. You pick a
directory, Cabin asks the portal for its real path on the host, and passes only
that path to Podman. If the portal cannot give a real path, Cabin refuses the
share.

### Deleting a desktop

Delete removes the container, the desktop's home volume and its state volume,
which holds the certificate. Nothing saved inside the desktop survives, and
Cabin asks first. Changing a setting is different: Cabin recreates the
container and keeps both volumes, so your files survive. If Podman refuses the
new settings, the old desktop stays as it was.

## Using Cabin

### The guest image

A desktop runs from an image Cabin builds on your machine, from
`guest/Containerfile`, on top of the official Fedora container base. The build
also fetches the Flathub repository file from `dl.flathub.org`, so that the
Software store in each desktop can install applications from Flathub. Nothing
else is downloaded, and nothing is published.

Build it from the main menu, under Guest Images, or with `just guest`. The
first build takes several minutes.

The same place lists Cabin's images, and only those: the guest image carries a
label, so your other images are left alone unless you ask to see them. It is
worth visiting after a few rebuilds. Each rebuild leaves the previous image
untagged and still over a gigabyte, and the manager offers to clear them once
you have seen the list.

### Several guest images

Guest Images lists recipes. Cabin's own is always first, and you can add your
own with New Recipe. Build one and it appears in the New Desktop dialogue as
something to choose, so you can keep one desktop plain and another with your
toolchain in it.

New desktops start on Cabin's own image until you say otherwise. The star on a
recipe row makes that recipe's image the one new desktops start on. It lights
up on the one in use, and it waits until the image is built.

The New Desktop dialogue offers every recipe you have, built or not. One that
is not built says so and offers to build it, rather than leaving you to work
out why it is missing.

A new recipe starts one of two ways:

| Start from | Builds in | Follows Cabin | Good for |
|---|---|---|---|
| The Cabin guest image | seconds | yes | adding software |
| A full copy of the recipe | minutes | no | changing the desktop itself |

Either way the recipe explains itself. Every section is marked `[REQUIRED]`,
with the reason and how it fails if you remove it, or `[YOURS]` for what is
safe to change. So you can edit it without guessing which line is holding the
desktop up.

### Changing what goes into a desktop

The image is built from a Containerfile, which Guest Images shows you. The
recipe Cabin ships is read-only, because it lives inside the installation. To
change it, press "Copy to a new recipe": you get every line of it under a name
you choose, and you can edit it freely. Add packages, change the desktop,
whatever you need.

The recipe Cabin ships never goes away. It stays in the list beside yours and
stays offered when you make a desktop, so a copy that stops building is never
the end of it.

Your copy stops following Cabin's updates, and the dialogue tells you when
Cabin has a newer recipe than the one you copied.

### Applications in a desktop

A desktop comes with Console, Files, Text Editor, Settings and Software in the
dash, and Calculator, Disks, Document Viewer, Image Viewer, Characters, Fonts,
Logs, System Monitor and File Roller in the application grid.

Software is the store, and it installs Flatpaks from Flathub. They go into the
desktop's home, which is a volume, so applications you add survive the desktop
being recreated.

It does not install RPMs on purpose. Those land in the container itself, and
Cabin recreates the container whenever you change a setting, so they would
disappear without warning. Use `sudo dnf install` in the terminal for a one-off,
and put anything you want to keep in your recipe.

### Opening a shell in a desktop

Each desktop's menu has "Open a shell", which gives you the command:

```sh
podman exec -it -u cabin -w /home/cabin <desktop> bash
```

Cabin shows the command rather than running it, because launching a terminal on
your computer would need a permission it deliberately does not ask for.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) first. [DESIGN.md](DESIGN.md) explains
how Cabin works and records why it works that way.

## Reporting a vulnerability

Do not open a public issue. [SECURITY.md](SECURITY.md) says how to report one
privately.

## Licence

GPL-3.0-or-later. See [COPYING](COPYING).
