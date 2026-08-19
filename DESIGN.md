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

Clipboard uses the CLIPRDR channel, bridged to `GdkClipboard`. Text first,
images second.

Audio uses the RDPSND channel. `gnome-remote-desktop` sources it from PipeWire
in the guest. On the host, Cabin plays it through the Flatpak PulseAudio
socket. Audio is scheduled after the first release.

---

## 7. Feature specifications

### 7.1 Mounts

A mount has a host path, a guest path, and a mode.

| Field | Values |
|---|---|
| Host path | Any directory, chosen through the file chooser portal |
| Guest path | An absolute path, default `/mnt/<basename>` |
| Mode | Read-write or read-only |

Cabin passes mounts to Podman as bind mounts with the `:ro` or `:rw` option,
plus `:U` when the user IDs need remapping and `:z` when SELinux relabelling is
wanted.

The file chooser portal matters here. Cabin runs in a Flatpak sandbox and does
**not** hold `--filesystem=home`. The portal returns a path for a directory the
user picked, and Cabin passes that path string to Podman. Podman runs on the
host and opens the path itself. The application never needs broad filesystem
access to make a mount work.

Changing mounts needs the container to be recreated. Cabin states this in the
dialogue and offers to do it, preserving the writable layer through a named
volume for `/home/cabin`.

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

Cabin builds the base image on first use if it is absent, and shows build
output in a progress page.

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
| 4 | `CabinDisplay` shows guest frames, read-only | The hardest component | **Blocked, see section 14** |
| 5 | Keyboard and pointer input | Usable desktop |
| 6 | Fit-to-window and fixed resolutions | The headline feature |
| 7 | Create, delete, mounts, GPU and socket settings | Feature complete |
| 8 | Clipboard, audio, connection profiles for other devices | Polish |

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

### 14.7 Still unverified

Input has never been proven end to end. `view-only` was `true` for every test
so far, and the host session could not inject synthetic input, because GNOME 50
ignores XTEST injection on the host. Now that `view-only` is `false` this needs
one manual check: connect, and type.

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

### 15.4 Open: the client segfaults once frames flow

The session establishes, all channels connect, and then the process segfaults
inside FreeRDP's event handling as soon as the guest sends graphics.

Ruled out so far:

- Not the clipboard. Disabling `FreeRDP_RedirectClipboard` fixed a separate,
  earlier crash where the guest daemon aborted. The session is stable for
  thirteen seconds now instead of dropping immediately.
- Not `EndPaint`. It is never called on this path.
- Not the experimental AV1 codec on its own. Fedora builds FreeRDP with
  `WITH_GFX_AV1=ON`, and the library warns at runtime that experimental options
  "might crash the application". Disabling `FreeRDP_GfxCodecAV1` and pinning
  RemoteFX Progressive did not stop it.

Still to check: whether stock `xfreerdp` still works against the same guest,
which separates a shim defect from a library or guest problem; and a proper
backtrace, which needs `gdb`, not currently installed.

The remaining channel settings are also suspect. `rdpdr`, `rdpsnd` and
`AUDIO_PLAYBACK_DVC` still connect even though the shim sets
`FreeRDP_DeviceRedirection`, `FreeRDP_AudioPlayback` and friends to false, so
those settings are either being applied too late or overridden by
`freerdp_client_load_addins`.
