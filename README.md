# Cabin

Run full Fedora GNOME desktops in containers, and show them in a resizable
window.

Each desktop behaves like a small virtual machine. It has its own process tree,
its own filesystem, its own network stack, and its own GNOME session. It starts
in under a second, because it is a container and not a virtual machine.

Connect from this computer through the application window, or from a phone,
tablet, or another computer with any standard RDP client.

> **Status: early.** The guest image works and serves a desktop over RDP. The
> application is a scaffold: the window, the packaging, and the domain model are
> in place, and the viewer does not yet draw guest frames. See
> [DESIGN.md](DESIGN.md) for the plan and the milestones.

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
