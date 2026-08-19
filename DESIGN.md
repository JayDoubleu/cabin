# Cabin: design

Cabin runs full Fedora GNOME desktops inside containers and shows them in a
desktop application. Each desktop behaves like a small virtual machine. It is
not a virtual machine.

Application ID: `io.github.jaydoubleu.Cabin`

---

## 1. Goals

1. Run several isolated Fedora GNOME desktops on one Fedora host.
2. Show a desktop in a resizable window. The guest resolution follows the
   window size.
3. Offer buttons for common fixed resolutions.
4. Connect to the same desktop from other devices with a standard RDP client.
5. Manage the desktops from a sidebar: create, start, stop, delete, configure.
6. Configure host directory mounts as read-write or read-only.
7. Optionally expose a container runtime socket to a desktop.
8. Start in about one second, not in about one minute.

### Non-goals

1. Cabin does not emulate hardware. There is no BIOS, no virtual disk, and no
   guest kernel.
2. Cabin does not run other operating systems. The guest shares the host
   kernel, so the guest must be Linux.
3. Cabin does not replace GNOME Boxes. Use Boxes when you need a real virtual
   machine.

---

## 2. What a "pseudo-VM" is here

A Cabin desktop is one long-running Podman container. The container has:

| Property | Provided by |
|---|---|
| Own process tree | PID namespace |
| Own filesystem | Container image plus a writable layer |
| Own network stack | Network namespace, `pasta` or `slirp4netns` |
| Own hostname, users, services | UTS namespace, systemd as PID 1 |
| Own desktop session | GNOME Shell on the headless mutter backend |
| Remote access | `gnome-remote-desktop` serving RDP |

The container shares the host kernel. This is the whole trade. Boot is fast and
memory use is low, but a kernel exploit escapes to the host. Section 8 covers
the security consequences.

---

## 3. Architecture

```
  HOST                                         GUEST CONTAINER
  ┌────────────────────────────────────┐       ┌──────────────────────────────┐
  │  Cabin (Flatpak, GTK4/libadwaita)  │       │  systemd (PID 1)             │
  │                                    │       │    │                         │
  │  ┌──────────┐  ┌────────────────┐  │       │    ├─ cabin-firstboot        │
  │  │ Sidebar  │  │  RDP viewer    │  │       │    │    TLS cert, password   │
  │  │          │  │  (IronRDP →    │  │       │    │                         │
  │  │ desktop1 │  │   GdkTexture)  │  │       │    └─ systemd --user (cabin) │
  │  │ desktop2 │  │                │  │       │         │                    │
  │  │ [ + new ]│  │                │  │       │         ├─ cabin-keyring     │
  │  └──────────┘  └────────────────┘  │       │         │                    │
  │        │               │           │       │         ├─ cabin-shell       │
  │        │               │           │       │         │   gnome-shell      │
  │        │               │           │       │         │     --headless     │
  │        │               │           │       │         │     --virtual-     │
  │        │               │           │       │         │       monitor WxH  │
  │        │               │           │       │         │                    │
  │        │               │           │       │         └─ cabin-remote-     │
  │        │               │           │       │              desktop         │
  │        │               │           │       │              (gnome-remote-  │
  │        │               │           │       │               desktop)       │
  └────────┼───────────────┼───────────┘       └──────────────┼───────────────┘
           │               │                                  │
           │ HTTP          │ RDP over TLS ────────────────────┘
           │ (unix socket) │ 127.0.0.1:<port>
           ▼               │
  ┌────────────────────┐   │                   OTHER DEVICES
  │ podman.socket      │   │                   ┌──────────────────────────┐
  │ (rootless, user)   │   └───────────────────│ mstsc / FreeRDP / iOS RD │
  └────────────────────┘                       └──────────────────────────┘
```

Two channels connect the application to a desktop:

1. **Control.** The Podman REST API over the rootless user socket. Cabin uses
   this to create, start, stop, inspect, and delete containers.
2. **Pixels and input.** RDP over TLS to a port that the container publishes on
   the loopback interface.

The two channels are independent. If Cabin exits, the desktops keep running,
and a phone can still connect to them over RDP.

---

## 4. Guest image

### 4.1 Session bring-up

The container runs `/sbin/init`. systemd starts a user session for the `cabin`
user, which lingers so that the session survives with no login.

The session brings up three units in order:

1. `cabin-keyring.service` unlocks the login keyring.
   `gnome-remote-desktop` stores its RDP credentials through libsecret, so the
   keyring must be open before those credentials can be written.
2. `cabin-shell.service` runs
   `gnome-shell --headless --virtual-monitor ${WIDTH}x${HEIGHT}`.
   The headless mutter backend needs no DRM device and no physical monitor.
3. `cabin-remote-desktop.service` configures `grdctl --headless` and then runs
   `gnome-remote-desktop-daemon --headless`, which serves RDP.

`cabin-firstboot.service` runs once before the user session. It reads the
container environment from `/proc/1/environ`, sets the account password,
generates a self-signed TLS certificate, and writes `/run/cabin/env` for the
user units to read.

Reading `/proc/1/environ` is deliberate. systemd does not pass the container
environment down to individual services, so the values from `podman run --env`
would otherwise be lost.

### 4.2 Resolution

`--virtual-monitor` sets only the starting size. RDP carries a Display Control
Virtual Channel. When the client sends a new size, mutter resizes the virtual
monitor and GNOME re-lays out. This gives the automatic fit behaviour that
VMware Workstation has, with no guest agent and no polling.

The fixed-resolution buttons use the same channel. They send a chosen size
instead of the window size.

### 4.3 Graphics

The headless backend renders with OpenGL. Two paths exist:

| Path | Requirement | Notes |
|---|---|---|
| Software (`llvmpipe`) | None | Always works. Enough for terminals, editors, and browsing. Uses CPU. |
| Hardware | A DRM render node passed into the container | Faster, lower CPU use. |

`start-shell.sh` selects software rendering when `/dev/dri/renderD128` is
absent.

This host has two render nodes. The Intel node is the sensible default: it
needs no extra container configuration, while NVIDIA needs the Container Device
Interface and a matching driver inside the guest. Cabin therefore prefers the
first non-NVIDIA node, offers software rendering as an alternative, and lists
every node it finds. Section 7.3 covers the setting.

**Never infer the driver from the node number.** On this host the card numbers
and the render node numbers disagree:

| Device | Driver |
|---|---|
| `card0` | `nvidia` |
| `card1` | `i915` |
| `/dev/dri/renderD128` | `i915` |
| `/dev/dri/renderD129` | `nvidia` |

Cabin reads the driver name from
`/sys/class/drm/<node>/device/driver` for each node. A rule such as "renderD128
is the integrated GPU" would pass the NVIDIA node to every desktop on this
machine.

### 4.4 Image variants

The base image is `cabin-guest:f44`. It holds GNOME plus a small set of core
applications. Users layer their own images on top with a normal
`Containerfile`. A development image, for example, adds toolchains.

---

## 5. Host application

### 5.1 Process model

Cabin is a single GTK4 process. It holds no long-lived privileged state. All
desktop state lives in Podman, which is the single source of truth.

Cabin stores a desktop's configuration in container labels, with the prefix
`io.github.jaydoubleu.cabin.`. Nothing is stored in a separate database.

This choice matters. It means:

1. A desktop that Cabin did not create is still visible, if it carries the
   labels.
2. `podman ps` shows the truth, and Cabin cannot drift from it.
3. Removing Cabin leaves working containers behind.

### 5.2 Crates

| Crate | Responsibility |
|---|---|
| `cabin-core` | Domain model. A `Desktop`, its `Config`, mounts, GPU mode, socket policy. Serialises to and from container labels. No I/O. |
| `cabin-podman` | Podman REST client over the rootless UNIX socket. Create, start, stop, remove, inspect, events, image pull and build. |
| `cabin-rdp` | The RDP client. Wraps IronRDP and exposes a GTK widget. |
| `cabin-gtk` | The application. Windows, sidebar, preferences, actions. Owns the binary. |

The split keeps the RDP work testable without a GTK main loop, and keeps the
Podman client usable from a future command line tool.

### 5.3 User interface

`AdwNavigationSplitView` gives the two-pane layout directly.

```
┌───────────────┬────────────────────────────────────────────────┐
│  Cabin     ⌕  │  fedora-dev            ● Running     ⋮   ⛶     │
├───────────────┼────────────────────────────────────────────────┤
│ ● fedora-dev  │                                                │
│ ○ writing     │                                                │
│ ○ testing     │            (the guest desktop)                 │
│               │                                                │
│               │                                                │
│               │                                                │
│  + New        │                                                │
└───────────────┴────────────────────────────────────────────────┘
```

The header bar of a running desktop carries:

- A state indicator and the desktop name.
- A display menu: **Fit to window**, then 1280x720, 1920x1080, 2560x1440,
  3840x2160, and a custom entry.
- A fullscreen button.
- An overflow menu: Restart, Stop, Settings, Show logs, Delete.

The sidebar row for a stopped desktop shows a Start button on hover. The row
for a running desktop shows CPU and memory use.

### 5.4 Podman client

Cabin talks to `$XDG_RUNTIME_DIR/podman/podman.sock` and uses the libpod API,
not the Docker compatibility API. The libpod API exposes what Cabin needs:
device requests, label filters, and health state.

The socket is a user service. If `podman.socket` is inactive, Cabin explains
this and offers the exact command to enable it.

Container events stream over `/libpod/events`. Cabin subscribes once and
updates the sidebar from that stream, so the list never polls.

---

## 6. The RDP viewer

This is the only part with no ready-made component. It deserves the most care.

### 6.1 Stack choice

> **Superseded by section 14.** IronRDP cannot connect to
> `gnome-remote-desktop`, which requires the MS-RDPEGFX graphics pipeline that
> no published IronRDP crate implements. Read section 14 before acting on this
> section.

Cabin uses **IronRDP**, the pure-Rust RDP client library from Devolutions. It
handles the protocol, the security layer, bitmap codecs, the clipboard channel,
and the Display Control channel.

The alternative was FreeRDP through FFI. It was rejected. GNOME Connections
does ship a GTK widget for FreeRDP, but it lives at
`/usr/lib64/gnome-connections/libgtk-frdp-0.2.so`. It is a private vendored
library with no public API, no `pkg-config` file, and no stability promise.
Building on it would mean vendoring it too, and then maintaining C FFI as well.

### 6.2 Frame path

```
  IronRDP session (tokio task, own thread)
        │  decoded BGRA framebuffer + damage rectangles
        ▼
  async channel  ──►  GTK main thread
                          │
                          │  gdk_memory_texture_new  (or a GL texture upload)
                          ▼
                      GdkTexture
                          │  snapshot
                          ▼
                   CabinDisplay : GtkWidget
```

The RDP session runs on its own thread with a Tokio runtime. It never touches
GTK types. Frames cross to the main thread through a bounded channel, so a slow
UI applies back pressure instead of growing a queue.

`CabinDisplay` is a custom `GtkWidget` that implements `snapshot`. It keeps
aspect ratio and reports its own size so the resize logic can read it.

The first version uploads a whole `GdkMemoryTexture` per frame. That is simple
and correct. If profiling shows it is too slow, the next step is a
`GtkGLArea` with a persistent texture and damage-rectangle sub-uploads. The
widget API does not change, so this stays an internal optimisation.

### 6.3 Input

`CabinDisplay` attaches `GtkEventControllerKey`, `GtkEventControllerMotion`,
`GtkGestureClick`, and `GtkEventControllerScroll`.

Keyboard mapping is the fiddly part. RDP expects PC/AT scancodes. GTK gives
hardware keycodes, which on Linux are evdev codes offset by 8. The mapping is a
static table, and IronRDP provides the scancode types.

Pointer coordinates scale from widget space to guest framebuffer space. The
widget letterboxes, so the transform must account for the offset, not only the
scale.

### 6.4 Resize

Two directions exist and they must not fight each other.

**Fit to window.** The widget's `size-allocate` starts a 300 ms debounce timer.
When the timer expires, Cabin sends a Display Control request for the new size.
Without the debounce, dragging a window edge would send hundreds of requests
and GNOME would re-lay out continuously.

**Fixed resolution.** Cabin sends the chosen size once and stops reacting to
window size. The widget then scales the guest framebuffer to fit, and
letterboxes it.

Fit mode rounds sizes to even numbers. Several RDP codecs need even dimensions.

### 6.5 Clipboard and audio

> **Superseded for the clipboard by section 15.2.** Advertising CLIPRDR crashes
> the guest session, so Cabin deliberately never offers it. What follows was
> the original plan and is kept for when the upstream fault is fixed.

Clipboard uses the CLIPRDR channel, bridged to `GdkClipboard`. Text first,
images second.

Audio uses the RDPSND channel. `gnome-remote-desktop` sources it from PipeWire
in the guest. On the host, Cabin plays it through the Flatpak PulseAudio
socket. Audio is scheduled after the first release.

---

## 7. Feature specifications

### 7.1 Mounts

A desktop can be given host directories and single files. Both are Podman bind
mounts. Nothing goes over the RDP connection, so a share behaves the same
whether the desktop is used from this computer or from a phone, and read-only
is enforced by the kernel rather than by a client honouring a flag.

| Field | Meaning |
|---|---|
| Source | A host path, always from the file chooser portal. |
| Location | Where it appears inside the desktop. Suggested, then editable. |
| Writable | Off by default. |
| Relabel | SELinux `z`. On by default, which Fedora needs. |

The location is editable rather than derived. A suggestion from the source name
is right most of the time and wrong whenever the desktop expects a particular
path, and there is no way to guess which. `Mount::check_guest_path` refuses a
path that Podman would take literally and produce something unusable from: an
empty one, a relative one, `/` itself, one containing a colon, and the guest's
own system directories. Mounting over `/usr` leaves a desktop that cannot
start, and the failure does not point at the setting that caused it.

Sharing a single file is offered separately from sharing a folder, because a
configuration file such as `.gitconfig` should not need its whole directory
shared with it. A file share carries a warning that a folder share does not:
a bind mount of a single file follows the inode, and most editors replace a
file rather than change it, so edits made on the host stop reaching the
desktop with no sign that anything is wrong.

Cabin never browses the filesystem itself. Every source path comes from the
portal, so the application needs no broad filesystem permission of its own.
See section 8.

### 7.2 Container runtime sockets

The setting has three values.

| Value | Effect |
|---|---|
| **None** (default) | No socket is exposed. |
| **Nested** | The guest runs its own rootless Podman. Cabin adds `/dev/fuse` and the needed annotations. |
| **Host socket** | The host Podman or Docker socket is bind-mounted into the guest. |

**Nested is the default recommendation** and the setting dialogue says so.

The **Host socket** option carries a warning that the user must dismiss before
it applies. The wording is plain:

> Exposing the host container socket gives this desktop the same power over
> your computer as the account that runs it. A process in the desktop can start
> a privileged container and read or change any file. Only enable this for
> desktops you fully trust.

For the Docker socket the warning is stronger, because the Docker daemon runs
as root, so the guest gains root on the host.

Both options are offered, as requested. The default is safe, the risk is
stated once and clearly, and the choice is per desktop, not global.

### 7.3 Graphics setting

| Value | Effect |
|---|---|
| **Automatic** (default) | Cabin picks the first non-NVIDIA render node. On this host that is the Intel one. |
| **Software rendering** | No device is passed. `llvmpipe` is used. |
| **A specific render node** | Cabin lists every `/dev/dri/renderD*` with its driver name, so the choice is informed. |

NVIDIA nodes are listed but carry a note that they need the Container Device
Interface and matching guest drivers. Cabin does not silently attempt that.

### 7.4 Creating a desktop

The create dialogue asks for a name, a base image, a memory limit, a CPU limit,
and a home volume size. Everything else takes a default and is changed later in
Settings.

The guest image is built by Cabin itself, on the machine that runs it. Section
19 has the detail. The create dialogue checks for it and offers to build it,
rather than letting the desktop fail with "no such image".

### 7.5 Remote access and network selection

A desktop publishes its RDP port on loopback by default. Remote access is two
questions, not one, and the settings ask them separately.

| Setting | Published on |
|---|---|
| **Off** (default) | `127.0.0.1` only. |
| **Chosen networks** | Loopback, plus every address on the named interfaces. |
| **Every network** | `0.0.0.0`. |

Turning remote access on lands the user on **chosen networks**, because that is
the safer of the two. Widening it to every network is a separate, deliberate
step with its own warning.

**Interface names are stored, not addresses.** An address changes whenever the
network does. A stored address would leave the desktop unable to start after a
DHCP lease changed, and the reason would not be obvious. Cabin resolves the
names to addresses when it creates the container, and an interface that has no
address at that moment simply contributes nothing.

**Loopback is always bound.** Choosing an interface never removes the ability
to connect from the computer the desktop runs on. Offering loopback as a
checkbox would suggest it can be turned off, which it cannot.

This matters because a laptop joins networks it does not trust. On the
development host the interfaces are:

```
docker0      172.17.0.1
tailscale0   100.64.0.1, fd7a:115c:a1e0::1
wlp0s20f3    192.0.2.10
```

Choosing `tailscale0` alone gives a desktop that is reachable from the user's
own devices anywhere in the world and reachable from nobody in a cafe. That
distinction is not expressible with a single on and off switch, which is why
the setting is not one.

Each chosen address becomes its own Podman port mapping. Several mappings may
share one host port as long as their addresses differ.

---

## 8. Security model

State the boundary honestly, because the name "pseudo-VM" invites the wrong
assumption.

1. **A container is not a security boundary against a kernel exploit.** The
   guest shares the host kernel. Treat a Cabin desktop as a strong
   configuration boundary, not as a sandbox for hostile code. Use a real
   virtual machine for that.
2. **Rootless by default.** Containers run as the user, with a user namespace.
   A root process in the guest maps to an unprivileged host user ID.
3. **Loopback only.** The RDP port publishes to `127.0.0.1` by default. Remote
   access from other devices is opt-in per desktop, and the dialogue names the
   interface it will listen on.
4. **TLS always.** RDP runs over TLS with a per-container self-signed
   certificate. Cabin pins the fingerprint on first connect and warns if it
   changes.
5. **Credentials are per desktop.** They are generated, not defaulted, and
   stored in the host keyring through the Secret portal.
6. **The socket settings are the sharp edge.** Section 7.2 covers them.

---

## 9. Build and distribution

### 9.1 Flatpak

| Item | Value |
|---|---|
| Runtime | `org.gnome.Platform//50` |
| SDK | `org.gnome.Sdk//50` |
| Rust | `org.freedesktop.Sdk.Extension.rust-stable//25.08` |
| Build system | Meson, which drives Cargo |
| Bundled | FreeRDP 3.30.0, built from source |

FreeRDP is not in the GNOME runtime, so the manifest builds it. The build turns
off everything Cabin does not use: FreeRDP's own clients, servers and tools,
the X11 and Wayland backends, smartcards, FFmpeg and Cairo.

Two of those flags come from findings elsewhere in this document rather than
from tidiness:

- `WITH_GFX_AV1=OFF`. FreeRDP warns at runtime that AV1 is experimental and
  "might crash the application". Section 15.5 shows the guest sends RemoteFX
  Progressive regardless, because a container with no render node has no
  hardware encoder.
- `WITH_FUSE=OFF`. The clipboard mounts a FUSE filesystem, which Cabin never
  asks for because it crashes the guest daemon. Section 15.2 has the detail.

`WITH_VERBOSE_WINPR_ASSERT` is also off. It defaults on, and FreeRDP warns that
it "might slow down the application", which is not something to ship.

Sandbox permissions:

| Permission | Why |
|---|---|
| `--socket=wayland`, `--socket=fallback-x11` | Display |
| `--device=dri` | Rendering the viewer |
| `--share=network` | RDP to the published port |
| `--socket=pulseaudio` | Guest audio, later |
| `--filesystem=xdg-run/podman:ro` | The Podman REST socket |
| `--talk-name=org.freedesktop.secrets` | Storing desktop credentials |

Cabin does **not** request `--filesystem=home` and does **not** request
`--talk-name=org.freedesktop.Flatpak`. Mount paths come from the file chooser
portal, as section 7.1 explains.

Note that `--filesystem=xdg-run/podman:ro` marks the directory read-only. The
socket inside it is still read-write, which is what a UNIX socket needs. The
flag stops Cabin creating files next to it.

### 9.2 Building

Cabin builds from a Fedora toolbox. `flatpak-builder`, `flatpak`, and `bwrap`
all work inside the container, and the GNOME 50 SDK is installed in the user
Flatpak installation, which the toolbox sees through the shared home directory.

```sh
just build          # native build, fast iteration
just run            # native run
just flatpak        # build and install the Flatpak
just guest          # build the guest image
```

---

## 10. Milestones

| # | Milestone | Proves | State |
|---|---|---|---|
| 1 | Guest image serves a GNOME desktop over RDP | The whole idea works | Done, see section 12 |
| 2 | Flatpak shell with sidebar and stubbed data | Packaging and UI skeleton | Done |
| 3 | Podman client: list, start, stop, event stream | Real state in the sidebar | Done, see section 13 |
| 4 | `CabinDisplay` shows guest frames, read-only | The hardest component | Done, see section 15 |
| 5 | Keyboard and pointer input | Usable desktop | Done, see sections 15.9 to 15.11 |
| 6 | Fit-to-window and fixed resolutions | The headline feature | Done, verified against the guest |
| 7 | Create, delete, mounts, GPU and socket settings | Feature complete | Done |
| 8 | Clipboard, audio, connection profiles for other devices | Polish | Connection profiles done, see section 15.14. Clipboard blocked, see 15.2. Audio not started |

Milestone 1 comes first on purpose. It carries nearly all the technical risk,
and it needs no Rust at all.

---

## 11. Open questions

1. **Does `gnome-shell --headless` start cleanly with no logind seat?** If it
   does not, the fallback is to run the session under `dbus-run-session`
   without a systemd user session. Milestone 1 answers this.
2. **Does the keyring unlock reliably at boot?** If libsecret proves awkward,
   the alternative is `grdctl --system` with a gdm handover, which stores
   credentials in a file instead.
3. **Is a whole-texture upload per frame fast enough at 4K?** Measure before
   optimising. Section 6.2 has the fallback.
4. ~~**How well does IronRDP interoperate with `gnome-remote-desktop`?**~~
   Answered in section 14. CredSSP interoperates. The graphics pipeline does
   not exist in IronRDP, and the guest requires it, so the connection is
   refused. Section 6.1 is superseded.
5. **`polkit.service` does not start in the guest.** Section 12.3 records the
   investigation. GNOME Shell and RDP both work without it. The effect is that
   privileged actions in Settings will refuse. This is open.


---

## 12. Findings from the milestone 1 prototype

The prototype ran on 2026-08-19. It reached a clean automatic boot in **649 ms**
and served RDP with no manual step. These findings are already applied to
`guest/`.

### 12.1 The user manager needs XDG_RUNTIME_DIR set explicitly

`user@.service` failed with "Trying to run as user instance, but
$XDG_RUNTIME_DIR is not set".

`systemd-logind` does run in the container, and it does create
`/run/user/1000` with the right owner. The gap is `pam_systemd`, which does not
export the variable without a real seat.

The fix is a drop-in that sets it directly:

```ini
# /etc/systemd/system/user@.service.d/10-cabin-runtime-dir.conf
[Service]
Environment=XDG_RUNTIME_DIR=/run/user/%i
```

### 12.2 Do not duplicate the stock remote desktop unit

The first design shipped a `cabin-remote-desktop.service` that claimed the
`org.gnome.RemoteDesktop.Headless` bus name. The stock
`gnome-remote-desktop-headless.service` claims the same name, so
`grdctl rdp enable` failed with `File exists`.

The guest now configures with `grdctl` and then starts the stock unit. Cabin
owns configuration, not the daemon's unit.

### 12.3 Services with User= fail at step USER

`polkit`, `rtkit-daemon`, and `geoclue` all fail with
`Failed at step USER ... Operation not permitted`.

The investigation established the following, in order:

1. `CAP_SETUID` and `CAP_SETGID` are present, and a manual `setpriv` to the
   service user works. The user switch itself is not the problem.
2. The real message underneath is `Failed to keep CAP_SYS_ADMIN`. A rootless
   container's bounding set has no `CAP_SYS_ADMIN`, so systemd cannot retain it
   across the switch to the service user.
3. Several hardening directives need that capability applied after the switch.
   A bisect identified `PrivateDevices`, `RestrictSUIDSGID`,
   `MemoryDenyWriteExecute`, `LockPersonality`, `ProtectKernelTunables`,
   `ProtectControlGroups`, and `ProtectHostname`.
4. `--security-opt seccomp=unconfined` does **not** help, so podman's seccomp
   profile is not the cause.
5. `--cap-add SYS_ADMIN` does not help either. The container fails to start,
   because a rootless user cannot grant a capability it does not hold.
6. Turning every implicated directive off clears the step USER failure.
   `polkitd` then starts and exits 1 on its own. Run by hand as root, the same
   binary runs correctly and stays up.

Step 6 is where the work stopped. The remaining `polkitd` exit is unexplained.

`geoclue` and `rtkit-daemon` are masked in the image. Neither can work in a
container and neither is needed. `polkit` is left unmasked and failing, and is
tracked as open question 5.

Note the general lesson for the guest image: **`NoNewPrivileges=yes` must stay
on.** Turning it off stops seccomp filters installing, because a seccomp filter
needs either that flag or `CAP_SYS_ADMIN`. An early attempt to relax the unit
by setting it to `no` made matters worse.

### 12.4 gnome-remote-desktop requires NLA

A raw X.224 negotiation probe gave the decisive answer:

| Requested protocol | Server response |
|---|---|
| `0x01` TLS only | `RDP_NEG_FAILURE` = `HYBRID_REQUIRED_BY_SERVER` |
| `0x03` TLS or CredSSP | `RDP_NEG_RSP` = CredSSP/NLA |

The RDP client must therefore implement CredSSP with NTLM. Plain TLS is
refused. This raises the priority of the interoperability test in open
question 4, because CredSSP is the part of RDP that clients most often get
wrong.

The TPM warning that `grdctl` prints on every call is harmless. Credentials
fall back to a `GKeyFile`, which works.

### 12.5 Confirmed working

```
Startup finished in 649ms (userspace)

cabin-keyring.service                 active exited
cabin-rdp.service                     active exited
cabin-shell.service                   active running
gnome-remote-desktop-headless.service active running
cabin-session.target                  active active

LISTEN  *:3389
```

`org.gnome.Mutter.DisplayConfig` reports the virtual monitor, which confirms
the headless backend really composites:

```
('Meta-0', 'MetaVendor', 'MetaVirtualMonitor', '0x00'),
[('1920x1080@60.000', 1920, 1080, 60.0, 1.0,
  [1.0, 1.25, 1.333, 1.5, 1.667, 2.0], {'is-current': true})]
```

GNOME Shell reports version 50.4 and `wayland-0` exists in the runtime
directory.

The Shell screenshot D-Bus method returns `AccessDenied`. That is expected on
GNOME 47 and later, which restricts it to portal callers. It is a dead end for
diagnostics, not a fault.


---

## 13. Milestone 3 notes

The application now reads real state. The sidebar lists desktops, starts and
stops them, and creates new ones.

### 13.1 Bridging Tokio and GTK

The Podman client needs a Tokio runtime for its socket I/O. GTK objects are not
`Send`, so they cannot cross to a Tokio worker thread. `runtime.rs` holds the
bridge: the future runs on Tokio, the result crosses back through a one-shot
channel, and the callback runs on the main thread where it may touch widgets.

The event feed uses the same idea with a buffered channel, because it produces
many values instead of one.

### 13.2 The list is never mutated in place

Every change re-queries Podman and rebuilds the rows. This is deliberate.
Podman is the only store of state, so any attempt to keep a parallel model in
the interface would drift from it. Rebuilding a list of a few rows costs
nothing.

The Podman event feed drives the rebuild, so a desktop started from the command
line appears without the user refreshing anything. This is verified: an
external `podman start` produces `init` and `start` events and the list
reflects them.

### 13.3 Live tests

`crates/cabin-podman/tests/live_podman.rs` runs against a real Podman socket.
The tests are ignored by default because they need `podman.socket` and the
guest image.

```sh
cargo test -p cabin-podman -- --ignored --test-threads=1
```

They cover the create, list, start, stop, and remove cycle, that mounts and
sizes survive a round trip through container labels, and that a container
without Cabin labels never appears in the desktop list.

### 13.4 Password handling is not final

`create.rs` generates a per-desktop password and passes it to the guest as an
environment variable, so `podman inspect` reveals it. That is the same exposure
the guest already has, so it adds no new risk, but it is not where the password
belongs. Section 8 states the intent: the host keyring, through the Secret
portal. This is still to do.

---

## 14. Milestone 4 investigation: the RDP client stack

This section revises section 6.1. **The recommendation there is wrong for this
server.** IronRDP cannot connect to `gnome-remote-desktop`, and the reason is
structural rather than a bug we can work around.

The work below was done against a real guest, and every claim here is from an
observed run rather than from reading documentation.

### 14.1 What works

CredSSP interoperates correctly. This was the risk called out in open question
4, and it is settled:

| Test | Result |
|---|---|
| Correct password | Passes authentication, reaches licensing |
| Wrong password | Rejected at the CredSSP stage |

An external FreeRDP client agrees: a wrong password gives
`ERRCONNECT_LOGON_FAILURE`, and the guest logs
`transport_accept_nla: client authentication failure`.

### 14.2 Blocker: gnome-remote-desktop requires the Graphics Pipeline

The guest closes the connection with:

```
[RDP] Client did not advertise support for the Graphics Pipeline, closing connection
```

The requirement is hard-coded. The daemon binary carries a list of demands and
no setting relaxes any of them:

- the Graphics Pipeline, MS-RDPEGFX
- the DRDYNVC dynamic channel
- desktop resizing
- 32-bit colour depth
- fastpath output
- a pointer cache

**No published IronRDP crate implements MS-RDPEGFX.** Searching every fetched
crate for the channel name `Microsoft::Windows::RDS::Graphics` returns nothing.
`ironrdp-session` handles legacy fastpath bitmaps and RemoteFX, and there is no
graphics pipeline layer above them.

So the client is refused before any pixel is sent. This is not a defect to fix
in Cabin. It is a missing feature in the library.

Note that the RemoteFX Progressive codec itself does exist, at
`ironrdp-pdu/src/codecs/rfx/progressive.rs`. The codec is present; the channel
that carries it is not.

### 14.3 Second finding: IronRDP advertises a capability it does not implement

Before reaching the blocker, the connection deadlocked for a different reason.
It is worth recording, because it is a genuine IronRDP bug and it will affect
anyone using the library against a FreeRDP-based server.

`ironrdp-connector` sets `SUPPORT_NET_CHAR_AUTODETECT` in the client early
capability flags unconditionally, with no way to turn it off. The server takes
that at its word and begins connect-time network auto-detection, which includes
a bandwidth measurement. A diagnostic build showed what then happens:

```
autodetect phase, input len=25,    message_channel_id=Some(1004), io_channel_id=1003
  decoded as SendDataIndication on channel 1004
autodetect phase, input len=16347, message_channel_id=Some(1004), io_channel_id=1003
  decoded as SendDataIndication on channel 1004
```

The 16347 byte PDU is the bandwidth measurement payload. It arrives on the
message channel, and `ironrdp-connector` ignores anything on that channel that
is not an auto-detect request it recognises, then keeps waiting. The server
waits for a result that never comes. Both sides block, and the connection hangs
until it times out, roughly 200 seconds later.

Removing that one flag fixes the deadlock. The connection then proceeds
immediately, and the next PDU correctly arrives on the I/O channel:

```
autodetect phase, input len=35, message_channel_id=Some(1004), io_channel_id=1003
  decoded as SendDataIndication on channel 1003
```

Dropping the flag is safe. The guest treats auto-detection as optional and says
so: "Client does not support autodetecting network characteristics (RTT
detection, Bandwidth measurement). High latency connections will suffer!"

Both findings are worth reporting upstream. Neither is worked around in the
tree, because the blocker in 14.2 makes the point moot for now.

### 14.4 The options

| Option | Cost | Notes |
|---|---|---|
| **FreeRDP through FFI** | C FFI, and a build dependency | Proven to work against this exact guest. Implements the graphics pipeline, dynamic resize, and clipboard already. |
| **Implement MS-RDPEGFX on IronRDP** | Large | The channel protocol, surface and cache management, and capability negotiation. The RemoteFX Progressive codec already exists, so decoding is not the whole job. Best done as an upstream contribution. |
| **Use a different guest server** | Loses GNOME | A server such as `xrdp` is less strict, but the whole point is a real GNOME session. |

FreeRDP is the pragmatic choice, and it reverses the reasoning in section 6.1.
That section rejected FreeRDP partly because GNOME Connections only ships a
private vendored `gtk-frdp`. That argument still holds against using
`gtk-frdp`, but it does not apply to linking FreeRDP itself, which is a normal
shared library with a stable public API and a `pkg-config` file.

This is a decision for the project owner and is not taken here.

### 14.5 Guest fixes proven in this round

A host session drove a real FreeRDP client against the guest and captured
`docs/milestone1-rdp.png`: a genuine GNOME desktop over RDP, with the top bar,
Files, and Text Editor. Milestone 1 is complete, pixels included.

That run and the follow-up work found four guest defects, all now fixed:

1. **The guest did not survive a restart.** `cabin-firstboot.service` was
   guarded by a marker in `/var/lib`, which persists, but it wrote
   `/run/cabin/env`, and `/run` is a tmpfs. The second boot skipped
   provisioning, the environment file never reappeared, and the whole session
   failed. The unit is now `cabin-prepare.service`, it runs on every boot, and
   only the TLS certificate is generated once.
2. **`/dev/fuse` is required but is NOT sufficient.** This was recorded as
   fixed in an earlier revision of this document. That was wrong, and section
   15.2 has the correction. The device is passed in, and the daemon still
   aborts.
3. **Input was disabled.** `view-only` stayed `true`, so the session ignored
   every key press and pointer move. The cause is that
   `grdctl --headless rdp disable-view-only` exits 0 and does nothing. The
   same is true of `rdp disable-port-negotiation`. Settings are now written
   with `gsettings` and each write is read back and checked.
4. **`grdctl --headless rdp enable` is not a settings write.** It is the call
   that makes the daemon serve. Replacing it with a `gsettings set` of the
   `enable` key stopped the server binding at all, while the daemon runs
   happily with that key still reading `false`. It must be the last step, and
   it must be `grdctl`.

Point 3 was hidden by a shell helper that ended in `|| true`, which discarded
the exit status of every `grdctl` call. The helper now preserves it. A wrapper
that swallows errors costs more than it saves.

`grdctl status` reports on `gnome-remote-desktop.service`, not on the headless
unit that actually serves. It will claim the service is inactive and RDP is
disabled while the session is working. Do not use it as a health check.

### 14.6 The client must read back the granted resolution

Dynamic resize works, and it is exact except for rounding:

| Requested | Granted |
|---|---|
| 1280x720 | 1280x720 |
| 1024x768 | 1024x768 |
| 1920x1080 | 1920x1080 |
| 1703x961 | **1702x960** |

Odd sizes round down to even, as H.264 requires. Section 6.4 already rounds
before sending, which is right, but the viewer must also read the size back
from the server rather than assume the request was honoured.

### 14.7 Input, since resolved

At the time of this round input had never been proven. `view-only` was `true`
for every test, and synthetic input could not be injected because GNOME 50
ignores XTEST.

Both are settled now. Section 12.3 covers the `view-only` fix, and sections
15.9 and 15.10 prove keyboard and pointer input end to end through Cabin's own
client.

---

## 15. The FreeRDP client

Section 14 established that IronRDP cannot talk to `gnome-remote-desktop`.
FreeRDP is now the client stack. This section records how it is bound and what
is proven.

### 15.1 A C shim, not bindgen

`cabin-freerdp-sys` is a small C file plus about seventy lines of hand-written
`extern "C"` declarations. Bindgen was tried first and rejected.

FreeRDP marks nearly every struct member with `ALIGN64`, which expands to
`DECLSPEC_ALIGN(8)`, a per-member alignment attribute. Bindgen cannot lay that
out, and rather than failing it **silently degrades the struct to an opaque
blob**. `rdpContext`, `freerdp` and `rdp_client_entry_points_v1` all came out as
a single `_address: u8` with none of their fields. Nineteen structs were
affected, including every one the client needs.

Two red herrings cost time on the way, and are recorded so nobody repeats them:

- `ALIGN64` cannot be overridden from the command line, because `freerdp/api.h`
  defines it unconditionally. `DECLSPEC_ALIGN` sits behind an `#ifndef` in
  `winpr/spec.h` and can be neutralised. Doing so fixed the padding in other
  structs but did **not** make the core structs concrete.
- Clang now defaults to C23, where winpr expands `WINPR_ATTR_NODISCARD` to
  `[[nodiscard]]`. That attribute is not valid on a struct member. This is a
  real hazard for any bindgen user, though it was not the cause here.

The shim sidesteps all of it. Every function takes primitives or an opaque
pointer, so no FreeRDP layout is reproduced in Rust and the binding is ABI-safe
by construction. It also drops the libclang build dependency, which keeps the
Flatpak module simpler.

### 15.2 Correction: the FUSE clipboard crash is not fixed by /dev/fuse

An earlier revision of section 14.5 claimed passing `--device /dev/fuse` fixed
the daemon crash. **That was wrong.** The test command used to verify it carried
`-clipboard`, which stops the client advertising `cliprdr`, so the crash path
was never exercised. The device is necessary but not sufficient.

What actually happens, established by an A/B on the same image:

| Container | `fusermount3` result |
|---|---|
| `--device /dev/fuse` | `mount failed: Operation not permitted` |
| `--device /dev/fuse --cap-add SYS_ADMIN` | succeeds, mount appears |

`CAP_SYS_ADMIN` is the missing piece, and podman's default bounding set excludes
it. Because it is absent from `CapBnd`, no process inside can acquire it, not
even setuid `fusermount3`.

**But granting it breaks the guest.** With `--cap-add SYS_ADMIN` the RDP port
never comes up, because `systemd-logind` and `systemd-userdbd` fail with
`226/NAMESPACE`. With the capability present, systemd stops skipping the
namespace setup it knows it cannot do in a rootless container, attempts it, and
fails. The desktop session never starts.

So both branches are dead ends, and there is no server-side off switch: the
`org.gnome.desktop.remote-desktop.rdp` schema has nine keys and none of them
controls the clipboard.

The resolution is on the client. Cabin's own client never advertises `cliprdr`,
so the guest never attempts the mount. That is a real answer rather than a
workaround, at the cost of no clipboard sharing for now.

**The consequence is worth stating plainly: the guest is fragile against other
RDP clients.** Any client that advertises the clipboard, which is default-on
behaviour for stock `xfreerdp`, Remmina, GNOME Connections and `mstsc`, will
crash the guest daemon. Until this is resolved, connecting from a phone or
another computer needs the clipboard disabled on that client.

Aborting on a failed optional-feature mount is an upstream bug. Degrading to no
clipboard would be correct behaviour, and this is worth a
`gnome-remote-desktop` issue.

Note also that CLIPRDR is a **static** virtual channel. It never appears in
FreeRDP's "Loading Dynamic Virtual Channel" log lines, so seeing only `ainput`,
`rdpgfx`, `disp` and `rdpsnd` there does not mean the clipboard is off. It must
be disabled by name with `FreeRDP_RedirectClipboard`.

### 15.3 What is proven

The shim connects to a real guest and the graphics pipeline negotiates:

```
gdi_init_ex: Local framebuffer format  PIXEL_FORMAT_BGRA32
gdi_init_ex: Remote framebuffer format PIXEL_FORMAT_BGRA32
Loading Dynamic Virtual Channel rdpgfx
Loading Dynamic Virtual Channel disp
channel connected 'Microsoft::Windows::RDS::Graphics'  -> gfx pipeline init done
channel connected 'Microsoft::Windows::RDS::DisplayControl'
```

This is the thing IronRDP could not do. The stack choice is settled.

One piece of plumbing is worth knowing: the graphics pipeline delivers surfaces
to a channel, not through the legacy drawing orders. Unless the client hands
that channel to the GDI layer with `gdi_graphics_pipeline_init` on the channel
connected event, FreeRDP decodes frames that never reach `gdi->primary_buffer`,
and the framebuffer stays blank while the session looks healthy. Relatedly,
`update->EndPaint` is never called on the graphics pipeline path, so it cannot
be used to count frames.

### 15.4 Resolved: the missing DesktopResize callback

The client segfaulted inside FreeRDP as soon as the guest sent graphics. The
cause was a null function pointer of my own making.

`update->DesktopResize` is a callback the client must supply. The graphics
pipeline changes the desktop size through a `ResetGraphics` PDU, and FreeRDP
invokes `DesktopResize` **without checking it for null**. Leaving it unset is
therefore a null call on the first `ResetGraphics`, which arrives immediately
after the surface is created. Setting it fixed the crash outright:

```
connected
frames=1  size=1920x1080  stride=7680
non-black pixels: 2073600
```

`docs/milestone4-cabin-client.png` is that framebuffer: a real GNOME desktop
with the top bar and clock, captured through Cabin's own client rather than
through `xfreerdp`.

The implementation is the standard one: read `FreeRDP_DesktopWidth` and
`FreeRDP_DesktopHeight` from settings and call `gdi_resize`.

Three hypotheses were wrong on the way, and are recorded so the reasoning is
not repeated:

- **Not the experimental AV1 codec.** Fedora builds FreeRDP with
  `WITH_GFX_AV1=ON` and the library warns at runtime that it "might crash the
  application", which was a compelling but false lead. Section 15.5 explains
  why AV1 is never even selected.
- **Not the surface lifecycle.** With dynamic resolution enabled the guest
  creates surface 0, sends one frame, deletes both the encoding context and the
  surface, issues a second `ResetGraphics`, and creates surface 1, all within
  about 100 ms. That looked like an obvious use-after-free. Disabling dynamic
  resolution reduces it to a single clean surface with no teardown, and the
  crash still happened, which cleared it.
- **Not `EndPaint`.** It is never called on the graphics pipeline path.

### 15.5 The codec is RemoteFX Progressive, and that is a property of the host

Worth knowing before optimising anything. The client advertises H.264 and the
guest acknowledges it:

```
CapsAdvertise: ... Client cap flags: H264 (AVC444): true, H264 (AVC420): true
```

Yet every frame on the wire is `RDPGFX_CODECID_CAPROGRESSIVE`, and nothing
else: no AVC444, no AVC420, no AV1, no RFX, no planar, no NSC.

The reason is that the container has no GPU. With no render node there is no
hardware H.264 encoder, so `gnome-remote-desktop` falls back to its CPU
progressive encoder.

The consequence for Cabin is direct: **the codec depends on whether a render
node is passed into the container, not on the protocol.** Section 7.3 makes
that a per-desktop setting, so a desktop given the Intel node may well
negotiate AVC444 while a software-rendered one uses progressive. The client
must handle both, and must not hard-code the progressive path.

### 15.6 The session thread

FreeRDP is not thread safe and every call must come from the thread that
created the client, so a session owns a dedicated thread. The interface never
touches a FreeRDP pointer.

```
  session thread                          GTK main thread
  ┌──────────────────────┐                ┌────────────────────────┐
  │ cabin_rdp_poll       │                │ spawn_future_local     │
  │ framebuffer copy     │ ─ Frame ─────► │   display.set_frame    │
  │                      │  (bounded, 1)  │     GdkMemoryTexture   │
  │ apply(command)       │ ◄─ command ──  │   input, resize        │
  └──────────────────────┘   (mpsc)       └────────────────────────┘
```

The frame channel holds one item. A stale frame has no value, so a slow
interface should make the session wait rather than build a queue.

The framebuffer is copied rather than borrowed, and the copy strips row
padding. FreeRDP's buffer belongs to FreeRDP, is reused between frames and is
reallocated on resize, so it cannot cross a thread by reference.
`GdkMemoryTexture` then takes the packed buffer with a stride of `width * 4`.

### 15.7 Verified in the running application

A host session drove the built application against a live guest and checked
each claim against the guest itself rather than against Cabin's own logs.

**Rendering.** Selecting a running desktop fills the pane with the guest's
GNOME session, including a live notification, which proves it is a stream and
not a still frame.

**Letterboxing.** With the pane at 1380x604 and the guest still at 960x754, the
image was centred with equal bars and the aspect ratio held. Once the debounce
fired and the guest resized, the bars disappeared. In steady state the
letterbox is rarely visible, which is the intent: it exists for the transition.

**Resize.** Every request was granted exactly, confirmed through the guest's
own `org.gnome.Mutter.DisplayConfig`:

| Requested | Granted by the guest |
|---|---|
| 960x754 | `960x754@60.000` |
| 1380x604 | `1380x604@60.000` |
| 1130x804 | `1130x804@60.000` |

The "Keep these display settings?" dialogue never appeared, which confirms the
change goes over Display Control rather than `DisplayConfig`.

**The clipboard decision holds.** The demo desktop ran with `Devices=[]` and
`CapAdd=[]`, so no `/dev/fuse` and no added capabilities, and the guest journal
contained no `fuse` line at all. A stock `xfreerdp` against the same image
aborts the daemon within a second. Suppressing `cliprdr` in Cabin's own client
is therefore sufficient on its own, and needs nothing from the container
configuration. That matters for section 8: the safe default costs nothing.

**One defect found and fixed.** The debounce fired twice per gesture and sent
the same size twice, about 350 ms apart, plus a third identical request later
when nothing had changed. Each one is a Display Control round trip and a guest
re-layout for no benefit. `should_send_resize` now suppresses an exact repeat
of the previous request, while still sending a return to an earlier size.

### 15.8 Notes on testing the application

Two constraints are worth recording, because both cost time to discover.

**Synthetic X events do not work on Cabin's own window.** GTK4 reads input
through XInput2 and ignores core X events, so `XSendEvent` does nothing. It
works on `xfreerdp` only because that client reads core events. The working
route for automating Cabin's own chrome is **AT-SPI**: find the application on
the accessibility bus, walk to the `GtkListBox`, and call
`Selection.select_child`.

An input regression test therefore needs both mechanisms: AT-SPI to open a
session in Cabin, `XSendEvent` to type into the guest surface, then read the
framebuffer back to assert the pixels changed.

**The verification above ran under `GDK_BACKEND=x11`.** GNOME 50 blocks the
Shell screenshot D-Bus API for unlisted callers, so a Wayland window can be
neither captured nor clicked from a script. The frame path should not care, but
the render path has not been eyeballed on native Wayland and should be once.

### 15.9 Proving that input works

Input is verified headlessly, through Cabin's own client, with no display and
no third party RDP client. `crates/cabin-rdp/tests/live_input.rs` is the test.

**Do not assert on typed text.** A host session measured both approaches
against a real guest, as a fraction of pixels changed:

| Action | Changed fraction |
|---|---|
| 17 characters appended to existing text | 0.00026 |
| 5 characters typed into an empty editor | 0.0013 |
| 21 characters typed into an empty editor | 0.0018 |
| Escape with the overview already closed | 0.000000 |
| Super, opening the Activities overview | 0.399 |

Text is three orders of magnitude weaker than a shell state change, and it
depends on the font, theme and scaling. Worse, ink does not scale with the
number of characters: five characters produced 0.0013 and twenty one produced
only 0.0018, because the caret and layout dominate. So "assert the ink grew"
is not available either.

**The idle noise floor is exactly zero.** Consecutive frames from a quiescent
guest are bit identical, so no tolerance band is needed. The one periodic
change is the top bar clock, worth about twenty pixels once a minute.

**Assert the round trip.** Toggling the overview twice proves three things at
once: the keystroke arrived, the guest acted on it, and the frame path
delivered both transitions. Measured on this tree:

```
negative control, escape as a no-op: 0.000000
overview opened:                     0.360391
overview closed:                     0.360391
round trip back to the start:        0.000000
```

The thresholds are 0.05 for a change and 0.001 for no change, which leaves at
least a fortyfold margin on every assertion.

The negative control matters as much as the positive one. If `XSendEvent` or
any other part of a harness silently stops working, a test that only asserts
"something changed" fails in a way indistinguishable from a broken frame path.
Asserting that an inert keystroke changes **nothing** catches a test that
passes because something unrelated is repainting.

**One timing trap.** The overview close animation runs for over a second. With
a 700 ms quiet period the frame is captured mid-animation and the round trip
reads about 0.025 instead of 0, which looks exactly like a real failure. The
test waits 1500 ms and the comment says why.

### 15.10 The pointer, and the button transposition

The pointer path is verified the same way. The target is the top bar clock,
which makes the test a **position** check as well as a button check: a click
sent to the wrong place changes nothing.

```
calendar opened:                  0.012299
round trip after a second click:  0.000000
```

A popover covers far less of the screen than the overview, so it has its own
threshold of 0.002. That keeps a sixfold margin below the measured signal while
staying two hundred times above the clock tick.

Two things this test caught, both real:

1. **Clicks were being sent to (0, 0).** `SessionCommand::PointerButton`
   carried no coordinates, and the client passed zeros. RDP has no notion of
   "where the pointer already is": every pointer PDU states its own position.
   The variant now carries `x` and `y`.
2. **The pointer leaves a hover highlight.** The top bar highlights whatever
   the pointer is over, so a frame captured with the pointer resting on the
   clock differs from the baseline by that highlight alone, about 0.0013. The
   test parks the pointer somewhere inert before both captures. This is not
   noise, it is real state, and widening the threshold would have hidden it.

**The button numbering disagrees between GTK and RDP**, and only for the middle
and right buttons:

| Button | GTK | RDP flag | Value |
|---|---|---|---|
| left | 1 | `PTR_FLAGS_BUTTON1` | `0x1000` |
| middle | 2 | `PTR_FLAGS_BUTTON3` | `0x4000` |
| right | 3 | `PTR_FLAGS_BUTTON2` | `0x2000` |

Because the left button agrees, a smoke test that only left-clicks passes while
paste-on-middle-click opens a context menu. The values are not a straight swap
of the names either, so any arithmetic mapping is silently wrong. `button_flag`
writes the arms out, and a unit test asserts that the obvious arithmetic scheme
would disagree with it.

### 15.11 Keycodes

RDP carries PC/AT set 1 scancodes, while GTK reports X11 hardware keycodes,
which on Linux are evdev codes offset by eight.

The main block needs no table: evdev numbered its keys from the AT scancodes in
the first place, so evdev 1 is Escape and the AT scancode for Escape is `0x01`,
and that identity holds to the end of the keypad at evdev 83. Above that the
keys live in the `0xE0` prefixed set and `keymap.rs` carries a small table for
the arrows, navigation keys and Super.

An unmapped key is dropped rather than approximated. Sending a wrong scancode
would type the wrong character, which is worse than typing nothing.

### 15.12 The viewer needs a visible state

The first build shipped a viewer that drew a dark rectangle until a session
attached, and attached only when the user clicked a sidebar row. On first run
that is a black pane with nothing to explain it, which reads as a broken
application rather than as an empty one.

Two changes fix it:

- The pane is a `GtkStack`. Until a frame arrives it shows an `AdwStatusPage`
  that says what to do, and it reports connecting and disconnected states with
  the reason. The swap happens on the **first frame**, not on `Connected`,
  because a connected session with nothing drawn still looks broken.
- The sidebar opens the first running desktop once, at start-up. A guard stops
  the event feed reselecting behind the user on every refresh.

`CABIN_DEV_INSTANCE=1` opts out of the single instance behaviour, so a second
copy can run beside the first during development. GTK hands a second launch off
to the first and exits, which is right for users and awkward when testing.

### 15.13 Settings, and why the home directory is a volume

Changing a mount, the graphics mode, a resource limit or the socket policy all
need the container recreated: Podman cannot alter them in place. Recreating
would destroy the writable layer, and with it everything the user saved.

So a desktop's home directory lives in a named volume, `cabin-<name>-home`,
mounted at `/home/cabin`. Podman seeds it from the image on first use, and it
outlives the container. This is verified by
`crates/cabin-podman/tests/live_recreate.rs`, which recreates a desktop with a
changed configuration and checks the new settings applied.

The settings dialogue makes the safe choice the easy one:

- A new shared folder starts **read-only**. Making it writable is a switch the
  user has to reach for.
- The host path comes from the file chooser portal, so Cabin needs no
  filesystem permission of its own. Section 7.1 explains why that matters.
- The two dangerous socket options warn before they take effect, and the row
  snaps back to its previous value if the warning is declined. The safe option
  is labelled "Recommended" and the dangerous ones say what they grant: the
  power of your user account, or root on your computer.
- Applying is an explicit button, and it confirms first, because it restarts
  the desktop and closes whatever is running in it. The confirmation says the
  user's files are kept, because otherwise "recreated" sounds like data loss.

Deleting a desktop asks first and says plainly that everything saved inside it
goes with it.

### 15.14 Connecting from another device

The RDP port binds to `127.0.0.1` unless the user turns on remote access, which
asks for confirmation because opening a desktop to the network is not something
to do by accident.

The connection dialogue shows the address, port, username and password, each
with a copy button, because these get typed into a phone. The address comes
from connecting a UDP socket to a documentation address, which fixes the route
without sending a packet, and it never offers a loopback address.

Two warnings sit alongside, because both would otherwise waste the user's first
attempt:

1. **The client must have the clipboard turned off.** A client that advertises
   `cliprdr` crashes the guest session. Section 15.2 explains why, and that it
   is a fault in `gnome-remote-desktop` rather than in the client.
2. **The certificate warning is expected.** Each desktop makes its own
   certificate, so nothing has signed it.

### 15.15 The inotify limit caps how many desktops can run

Every desktop runs systemd, and systemd needs several inotify instances.
Fedora's default `fs.inotify.max_user_instances` is 128 for the whole user, so
after a handful of desktops the next one fails.

The failure is invisible without knowing what to look for. systemd cannot even
allocate its manager object, so it exits before logging anything:

```
Failed to create control group inotify object: Too many open files
Failed to allocate manager object: Too many open files
```

The container dies with status 255 and an **empty log**, and Podman reports the
start as successful, because it succeeded in handing the container to the
runtime. Nothing anywhere points at inotify.

Two changes deal with it:

- `start_desktop` waits briefly and confirms the container is still running.
  Podman's success means the runtime accepted it, not that the guest lives.
- An exit of 255 with no recorded error is reported as the inotify limit, with
  the `sysctl` command to raise it and where to make it permanent.

The guess is only a guess, so a real error from Podman is always preferred over
it. But 255 with an empty log is this failure often enough that naming it is
far more useful than reporting the number.

### 15.16 Exposing a container runtime

Binding the host socket works, and it was more involved than mounting a file.

**SELinux blocks it by default.** The socket is labelled `user_tmp_t`, which a
confined container may not open. The symptom is misleading: every access is
denied including `stat`, so `ls` reports the file as absent rather than
forbidden, and even root inside the guest is refused. Cabin sets
`security_opt: label=disable`, and only for the desktops that ask for a host
socket.

**The socket arrives owned by root.** In a rootless container the host user maps
to container root, so a socket owned by the host user appears as root's, and the
desktop user cannot open it. Rather than change anything on the host, or make
the user reach for `sudo`, the guest proxies it: a socket unit listens on
`/run/cabin/container.sock` owned by the desktop user and forwards to the bound
socket with `systemd-socket-proxyd`, which ships with systemd. `CONTAINER_HOST`
and `DOCKER_HOST` both point at the proxy, so tools work with no configuration.

Both variables are set for the Podman socket, because most tooling reads
`DOCKER_HOST` even when talking to Podman.

Verified: a desktop with the host Podman socket runs `podman info` as the
desktop user, with no `sudo`, and gets back the host's own hostname.

### 15.17 Nested Podman does not work yet, and is not offered

The dialogue does not list it. An option that quietly fails is worse than an
option that is absent.

It came close, and the remaining problems are recorded so the next attempt
starts further along:

1. **`newuidmap` cannot write a uid_map.** Rootless Podman inside a rootless
   container has no privilege to map a range for a child namespace. Emptying
   `/etc/subuid` and `/etc/subgid` fixes this: Podman then falls back to
   "rootless single mapping", where everything runs as one user. Setting
   `userns = "host"` in `containers.conf` does **not** help, because that
   governs the containers Podman runs, not Podman's own set-up.
2. **`fuse-overlayfs` needs `CAP_SYS_ADMIN`.** The `vfs` driver needs no mount
   and is the only one that works. It is slower and uses more disk.
3. **Networking needs `/dev/net/tun`.** Without it pasta fails with "Failed to
   set up tap device in namespace". The device is passed in for this policy.
4. **Still unresolved:** the storage database records `overlay` and then
   refuses the configured `vfs` driver on a later run, and the inotify limit in
   section 15.15 bites again from inside the guest.

The code paths for the policy are correct and stay in the tree. Only the choice
is withheld.

### 15.18 Pinning the certificate

Every desktop signs its own certificate, so a chain check can never succeed and
there is nothing to check against. Pinning is what makes the connection mean
anything: the first connection records the fingerprint, and a later one that
presents a different certificate is refused.

Three things had to be true for this to be more than decoration.

**`IgnoreCertificate` and `AutoAcceptCertificate` must be off.** Either one
makes FreeRDP accept the certificate without ever calling
`VerifyCertificateEx`, so the check would never run. An earlier version set
both, which is why the callback existed and did nothing.

**The certificate must outlive the container.** It lives in `/var/lib/cabin`,
which is now a named volume separate from the home directory. Without that,
every settings change would regenerate the certificate and look exactly like an
interception. It is a separate volume from the home directory on purpose,
because it holds the private key and nothing in the desktop session should be
able to read it.

**A deleted desktop must be forgotten.** Otherwise a new desktop reusing the
name would present a different certificate and be reported as an attack.

The store is a file per desktop in the application's data directory, keyed by
name rather than container id, because the id changes whenever a desktop is
reconfigured. Names come from the user, so they are reduced to safe characters
before being used as a file name, and a test covers `../` in a desktop name.

Verified live: a first connection records a fingerprint, and a connection
expecting a different one is refused.

### 15.19 Remembering where the user was

The window restores its previous size, and Cabin reopens the desktop that was
last in use rather than simply the first one running.

The maximised size is deliberately not recorded. Saving it would mean that
unmaximising after a restart restored a window the size of the screen, with no
way back to a sensible size except dragging.

One trap this exposed: **GLib aborts when a schema is missing**, so the moment
the application read a setting, a native run began exiting immediately with no
output at all. Nothing identifies the cause. `just run` now compiles the schema
into `target/glib-schemas` and points `GSETTINGS_SCHEMA_DIR` at it. The Flatpak
installs the schema properly and was never affected, which is exactly the sort
of difference that makes a packaged build look fine while development is
broken.

### 15.20 What is not finished

An audit of the tree against this document, rather than a list from memory.

**The password is not in the keyring.** Section 8 says the Secret portal. It is
still an environment variable on the container, which `podman inspect` shows.

**Clipboard sharing is blocked upstream.** Section 15.2 has the detail. It
cannot be enabled until `gnome-remote-desktop` degrades instead of aborting
when the FUSE mount fails.

**Audio is not started.** The RDPSND channel is deliberately disabled in the
shim.

**`polkit` does not run in the guest.** Open question 5. Privileged actions in
Settings inside the desktop will refuse.

Three things work but have never been checked in the way they will be used:

- The render path has only been seen under XWayland, never native Wayland.
- The widget's own event controllers and the keycode mapping are unit tested,
  but the live input tests drive `SessionCommand` directly, so the two have
  never been exercised together against a guest.
- The odd-dimension resize rounding has no test, because GTK kept constraining
  the pane to even sizes.

---

## 16. Milestone 8 corrections

Found while using the application rather than testing it. Each one is a case
where the code was right about the thing it was doing and wrong about the thing
around it.

### 16.1 The viewer pane must own what it shows

The pane kept whatever it last displayed. Deleting a desktop left its
"disconnected" notice on screen with nothing left to explain it, and selecting
a stopped desktop left the previous one's error in place.

There were three separate causes, and only the first is obvious.

**Nothing told the pane that a desktop had gone.** The sidebar refreshed itself
from the Podman event feed and the pane was never in that path.

**Selecting a stopped desktop returned early.** The handler logged "not
running" and left the pane alone, which is exactly the wrong thing to leave
alone.

**A dead session could write over a live one.** The session thread outlives the
moment it is replaced. Its failure arrives later, through a callback that
captured the shared stack, so a session the user had already moved on from
could put its own error over whatever had replaced it. This is the one that
would have been hard to find from a bug report, because it needs two sessions
and a specific interleaving.

The fix is to make the pane own its state instead of being written to from
several places:

- `Viewer` holds the desktop it is showing and whether a session is live.
- Every session carries a generation. `detach` increments it. A session whose
  generation is stale returns without touching anything.
- `Viewer::reconcile` runs after every refresh and settles the pane against
  what Podman actually reports.
- One function, `window::open`, is the only route into the pane. The sidebar
  selection, the Reconnect button and a desktop that starts again all go
  through it, so there is one set of rules rather than three.

A session that fails while its container keeps running is **not** retried
automatically. The pane offers a Reconnect button instead. Automatic retry
looks helpful until the Podman event feed produces a burst, and then it is a
reconnect loop.

The sidebar had a related defect. It rebuilds its rows from scratch on every
refresh, which drops the selection, so the highlight disappeared on every
Podman event while the pane kept showing the desktop. The selection is now
restored by name, behind a `restoring` flag so that restoring a highlight is
never mistaken for the user choosing a desktop.

### 16.2 The first resize was always lost

A new desktop connected at its created size and stayed there. Dragging the
window edge resized it correctly, which made it look like an initialisation
problem in the widget. It was not.

The Display Control channel is a dynamic virtual channel, so DRDYNVC negotiates
it after `PostConnect`. The widget is allocated its size well before that. So
the first resize request, the one that would make the desktop fit the pane,
always arrives before the channel exists.

Two pieces of bookkeeping then conspired:

1. The shim returned a generic error when `disp` was null, and the session
   logged it at debug and dropped the request.
2. The widget had already recorded the size in `last_requested`, and it
   suppresses an exact repeat, so it never asked again.

The desktop was therefore stuck until the user produced a *different* size by
hand.

The fix has three parts:

- The shim returns `CABIN_ERR_NOT_READY`, distinct from a refusal, and
  `cabin_rdp_display_ready` reports whether the channel has attached.
- The session holds a request that cannot be sent yet and sends it as soon as
  the channel appears.
- The widget records `last_requested` only when a session actually accepted the
  command. Recording a request that went nowhere is what made the failure
  permanent rather than momentary.

Attaching a session now also states the pane's current size at once, rather
than waiting for a size-allocate that may never come.

`live_session.rs` has a regression test. It queues the resize before the
session thread starts, which is the earliest a request can possibly arrive, and
asserts the guest reaches that size. With the fix removed it fails with the
guest still at 1920x1080.

### 16.3 The guest serves one session at a time

The live tests began failing when run together, with
`ERRINFO_LOGOFF_BY_USER` during the capabilities exchange. That reads like a
client fault and is not one.

`gnome-remote-desktop` serves one headless session. A connection that arrives
while the previous one is tearing down is rejected rather than queued. Cargo
runs the tests in one binary on several threads, so they were fighting over it.

`live_session.rs` now takes a process-wide lock and waits for the session to
recycle. Note that this only covers one test binary. Cargo runs separate test
binaries concurrently, so `live_input.rs` and `live_connect.rs` still need to
be run apart from it.

### 16.4 Settings were reachable but not findable

Per-desktop settings existed only in the sidebar row's overflow menu, which the
user did not find. Every comparable application puts the settings for the
machine you are looking at above the machine.

Settings now also sit in the viewer's own header bar, acting on whatever the
pane shows. Creating a desktop selects it, so a new desktop's settings are one
click from the dialogue that made it. The create dialogue says where the rest
of the settings are, rather than implying it is the whole set.

### 16.5 Backlog

Raised while testing, deliberately not built yet.

**Detect the container runtimes that are present.** The socket setting offers
Podman and Docker whether or not either is installed. It should read
`$XDG_RUNTIME_DIR/podman/podman.sock` and `/var/run/docker.sock`, mark what it
finds, and say plainly that the others are absent. The wording is already
corrected: the setting shares the runtime that is on the computer, and does not
give the desktop one of its own, which the old title implied.

**Application settings, separate from desktop settings.** A place for choices
that are about Cabin rather than about one desktop:

- Allow or forbid exposing a container runtime at all, so the dangerous choice
  can be turned off once rather than declined repeatedly.
- Show which runtimes were detected, with a Test button that proves the socket
  answers.
- The default guest image.

**Audio.** RDPSND is disabled in the shim. Nothing has been attempted.

---

## 17. Milestone 9 corrections

### 17.1 A desktop that has just started is not ready to answer

Starting a desktop and connecting to it reported "could not connect". Pressing
Reconnect a moment later worked, so the desktop was fine and the timing was
not.

Podman reports a container as running before the guest inside it binds the RDP
port. Measured on the development host, with a probe that connected once a
second from the moment the container started:

| Case | Gap before the first frame |
|---|---|
| Restart of an existing desktop | about 2.5 seconds |
| First boot with empty volumes | about 2.6 seconds |

So the first connection always lands in the gap. The session now retries for up
to 90 seconds instead of reporting a failure the user has to clear by hand. The
window is far wider than the measurement because a loaded host is slower, and
waiting costs nothing while a spurious error costs a click and some confidence.

Two failure modes need retrying, and only the first is obvious:

- **The port is not bound.** The connection is refused.
- **The port is bound but the session is not ready.** The connection succeeds
  and is then dropped before a single frame arrives. This one matters: an
  early version retried only refused connections and still failed, because the
  guest accepts and then hangs up.

So the rule is about frames, not about connecting: a session that ends before
it draws anything is treated as not up yet, and is tried again.

**A rejected password is not retried.** The guest cannot report one by
accident. `cabin-rdp.service` configures the credentials with `grdctl` and only
then starts `gnome-remote-desktop-headless.service`, so nothing is listening
until the password is in place. A changed certificate is not retried either,
since that is a decision for the user rather than a fault to wait out.

**Commands that arrive while waiting are kept.** The widget states the pane
size the moment it attaches, so that request is always sitting in the channel
while the retries run. An early version drained the channel to watch for
`Disconnect` and threw everything else away, which brought back the fault in
section 16.2 whenever a desktop was slow to start. The size the interface last
asked for is also re-applied to each new attempt, because a request handed to
an attempt that then died would otherwise be lost.

### 17.2 Resizing smaller could abort the process

Found while testing the retry, but not caused by it. It was reachable in
ordinary use, by dragging the window smaller.

`cabin_rdp_framebuffer` reports the GDI buffer with its width, height and
stride. During a resize those do not agree. A guest going from 1920 to 1152
wide reported:

```
width=1152  height=720  stride=7680   (7680 = 1920 * 4, the old width)
```

The buffer had already been reallocated for the new width, so it held 1152 * 4
* 720 bytes, while the description claimed 7680 * 720. Copying on that
description read a megabyte past the end of the allocation. Rust's
`copy_nonoverlapping` precondition check caught it and aborted the process,
which is a better outcome than the silent corruption that would otherwise
follow.

The client now checks that a stride could really belong to a row of that width
before it copies. Real row alignment adds a few bytes; anything further out is
a value left over from a different size, and the frame is skipped. The next
frame carries a consistent description.

Note that the direction of the mismatch matters. A stride shorter than the row
is caught by a bounds check, which panics safely. A stride longer than the row
passes every bounds check and reads memory that is not ours, so it is the
dangerous one, and it is the one that happens.

`cabin_rdp_framebuffer` also refuses to report a buffer for a client that is no
longer connected. `cabin_post_disconnect` calls `gdi_free`, and it can run
inside a poll that still reports the session as alive.

### 17.3 Creating a desktop offers its settings

The create dialogue asks for a name, memory and graphics. Everything else lives
in the desktop's settings, and a user who has just created a desktop is exactly
the user who wants them. The dialogue now carries an "Open settings" switch,
and the sidebar opens the settings as soon as the new desktop appears in the
list.

---

## 18. Milestone 10: the interface catches up

Five changes, all from watching the application be used rather than tested.

### 18.1 A name is checked as it is typed

Creating a desktop with a name already in use failed after the Create button
was pressed, and an `AdwAlertDialog` closes on any response, so the whole
dialogue went with it. The user then had to reopen it and set everything
again. The development host ended up with `fedora22` and `fedora23` as
evidence.

`cabin_core::check_name` now decides, and the dialogue calls it on every
keystroke. Create is insensitive while the name is unusable, and the reason
appears under the field. Nothing is rejected after the fact.

The name is not only a label. Podman uses it as the container name and Cabin
builds the volume names from it, so the rules are Podman's: it must start with
an alphanumeric character, and may then hold alphanumerics, underscore, dot and
dash. A name that differs from an existing one only in case is refused as well.
Podman would accept both, and the sidebar would then show two rows nobody could
tell apart.

The dialogue also opens on a name that is free, adding a number if it has to,
so the common case needs no typing at all.

### 18.2 Applying and running are separate questions

Settings had one button, "Apply and Restart". A desktop that is not running had
no reason to be started just because its shared folders changed.

There are now two: **Apply**, and **Apply and Start** or **Apply and Restart**
depending on what the desktop is doing. Apply recreates the container and
leaves it stopped.

Every setting in the dialogue needs the container recreated, because Podman
fixes a container's configuration when it is created and Cabin keeps that
configuration in its labels. There is no way to save these and keep running,
and the dialogue says so rather than implying a lighter kind of save exists.

When nothing has changed, the run button still means something, so it starts or
restarts what is already there. Only a running desktop is asked to confirm,
since only a running desktop loses anything.

### 18.3 A share is set up when it is added

Choosing a folder added it with defaults, and the options were behind the row's
expander. The location is the field most likely to need changing, and it was
the least visible.

Adding a share now asks: location, writable, and the SELinux relabel, with the
single file warning where it applies. The same options stay on the row
afterwards, so nothing here is a decision that cannot be revisited. The
location is checked as it is typed, for the same reason as the name in 18.1.

### 18.4 The desktop list can be put away

The layout was an `AdwNavigationSplitView`, which treats the sidebar as a place
to navigate back to. The desktop list is not that. It is something to hide when
the desktop needs the room.

It is now an `AdwOverlaySplitView` with a toggle in the pane's header bar. The
binding is two way, so a collapse the split view decides for itself on a narrow
window still leaves the button showing the truth. The state is saved with the
window geometry.

### 18.5 A fixed resolution can bring the window with it

Choosing a resolution larger than the pane letterboxes it. The desktop gets
smaller rather than larger, which reads as the setting having done nothing.

The display menu now offers "Resize window to match". The chrome is measured
rather than assumed, because it depends on the header bar and on whether the
desktop list is showing. Three details matter:

- A maximised or fullscreen window ignores a size request, so those states are
  left first. Without that the setting silently does nothing.
- A window larger than the screen cannot be shown, so the request is clamped to
  the monitor and the pane scales for the remainder. This is reported rather
  than left to look like a fault.
- Turning the option on acts at once when a fixed resolution is already
  chosen, rather than waiting for the user to pick the same one again.

---

## 19. Guest images

### 19.1 The image is ours, and it is built here

A desktop runs from `localhost/cabin-guest:f44`, which `guest/Containerfile`
builds from `registry.fedoraproject.org/fedora:44`. That base is the only
upstream dependency. Everything above it, the GNOME session, the systemd
units, the `cabin` user and the RDP configuration, is in this repository.

The image is never published. There is no registry account, no build pipeline
and no signing key, so pulling one would mean trusting something nobody has set
up. Building locally avoids all of that, at the cost of several minutes on
first use.

That cost has to be paid somewhere visible. Until this section was written it
was paid by the first desktop failing with:

```
Podman returned 404: no such image: localhost/cabin-guest:f44: image not known
```

which is accurate and names the wrong thing: the image, not the fix. Worse,
section 7.4 claimed Cabin already built the image on first use, and it never
did. That claim was aspirational and read as a description.

### 19.2 Managing images

The main menu opens a list of the images on the host: name, size and short id,
with the one Cabin uses marked. From there an image can be built, rebuilt or
deleted.

**The build context ships with the application.** `guest/` installs to
`<datadir>/cabin/guest`, and `guestdir::shipped` looks there, at `/app/share` for
the Flatpak, beside the executable, and finally in the source tree so that
`just run` works without installing. A directory only counts when it holds a
Containerfile, or a partly installed one would be chosen over a complete one
and the build would fail for a reason that points nowhere.

**The context is a tar built in memory.** File modes are deliberately not
preserved: the Containerfile runs `chmod` on everything it copies, so the
archive does not need them, and leaving them out means the build behaves the
same however the files were checked out. Entries are sorted, so the same tree
always produces the same archive and Podman's layer cache is not defeated by
the order the filesystem happens to return.

**Build output is streamed.** A guest image takes minutes, and a progress
display that only appears at the end is not a progress display. The build
endpoint reports a failure inside a stream that still ends with 200, so the
error is read out of the body rather than from the status.

**The list is Cabin's images only.** `guest/Containerfile` carries
`LABEL io.github.jaydoubleu.cabin.guest="true"`, so anything built from it is
told apart from the rest of the host, including images built with
`just guest`. A switch shows everything else. The default matters: the
development host carries two dozen images belonging to other work, and an
image manager that offers to delete those by default is a trap rather than a
convenience. An image built before the label existed is still recognised by
its name, so the one in use is never hidden from its owner.

Deleting is worth having for a reason that only showed up once the list
existed. The development host had fourteen untagged 1.65 GB images, left behind
by earlier rebuilds, using around 23 GB that nothing accounted for. Podman
refuses to delete an image a container still uses, and Cabin shows that refusal
rather than forcing past it.

Clearing them is a bulk delete, so it is a review rather than a warning. The
confirmation lists every image it would remove with its size, because a bulk
delete that says only how many is not something a person can agree to. The
button carries a trailing ellipsis, which is the platform's own way of saying
that it asks first. The result reports what Podman actually removed rather than
what was predicted, since the decision is made on the host at the moment of the
request and the two can differ.

Note that pruning is not limited to Cabin's images. Podman prunes by
danglingness, not by owner, and the confirmation says so.

### 19.3 Opening a shell in a desktop

`podman exec -it -u cabin -w /home/cabin <name> bash` gives a shell inside a
running desktop, as the user the desktop runs as, in its home directory. Both
flags matter: without `-u` the shell is root, which owns none of the files in
that home directory, and without `-w` it starts at `/`.

Cabin shows this command and copies it. It does not run it. Starting a terminal
on the host needs `--talk-name=org.freedesktop.Flatpak`, which the sandbox
design rules out, so offering a button that only worked outside the Flatpak
would be worse than offering none.

### 19.4 Toolbox integration was considered and rejected

The idea was to make a Cabin desktop double as a Toolbx container, so
`toolbox enter` would reach it. Inspecting a real toolbox container settles it.

| | `fedora-toolbox-44` as it runs | A Cabin desktop needs |
|---|---|---|
| PID 1 | `toolbox init-container` | `systemd` |
| PID namespace | host | its own |
| Privileged | yes | no, rootless |
| SELinux | `label=disable unmask=all` | enforcing |
| Home | the host home bind mounted | portal-chosen paths only |
| Host root | `/` at `/run/host` | nothing |
| Network | host | its own, with a published port |

The PID namespace alone decides it: systemd as PID 1 needs its own, and toolbox
uses the host's. The rest compounds it. Toolbox exists to dissolve the boundary
between container and host, which is the opposite of what a Cabin desktop is
for, so a container that was both would be neither.

The goal behind the request, reaching a desktop from a terminal, is met by
19.3 with none of that cost.

---

## 20. The guest recipe

### 20.1 One shipped tree, and copies of it

The recipe is `guest/Containerfile` and the files beside it. Cabin ships one
copy, in the installation, and the user may hold any number of their own.

| | Shipped | The user's |
|---|---|---|
| Where | `<datadir>/cabin/guest` | `<config>/cabin/images/<name>` |
| Writable | No | Yes |
| Follows Cabin updates | Yes | No |
| Tag | `localhost/cabin-guest:f44` | `localhost/cabin-guest-<name>:latest` |

The shipped tree is read-only because it lives inside the installation, under
`/app/share` in the Flatpak and under `/usr/share` in a system install. Editing
in place is not merely discouraged, it is impossible, so the interface shows it
read-only rather than offering a save that would fail.

**A copy never replaces the shipped recipe.** An earlier version let one copy
live at `<config>/cabin/guest` and take the shipped recipe's own tag, so taking
a copy hid the recipe Cabin ships. That was wrong for three reasons. A user
whose copy stopped building had no way back to a working guest. Two recipes
that build one tag cannot both be built, so there was no way to compare them.
And the New Desktop dialogue offers images by tag, so the shipped image simply
disappeared from it.

A copy is now a named recipe like any other, with its own name and its own tag.
The shipped recipe is therefore always present, always offered, and always the
thing that works on a machine with nothing else on it. `guestdir::shipped()`
has no fallback and no override: there is one shipped tree and it is found or
it is not.

`recipes::migrate_legacy_copy` moves a copy made by the earlier version into
`<config>/cabin/images/my-guest` at start-up, and carries its origin marker
with it. It runs before anything reads the recipe list, so the user never sees
the intermediate state.

**The whole tree is copied, not only the Containerfile.** The Containerfile
copies the scripts and units beside it, so a copy holding one file would build
nothing.

**A copy records where it came from.** `<config>/cabin/images/<name>.origin`
holds a hash of the shipped tree at the moment of copying, and the interface
says so when the shipped tree has moved on. Without it there is no way to tell
a copy that is current from one taken three releases ago, and the second
quietly keeps building an old guest.

The marker also answers a second question at no extra cost: a recipe that has
one is a full copy, and a recipe that has none is a layer. A layer is not a
copy of anything and so cannot fall behind, which is why it is never warned
about. One fact answers both questions rather than two that can disagree.

That marker sits beside the copy rather than inside it. Anything inside is part
of the build context and gets sent to Podman, and a build context should hold
what the build needs and nothing else. Deleting a recipe therefore has to
delete the marker separately, or the next recipe of the same name inherits it.

The hash is FNV-1a over the packed context. Packing is already deterministic,
sorted and without file modes or times, so hashing it answers "has any of this
changed" without walking the tree twice. This is a change check rather than a
security check, so a short non-cryptographic hash is the right size.

### 20.2 What the interface offers

The Guest Images dialogue lists every recipe, the shipped one first, and each
row says whether its image is built and offers to build it.

- **Shipped**: View, and "Copy to a new recipe".
- **The user's**: Edit, and Delete.

Copying the shipped recipe and adding to it are the same act with a different
starting point, so they are one dialogue rather than two that have to be kept
in step. `recipe::present_new_from` opens the New Recipe dialogue on the chosen
starting point.

Viewing and editing are the same window. The banner says which recipe it is,
since only one of the two can be saved and the window otherwise looks the same
either way. Editing says plainly that changes take effect at the next build,
because nothing visible happens on save.

**The two dialogues call each other, so their callbacks cannot stay generic.**
The recipe window opens the New Recipe dialogue, which opens the recipe window
on what it made. While the callback is `impl Fn()` each hop is a fresh closure
type, so the compiler instantiates the pair forever and stops at the recursion
limit. `present_path_boxed` and `present_new_boxed` take `Rc<dyn Fn()>` and
break the chain; the public entry points stay generic for callers.

`live_overlay.rs` proves the join that nothing else covers. Packing is unit
tested and building is tested against the shipped tree, but that a *user's*
recipe is the one that reaches the image was not. The test writes a recipe with
a marker and a file in a subdirectory, builds it, and runs the image to check
both arrived. The subdirectory matters: a context that packed only the top
level would build a guest with no scripts, and that failure would appear much
later and look like something else.

---

## 21. Named recipes, and choosing one

### 21.1 A recipe per image

Section 20 gave the user their own copies of the recipe. That answers "change
the guest" but not "have more than one guest". A user who wants a desktop with
a different toolchain should not have to choose which single guest everything
uses.

A recipe is a named build context. The shipped one is always there, and the
user may add more.

| | Shipped | The user's |
|---|---|---|
| Where | `<datadir>/cabin/guest` | `<config>/cabin/images/<name>` |
| Tag | `localhost/cabin-guest:f44` | `localhost/cabin-guest-<name>:latest` |
| Starts from | `registry.fedoraproject.org/fedora:44` | the base tag, or a full copy |

**A custom recipe builds on the base rather than repeating it.** This is the
decision the feature rests on. Repeating the base would mean every custom image
rebuilt the whole guest, and would drift from the base as Cabin updates.
Building on it keeps only the difference. Measured on the development host,
adding one package to the base:

| | Time | Added to disk |
|---|---|---|
| Full guest build | 57 s | 1.65 GB |
| Layer on the base | 11 s | about 30 MB |

The starter recipe a new image gets is therefore `FROM` the base tag, with the
Cabin label and a commented example. It carries the label so the image manager
lists it, exactly as the base does.

The name goes in the tag, so an image can be traced back to the recipe that
made it and `podman images` is readable without Cabin. Podman's tag rules
therefore apply to the name, and `recipes::check_name` enforces them as it is
typed: lower case, starting with a letter. Rejecting here means the failure is
about the name rather than about a request the user never saw.

Deleting a recipe leaves the image it built. The two are not the same thing,
and a desktop may still be running from the image.

### 21.2 Choosing an image for a desktop

The New Desktop dialogue offers every recipe in a dropdown, named the way the
recipe list names them, with the shipped one first and selected unless the user
has chosen another default.

**Every recipe is offered, whether or not its image exists.** Offering only
built images was tried and it is wrong: a fresh installation then shows an
empty dropdown, and a user who has just made a recipe cannot find it. A choice
whose image is not built says so, and Create stays disabled with a Build button
beside the reason. That is something the user can act on. An empty list is not.

The Build button builds the *selected* recipe. Building Cabin's own image
instead would leave the warning showing after a build that appeared to succeed.

Only Cabin's own images appear, and only those with a tag: an untagged image
left by a rebuild cannot be referred to and would not survive the next
clean-up. An image whose recipe has been deleted is still offered, because
deleting a recipe leaves its image alone on purpose and desktops made from it
keep working.

A row beneath the dropdown says where the image came from, what it costs, and
its tag, because the dropdown shows a name and only the tag is unambiguous.

A custom recipe cannot be built before the base is, because it starts `FROM`
it. The recipe row says that rather than letting Podman report a missing image
the user never asked for.

### 21.3 A recipe says what is load-bearing

A user editing a recipe cannot tell what the desktop needs from what is merely
present. Nothing in a Containerfile says which line is holding the session up.

Every section of the base recipe is now marked:

- `[REQUIRED]` with the reason. Removing it stops the desktop working, and the
  note says how it fails.
- `[YOURS]` for what is safe to change.

The reasons are the ones that cost time to find. That `geoclue` and `rtkit`
fail at "step USER" in a rootless container. That systemd shuts down on
`SIGRTMIN+3`, so the default signal makes every stop wait for the timeout and
then kill the container. That the session target is enabled `--global` because
it belongs to the user session. These were in DESIGN.md and in nobody's editor.

The label is the one that fails quietly: remove it and the image still builds,
and then appears nowhere in Cabin. Its note says exactly that.

The package list is now two steps rather than one. The first holds what the
session needs, with a line saying what each group is for. The second holds
applications and tools and is marked `[YOURS]`. Splitting them is not
cosmetic: editing the second rebuilds only that layer, which takes seconds,
while the session above stays as it is. The package set is unchanged, and the
rebuilt image boots in four seconds and answers the RDP probe.

A new recipe can therefore start two ways, and the dialogue states the cost of
each rather than describing them as simple or advanced:

| | Builds in | Follows Cabin | Good for |
|---|---|---|---|
| On the Cabin image | seconds | yes | adding software |
| A full copy | minutes | no | changing the desktop |

A full copy brings the whole tree, because the base Containerfile copies the
scripts and units from beside it. A copy of the one file would build nothing,
and the failure would name a missing path rather than the missing copy.

### 21.4 Names have to fit where they are shown

Two truncations, one cause. A name was carrying its own qualifiers, and the
place showing it was narrower than the name had become.

`Cabin guest (yours) (default)` appeared in the New Desktop dropdown as
`Cabin guest (yours) (de...`. The base recipe is now called `Cabin guest`
whatever its state. Whether it is the user's copy, and whether it is the
default, are marks beside the row rather than words inside the name. A name
appears in a list row and in a dropdown, and both truncate.

The second truncation was the widget rather than the text. `AdwComboRow` puts
the selected item on the same line as the title, which leaves it a few
characters wide in a dialogue: `The Cabin guest image` came out as
`The Cab...`. Setting `use-subtitle` moves the selection to its own line, and
anything explanatory moves to a row of its own. That is the general fix
wherever a combo carries something longer than a word.

The tag and size that were in the image name are now in a row beneath the
choice. The name has to be short enough for a dropdown; the tag is what makes
it unambiguous, and it has the width to be readable there.

### 21.5 The default image is chosen, not inferred

A new desktop starts on the image marked as the default, which is Cabin's own
until the user says otherwise. The image manager sets it with a star on each
recipe row, lit on the one in use.

**The star is a button on the row, not an entry in the overflow menu.** Which
image a new desktop starts on has to be visible as well as settable, and a menu
entry shows neither state nor the alternatives. The star is insensitive on a
recipe whose image is not built, with a tooltip saying to build it first,
because a desktop cannot be made from an image that does not exist.

It is stored as a tag rather than a recipe name, because it is a desktop's
image being chosen and an image can outlive the recipe that made it.

Creating a desktop from a different image does not change the default. An
earlier version remembered the last image used, which is reasonable on its own
and wrong beside an explicit setting: choosing a default in one place and
having it silently replaced from another leaves the setting meaning nothing.

### 21.6 Data in a label is not markup

`AdwPreferencesRow` parses its title and subtitle as Pango markup. An untagged
image was shown as `<untagged> abf4cebddb04`, which reads as an unclosed tag,
so GTK dropped the label and logged a warning for every such row.

Two fixes, because either alone leaves a hole. The name no longer uses angle
brackets, and every row whose text comes from Podman or the filesystem sets
`use-markup` to false. The second matters more than the first: a shared folder
row takes a host path as its title, and a path holding an ampersand would have
failed the same way. That one had not been noticed.

The rule is that anything from outside the program is data. A label that parses
it as markup will one day be handed something that is not.
