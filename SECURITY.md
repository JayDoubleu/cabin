# Security policy

## Reporting a vulnerability

Report a vulnerability privately through GitHub's private vulnerability
reporting:

1. Go to the [Security tab](https://github.com/JayDoubleu/cabin/security) of
   the repository.
2. Select **Report a vulnerability**.
3. Describe the problem, the version or commit, and the steps to reproduce it.

Do not open a public issue, a pull request or a discussion for a
vulnerability. A public report puts users at risk before a fix exists.

You get a reply when the maintainer has read the report. Cabin is maintained
by one person in their own time, so there is no fixed response time.

## Supported versions

Only the latest release and the `main` branch get security fixes.

## In scope

- **Container escape** through anything Cabin configures: mounts, devices,
  SELinux labels, socket options, or the guest image and its units.
- **Credential exposure** beyond what the documentation states. Each desktop's
  password is currently in the container environment, which `podman inspect`
  shows to the host user. That is known and documented. Exposure to anyone
  else is in scope.
- **Memory safety in the RDP client**, which is `cabin-rdp`,
  `cabin-freerdp-sys` and its C shim. A guest or a network peer that can make
  the client read or write out of bounds is in scope.
- **Network exposure** that is wider than the remote access setting says, for
  example a desktop that answers on an address its settings do not name.
- **Certificate pinning** that a changed certificate can get past without the
  user's agreement.
- **Flatpak sandbox permissions** that grant more than the manifest documents.

## Out of scope

- A kernel exploit from inside a desktop. A container shares the host kernel,
  and the README says Cabin is not a sandbox for hostile code.
- What a desktop can do when the user gives it the host Podman or Docker
  socket. The dialogue warns that this grants the power of the user's account,
  or root.
- What a desktop can do to a folder the user shared with write access. Guest
  root maps to the user's own account, and DESIGN.md section 8 says so.
- Faults in FreeRDP, `gnome-remote-desktop`, Podman or GNOME themselves.
  Report those to the projects concerned. If Cabin uses one of them in an
  unsafe way, that is in scope.
