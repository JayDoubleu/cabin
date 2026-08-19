# Cabin

Run full Fedora GNOME desktops in containers, and show them in a resizable
window.

Each desktop behaves like a small virtual machine. It has its own process tree,
its own filesystem, its own network stack, and its own GNOME session. It starts
in under a second, because it is a container and not a virtual machine.

Connect from this computer through the application window, or from a phone,
tablet, or another computer with any standard RDP client.

> **Status: usable, not finished.** A desktop starts, renders in the window,
> and takes keyboard and pointer input. The window can be resized and the guest
> follows. Desktops can be created, configured, shared with folders, reached
> from other devices, and deleted.
>
> Not done: clipboard sharing, which is blocked by a fault in
> `gnome-remote-desktop`, and audio. Section 15.20 of [DESIGN.md](DESIGN.md)
> lists everything outstanding, including the gaps between what this document
> claims and what the code does.

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

## Requirements

Run `just doctor` to check everything below at once. It changes nothing.
[docs/HOST-SETUP.md](docs/HOST-SETUP.md) explains each item and why it is
there.

- Fedora, or another Linux with Podman 5 and Flatpak.
- The Podman user socket:
  ```sh
  systemctl --user enable --now podman.socket
  ```
- The GNOME 50 runtime and SDK, plus the Rust extension:
  ```sh
  flatpak install flathub org.gnome.Platform//50 org.gnome.Sdk//50 \
      org.freedesktop.Sdk.Extension.rust-stable//25.08
  ```

## Build

The repository uses [`just`](https://github.com/casey/just).

```sh
just guest      # build the guest desktop image (about 1.6 GB)
just flatpak    # build and install the application
just flatpak-run
```

The Flatpak bundles FreeRDP, so it needs nothing installed on the host beyond
the runtime. A native build does need FreeRDP 3. If `freerdp-devel` is not
installed, run `just freerdp-sysroot`, which builds a local one without root.

For faster iteration, build natively instead:

```sh
just build
just run
just test
```

## Try the guest on its own

The guest needs no application to be useful. Start one and connect with any RDP
client:

```sh
just guest
just guest-test
just guest-probe     # confirms the RDP protocol really answers
```

Then connect to `127.0.0.1:13389` as user `cabin` with the password you passed
to `just guest-test`, which defaults to `testpass`.

Any client works: GNOME Connections, Remmina, FreeRDP, Windows Remote Desktop,
or the Microsoft Remote Desktop app on iOS and Android.

The certificate is self-signed and generated per container, so the client warns
on first connect.

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
- RDP always runs over TLS, and the credentials are per desktop.
- No container runtime socket is exposed.

Cabin can expose the host Podman or Docker socket to a desktop, because that is
sometimes what you want. Both options warn first, and the warning is accurate:
the Podman socket grants a desktop the power of your user account, and the
Docker socket grants root on the host. The safe alternative, running Podman
inside the desktop, is the recommended setting.

The application itself asks for very little. It does **not** request
`--filesystem=home`, and it does **not** request `org.freedesktop.Flatpak`.
Host directories reach a desktop through the file chooser portal: you pick a
directory, and Cabin passes only that path to Podman.

## Licence

GPL-3.0-or-later.
