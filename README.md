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

### Reaching a desktop from another device

Turning remote access on asks which networks, not just whether. You can name
the interfaces to listen on, and Cabin publishes the port on those addresses
only. Loopback is always included.

This matters on a laptop. Choosing a VPN interface such as `tailscale0` gives a
desktop that your own devices can reach from anywhere and that nobody in a cafe
can reach. Listening on every network is still offered, and it warns that the
desktop will then accept connections on public Wi-Fi as well.

### Container runtime access

Cabin can share the host's Podman or Docker socket with a desktop, because that
is sometimes what you want. The desktop gets no runtime of its own: containers
it starts run outside the desktop, and outside its sandbox.

Both options warn first, and the warning is accurate. The Podman socket grants
a desktop the power of your user account. The Docker socket grants root on the
host. Running Podman inside the desktop would be the safe alternative, and it
does not work yet, so it is not offered. DESIGN.md section 15.17 records how
far it got.

### The guest image

A desktop runs from an image Cabin builds on your machine, from
`guest/Containerfile`, on top of the official Fedora container base. Nothing is
downloaded from anywhere else, and nothing is published.

Build it from the main menu, under Guest Images, or with `just guest`. The
first build takes several minutes.

The same place lists Cabin's images, and only those: the guest image carries a
label, so your other images are left alone unless you ask to see them. It is
worth visiting after a few rebuilds. Each rebuild leaves the previous image
untagged and still over a gigabyte, and the manager offers to clear them once
you have seen the list.

### Several guest images

Guest Images lists recipes. Cabin ships one, and you can add your own with New
Recipe. Build it, and it appears in the New Desktop dialogue as something to
choose, so you can keep one desktop plain and another with your toolchain in
it.

New desktops start on Cabin's own image until you say otherwise. Any built
image can be made the default from its menu in Guest Images, and the row says
which one it is.

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

The image is built from a Containerfile, which Guest Images shows you. The copy
Cabin ships is read-only, because it lives inside the installation. To change
it, press Customise: Cabin copies the guest files to a folder you own and every
build uses that copy from then on. Add packages, change the desktop, whatever
you need.

Your copy stops following Cabin's updates, and the dialogue tells you when
Cabin has a newer recipe than the one you copied. There is a button to delete
your copy and go back.

### Opening a shell in a desktop

Each desktop's menu has "Open a shell", which gives you the command:

```sh
podman exec -it -u cabin -w /home/cabin <desktop> bash
```

Cabin shows the command rather than running it, because launching a terminal on
your computer would need a permission it deliberately does not ask for.

### Shared files and folders

A desktop can be given host folders and single files. Both are Podman bind
mounts, so read-only is enforced by the kernel and behaves the same however you
connect. You choose where each one appears inside the desktop.

Sharing a single file has one catch worth knowing. The mount follows the file
itself, and most editors replace a file rather than change it, so edits made on
this computer can stop reaching the desktop. Share the folder when you plan to
edit.

The application itself asks for very little. It does **not** request
`--filesystem=home`, and it does **not** request `org.freedesktop.Flatpak`.
Host directories reach a desktop through the file chooser portal: you pick a
directory, and Cabin passes only that path to Podman.

## Licence

GPL-3.0-or-later.
