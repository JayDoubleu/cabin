use crate::mount::Mount;
use serde::{Deserialize, Serialize};

/// How a desktop renders. See DESIGN.md section 7.3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum GraphicsMode {
    /// Cabin picks the first non-NVIDIA render node it finds.
    #[default]
    Automatic,
    /// No device is passed in. Mesa falls back to llvmpipe.
    Software,
    /// A specific DRM render node, for example `/dev/dri/renderD129`.
    RenderNode { path: String },
}

impl GraphicsMode {
    /// Resolves to the device path to pass to Podman, if any.
    ///
    /// `available` is the list of render nodes with their driver names, which
    /// the caller reads from sysfs.
    pub fn resolve<'a>(&'a self, available: &'a [(String, String)]) -> Option<&'a str> {
        match self {
            Self::Software => None,
            Self::RenderNode { path } => Some(path.as_str()),
            // NVIDIA needs the Container Device Interface and matching guest
            // drivers, so it is never chosen automatically.
            Self::Automatic => available
                .iter()
                .find(|(_, driver)| driver != "nvidia")
                .map(|(path, _)| path.as_str()),
        }
    }
}

/// A network interface on the host, with the addresses it currently holds.
///
/// Cabin performs no I/O in this crate, so the caller reads these from the
/// system and passes them in. This mirrors how render nodes are handled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetInterface {
    pub name: String,
    /// Every address on the interface, as strings, IPv4 and IPv6 together.
    pub addresses: Vec<String>,
    /// True for the loopback interface, which is always reachable and is
    /// never worth offering as a choice.
    pub loopback: bool,
}

/// Which addresses a desktop's RDP port publishes on. See DESIGN.md section 7.5.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Exposure {
    /// Loopback only. Nothing outside this computer can connect.
    #[default]
    Loopback,
    /// Every address on the machine, including any that appear later.
    Everywhere,
    /// Only the named interfaces.
    ///
    /// Interface names are stored rather than addresses, because an address
    /// changes whenever the network does. Podman fixes the bound addresses
    /// when it creates the container, so Cabin resolves the names when it
    /// creates the container and again before each start, and recreates the
    /// container when the addresses differ. See [`Exposure::needs_rebind`].
    /// A stored address would leave the desktop unable to start after a DHCP
    /// lease changed, and the reason would not be obvious.
    Interfaces { names: Vec<String> },
}

impl Exposure {
    /// The host addresses to publish the RDP port on.
    ///
    /// `available` is the host's interfaces, which the caller reads from the
    /// system. An interface that is named but currently has no address
    /// contributes nothing, so the desktop still starts.
    pub fn bind_addresses(&self, available: &[NetInterface]) -> Vec<String> {
        match self {
            Self::Loopback => vec!["127.0.0.1".to_string()],
            Self::Everywhere => vec!["0.0.0.0".to_string()],
            Self::Interfaces { names } => {
                // Loopback is always included. Without it the desktop would
                // stop being reachable from the computer it runs on, which is
                // never what choosing an interface is meant to mean.
                let mut addresses = vec!["127.0.0.1".to_string()];
                for name in names {
                    let Some(interface) = available.iter().find(|i| &i.name == name) else {
                        continue;
                    };
                    for address in &interface.addresses {
                        if !addresses.contains(address) {
                            addresses.push(address.clone());
                        }
                    }
                }
                addresses
            }
        }
    }

    /// True when the container must be recreated before it starts, because
    /// the addresses it binds are no longer the ones the chosen interfaces
    /// hold.
    ///
    /// `bound` is the host addresses of the container's current port
    /// mappings. Podman binds those exact addresses at every start, so an
    /// address that has gone makes the start fail. Loopback and Everywhere
    /// bind fixed addresses and never need this.
    pub fn needs_rebind(&self, bound: &[String], available: &[NetInterface]) -> bool {
        if !matches!(self, Self::Interfaces { .. }) {
            return false;
        }
        use std::collections::BTreeSet;
        let wanted: BTreeSet<String> = self.bind_addresses(available).into_iter().collect();
        let current: BTreeSet<String> = bound.iter().cloned().collect();
        wanted != current
    }

    /// True when something other than this computer can connect.
    pub fn reaches_other_devices(&self) -> bool {
        !matches!(self, Self::Loopback)
    }

    /// The interfaces chosen, when specific ones were.
    pub fn chosen_interfaces(&self) -> &[String] {
        match self {
            Self::Interfaces { names } => names,
            _ => &[],
        }
    }
}

/// Whether a desktop can reach a container runtime. See DESIGN.md section 7.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SocketPolicy {
    /// Nothing is exposed.
    #[default]
    None,
    /// The desktop runs its own rootless Podman. This is the safe choice.
    Nested,
    /// The host Podman socket is bind-mounted in. Dangerous.
    HostPodman,
    /// The host Docker socket is bind-mounted in. More dangerous: the Docker
    /// daemon runs as root, so this grants root on the host.
    HostDocker,
}

/// Where a host socket is bound inside the guest.
///
/// The host user maps to container root, so this lands owned by root and the
/// desktop user cannot reach it. The guest proxies it to
/// [`GUEST_PROXY_SOCKET`], which the desktop user owns.
pub const GUEST_HOST_SOCKET: &str = "/run/host-container.sock";

/// The socket the desktop user actually talks to.
pub const GUEST_PROXY_SOCKET: &str = "/run/cabin/container.sock";

/// The SELinux type a desktop container runs as.
///
/// Not the default `container_t`, because the desktop has to sandbox things
/// inside itself. Fedora loads every image through glycin, which decodes each
/// one in a `bwrap` sandbox, and that sandbox mounts its own `devpts`.
/// `container_t` is not allowed to, so the mount fails, every decode fails,
/// and the session shows blank squares where icons should be. Text still
/// draws, so it looks like a theme fault rather than a policy one.
///
/// `container_engine_t` is the type for a container that runs containers, and
/// it permits that mount. It is not `label=disable`: the container keeps its
/// SELinux type and keeps its own MCS categories, so it stays separated from
/// other containers. Disabling labelling would give up both.
pub const GUEST_SELINUX_TYPE: &str = "container_engine_t";

impl SocketPolicy {
    /// The host socket to bind into the guest, if any.
    ///
    /// Returns `(host path, guest path)`. The host path is resolved by the
    /// caller for Podman, because it lives under the runtime directory.
    pub fn guest_socket_path(self) -> Option<&'static str> {
        match self {
            Self::HostPodman | Self::HostDocker => Some(GUEST_HOST_SOCKET),
            Self::None | Self::Nested => None,
        }
    }

    /// True when the container must run without SELinux labelling.
    ///
    /// The host socket is labelled `user_tmp_t`, which a confined container
    /// cannot open. Without this the mount appears and every access is denied,
    /// including `stat`, which reads as the file being absent.
    pub fn needs_selinux_disabled(self) -> bool {
        self.guest_socket_path().is_some()
    }

    /// The `--security-opt label=` argument for a desktop.
    ///
    /// Exposing a host socket needs labelling off altogether. Every other
    /// desktop stays confined, under the type in [`GUEST_SELINUX_TYPE`].
    pub fn selinux_label_option(self) -> String {
        if self.needs_selinux_disabled() {
            "disable".to_string()
        } else {
            format!("type:{GUEST_SELINUX_TYPE}")
        }
    }

    /// Environment for the guest, so the usual tools find the right socket.
    ///
    /// `CONTAINER_HOST` is what Podman reads and `DOCKER_HOST` is what the
    /// Docker CLI and most libraries read. Setting both means a tool inside
    /// the desktop works without the user configuring anything.
    pub fn guest_env(self) -> Vec<(String, String)> {
        match self {
            Self::HostPodman => vec![
                (
                    "CONTAINER_HOST".into(),
                    format!("unix://{GUEST_PROXY_SOCKET}"),
                ),
                ("DOCKER_HOST".into(), format!("unix://{GUEST_PROXY_SOCKET}")),
            ],
            Self::HostDocker => {
                vec![("DOCKER_HOST".into(), format!("unix://{GUEST_PROXY_SOCKET}"))]
            }
            // Nested Podman uses its own socket inside the guest, so the
            // defaults are already right.
            Self::None | Self::Nested => Vec::new(),
        }
    }

    /// True when the guest needs its own container storage prepared.
    pub fn is_nested(self) -> bool {
        matches!(self, Self::Nested)
    }

    /// Devices the guest needs for this policy.
    ///
    /// Nested Podman sets up networking with pasta, which needs a tun device.
    /// Without it the inner container starts and then fails with "Failed to
    /// set up tap device in namespace".
    pub fn required_devices(self) -> &'static [&'static str] {
        match self {
            Self::Nested => &["/dev/net/tun"],
            _ => &[],
        }
    }

    /// True when the user must confirm a warning before this applies.
    pub fn needs_warning(self) -> bool {
        matches!(self, Self::HostPodman | Self::HostDocker)
    }

    /// The warning text shown before the setting applies. Returns `None` when
    /// no warning is needed.
    pub fn warning(self) -> Option<&'static str> {
        match self {
            Self::HostPodman => Some(
                "Exposing the host Podman socket gives this desktop the same power \
                 over your computer as the account that runs it. A process in the \
                 desktop can start a privileged container and read or change any \
                 file. Only enable this for desktops you fully trust.",
            ),
            Self::HostDocker => Some(
                "Exposing the host Docker socket gives this desktop root access to \
                 your computer. The Docker daemon runs as root, so a process in the \
                 desktop can take full control of the host. Only enable this for \
                 desktops you fully trust.",
            ),
            _ => None,
        }
    }
}

/// Everything the user can configure about a desktop.
///
/// This is `PartialEq` but not `Eq`, because the CPU limit is a float.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesktopConfig {
    pub name: String,
    pub image: String,
    #[serde(default)]
    pub mounts: Vec<Mount>,
    #[serde(default)]
    pub graphics: GraphicsMode,
    #[serde(default)]
    pub socket: SocketPolicy,
    /// Memory limit in megabytes. `None` means no limit.
    #[serde(default)]
    pub memory_mb: Option<u64>,
    /// CPU limit in whole cores. `None` means no limit.
    #[serde(default)]
    pub cpus: Option<f64>,
    /// Starting virtual monitor size. RDP resizes it after connecting.
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
    /// Which addresses the RDP port publishes on. Loopback by default.
    #[serde(default)]
    pub exposure: Exposure,
    /// Superseded by `exposure`, and read only so that a desktop created by
    /// an earlier version keeps its setting. `migrate` folds it in, and it is
    /// never written again.
    #[serde(default, rename = "expose_on_lan", skip_serializing)]
    pub legacy_expose_on_lan: bool,
}

fn default_width() -> u32 {
    1920
}
fn default_height() -> u32 {
    1080
}

/// Whether a desktop name can be used.
///
/// The name is not only a label. Podman uses it as the container name and
/// Cabin builds the volume names from it, so a name it rejects fails at create
/// time with a message about the request rather than about the name. Checking
/// here lets the dialogue say so before the user commits to it.
pub fn check_name(name: &str, taken: &[String]) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the desktop a name.".to_string());
    }
    if name.len() > 60 {
        return Err("That name is too long.".to_string());
    }
    // Podman requires the first character to be alphanumeric, and the rest to
    // be alphanumeric, underscore, dot or dash.
    let mut chars = name.chars();
    let first = chars.next().unwrap_or_default();
    if !first.is_ascii_alphanumeric() {
        return Err("The name must start with a letter or a number.".to_string());
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !c.is_ascii_alphanumeric() && !matches!(c, '_' | '.' | '-'))
    {
        if bad == ' ' {
            return Err("The name cannot contain spaces.".to_string());
        }
        return Err(format!("The name cannot contain '{bad}'."));
    }
    if taken.iter().any(|t| t.eq_ignore_ascii_case(name)) {
        return Err(format!("A desktop called {name} already exists."));
    }
    Ok(())
}

impl DesktopConfig {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            image: "localhost/cabin-guest:f44".to_string(),
            mounts: Vec::new(),
            graphics: GraphicsMode::default(),
            socket: SocketPolicy::default(),
            memory_mb: Some(4096),
            cpus: None,
            width: default_width(),
            height: default_height(),
            exposure: Exposure::default(),
            legacy_expose_on_lan: false,
        }
    }

    /// Folds a setting written by an earlier version into `exposure`.
    ///
    /// Call this after reading a configuration. Without it a desktop that was
    /// already open to the network would quietly fall back to loopback, and
    /// the user would find it unreachable with nothing to explain why.
    pub fn migrate(mut self) -> Self {
        if self.legacy_expose_on_lan && self.exposure == Exposure::Loopback {
            self.exposure = Exposure::Everywhere;
        }
        self.legacy_expose_on_lan = false;
        self
    }

    /// Checks every shared folder against the others and against the host
    /// socket, as well as on its own.
    ///
    /// Podman refuses a create request with two mounts on one destination.
    /// Checking the whole set here means a recreate can refuse the settings
    /// before it touches the container. It also covers a socket policy that
    /// changed after the shares were set.
    pub fn check_mounts(&self) -> Result<(), String> {
        let socket = self.socket.guest_socket_path();
        for (index, mount) in self.mounts.iter().enumerate() {
            let others: Vec<&str> = self
                .mounts
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .map(|(_, m)| m.guest_path.as_str())
                .chain(socket)
                .collect();
            Mount::check_guest_path_among(&mount.guest_path, &others)
                .map_err(|reason| format!("{}: {reason}", mount.guest_path.trim()))?;
        }
        Ok(())
    }

    /// Changing any of these needs the container to be recreated.
    pub fn needs_recreate(&self, other: &Self) -> bool {
        self.image != other.image
            || self.mounts != other.mounts
            || self.graphics != other.graphics
            || self.socket != other.socket
            || self.memory_mb != other.memory_mb
            || self.cpus != other.cpus
            || self.exposure != other.exposure
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes() -> Vec<(String, String)> {
        vec![
            ("/dev/dri/renderD128".into(), "nvidia".into()),
            ("/dev/dri/renderD129".into(), "i915".into()),
        ]
    }

    #[test]
    fn automatic_skips_nvidia() {
        let n = nodes();
        assert_eq!(
            GraphicsMode::Automatic.resolve(&n),
            Some("/dev/dri/renderD129")
        );
    }

    #[test]
    fn automatic_yields_nothing_when_only_nvidia() {
        let n = vec![("/dev/dri/renderD128".to_string(), "nvidia".to_string())];
        assert_eq!(GraphicsMode::Automatic.resolve(&n), None);
    }

    #[test]
    fn software_passes_no_device() {
        assert_eq!(GraphicsMode::Software.resolve(&nodes()), None);
    }

    #[test]
    fn only_the_host_options_bind_a_socket() {
        assert_eq!(SocketPolicy::None.guest_socket_path(), None);
        assert_eq!(SocketPolicy::Nested.guest_socket_path(), None);
        assert_eq!(
            SocketPolicy::HostPodman.guest_socket_path(),
            Some(GUEST_HOST_SOCKET)
        );
        assert_eq!(
            SocketPolicy::HostDocker.guest_socket_path(),
            Some(GUEST_HOST_SOCKET)
        );
    }

    #[test]
    fn nested_podman_needs_a_tun_device() {
        assert_eq!(SocketPolicy::Nested.required_devices(), &["/dev/net/tun"]);
        assert!(SocketPolicy::None.required_devices().is_empty());
        assert!(SocketPolicy::HostPodman.required_devices().is_empty());
    }

    #[test]
    fn only_the_host_options_relax_selinux() {
        assert!(SocketPolicy::HostPodman.needs_selinux_disabled());
        assert!(SocketPolicy::HostDocker.needs_selinux_disabled());
        assert!(!SocketPolicy::None.needs_selinux_disabled());
        assert!(!SocketPolicy::Nested.needs_selinux_disabled());
    }

    #[test]
    fn tools_are_pointed_at_the_proxy_not_the_raw_socket() {
        // The raw socket is owned by root inside the guest. Pointing tools at
        // it directly would give permission denied to the desktop user.
        for policy in [SocketPolicy::HostPodman, SocketPolicy::HostDocker] {
            for (_, value) in policy.guest_env() {
                assert!(
                    value.contains(GUEST_PROXY_SOCKET),
                    "{policy:?} should use the proxy, got {value}"
                );
            }
        }
    }

    #[test]
    fn the_podman_socket_also_answers_to_docker_host() {
        // Most tooling reads DOCKER_HOST even when talking to Podman, so
        // setting only CONTAINER_HOST would leave the docker CLI broken.
        let env = SocketPolicy::HostPodman.guest_env();
        assert!(env.iter().any(|(k, _)| k == "CONTAINER_HOST"));
        assert!(env.iter().any(|(k, _)| k == "DOCKER_HOST"));
    }

    #[test]
    fn the_safe_options_set_no_environment() {
        assert!(SocketPolicy::None.guest_env().is_empty());
        assert!(SocketPolicy::Nested.guest_env().is_empty());
    }

    #[test]
    fn host_sockets_warn_and_defaults_do_not() {
        assert!(SocketPolicy::HostDocker.needs_warning());
        assert!(SocketPolicy::HostPodman.needs_warning());
        assert!(!SocketPolicy::Nested.needs_warning());
        assert!(!SocketPolicy::None.needs_warning());
        assert!(SocketPolicy::None.warning().is_none());
    }

    fn wifi() -> Vec<NetInterface> {
        vec![
            NetInterface {
                name: "wlp3s0".into(),
                addresses: vec!["192.168.1.24".into(), "fd00::24".into()],
                loopback: false,
            },
            NetInterface {
                name: "docker0".into(),
                addresses: vec!["172.17.0.1".into()],
                loopback: false,
            },
        ]
    }

    #[test]
    fn accepts_an_ordinary_name() {
        assert!(check_name("fedora", &[]).is_ok());
        assert!(check_name("fedora-dev", &[]).is_ok());
        assert!(check_name("work_2", &[]).is_ok());
    }

    #[test]
    fn rejects_a_name_podman_would_refuse() {
        assert!(check_name("", &[]).is_err());
        assert!(check_name("-leading-dash", &[]).is_err());
        assert!(check_name("has space", &[]).is_err());
        assert!(check_name("slash/name", &[]).is_err());
    }

    #[test]
    fn rejects_a_name_already_in_use() {
        let taken = vec!["fedora".to_string()];
        assert!(check_name("fedora", &taken).is_err());
        assert!(check_name("fedora2", &taken).is_ok());
    }

    #[test]
    fn treats_a_name_that_differs_only_in_case_as_taken() {
        // Podman would accept both, and the two desktops would then be
        // impossible to tell apart in the sidebar.
        let taken = vec!["Fedora".to_string()];
        assert!(check_name("fedora", &taken).is_err());
    }

    #[test]
    fn ignores_surrounding_space() {
        assert!(check_name("  fedora  ", &[]).is_ok());
        assert!(check_name("   ", &[]).is_err());
    }

    #[test]
    fn default_is_loopback_only() {
        let c = DesktopConfig::new("test");
        assert_eq!(c.exposure.bind_addresses(&wifi()), vec!["127.0.0.1"]);
        assert_eq!(c.socket, SocketPolicy::None);
    }

    #[test]
    fn everywhere_binds_the_unspecified_address() {
        assert_eq!(
            Exposure::Everywhere.bind_addresses(&wifi()),
            vec!["0.0.0.0"]
        );
    }

    #[test]
    fn a_chosen_interface_binds_all_of_its_addresses() {
        let e = Exposure::Interfaces {
            names: vec!["wlp3s0".into()],
        };
        assert_eq!(
            e.bind_addresses(&wifi()),
            vec!["127.0.0.1", "192.168.1.24", "fd00::24"]
        );
    }

    #[test]
    fn choosing_interfaces_never_drops_loopback() {
        // Otherwise the desktop would stop being reachable from the computer
        // it runs on, which is never what choosing a network is meant to do.
        let e = Exposure::Interfaces { names: Vec::new() };
        assert_eq!(e.bind_addresses(&wifi()), vec!["127.0.0.1"]);
    }

    #[test]
    fn an_interface_that_has_gone_is_skipped() {
        // A network the computer has left must not stop the desktop starting.
        let e = Exposure::Interfaces {
            names: vec!["wlp3s0".into(), "tun0".into()],
        };
        assert_eq!(
            e.bind_addresses(&wifi()),
            vec!["127.0.0.1", "192.168.1.24", "fd00::24"]
        );
    }

    #[test]
    fn fixed_exposures_never_need_a_rebind() {
        let bound = vec!["192.0.2.9".to_string()];
        assert!(!Exposure::Loopback.needs_rebind(&bound, &wifi()));
        assert!(!Exposure::Everywhere.needs_rebind(&bound, &wifi()));
    }

    #[test]
    fn unchanged_addresses_need_no_rebind() {
        let e = Exposure::Interfaces {
            names: vec!["wlp3s0".into()],
        };
        // Order does not matter. Podman reports the mappings in its own order.
        let bound = vec![
            "fd00::24".to_string(),
            "127.0.0.1".to_string(),
            "192.168.1.24".to_string(),
        ];
        assert!(!e.needs_rebind(&bound, &wifi()));
    }

    #[test]
    fn a_changed_address_needs_a_rebind() {
        // The lease changed, so the old address is gone from the host and a
        // start would fail to bind it.
        let e = Exposure::Interfaces {
            names: vec!["wlp3s0".into()],
        };
        let bound = vec![
            "127.0.0.1".to_string(),
            "192.168.1.7".to_string(),
            "fd00::24".to_string(),
        ];
        assert!(e.needs_rebind(&bound, &wifi()));
    }

    #[test]
    fn a_new_address_needs_a_rebind() {
        // Without a recreate the desktop would stay unreachable on the
        // network the user chose.
        let e = Exposure::Interfaces {
            names: vec!["wlp3s0".into()],
        };
        assert!(e.needs_rebind(&["127.0.0.1".to_string()], &wifi()));
    }

    fn share(guest: &str) -> Mount {
        Mount {
            host_path: "/data".into(),
            guest_path: guest.into(),
            mode: crate::mount::MountMode::ReadOnly,
            relabel: true,
            kind: crate::mount::MountKind::Directory,
        }
    }

    #[test]
    fn two_shares_on_one_location_are_refused() {
        let mut c = DesktopConfig::new("test");
        c.mounts.push(share("/mnt/data"));
        c.mounts.push(share("/mnt/data/"));
        assert!(c.check_mounts().is_err());
    }

    #[test]
    fn distinct_shares_are_accepted() {
        let mut c = DesktopConfig::new("test");
        c.mounts.push(share("/mnt/data"));
        c.mounts.push(share("/home/cabin/work"));
        assert!(c.check_mounts().is_ok());
    }

    #[test]
    fn a_share_on_the_socket_location_is_refused() {
        let mut c = DesktopConfig::new("test");
        c.mounts.push(share(GUEST_HOST_SOCKET));
        // Refused with no socket as well, so that turning the socket on
        // later cannot make a saved configuration fail to create.
        assert!(c.check_mounts().is_err());
        c.socket = SocketPolicy::HostPodman;
        assert!(c.check_mounts().is_err());
    }

    #[test]
    fn a_share_on_a_volume_is_refused() {
        let mut c = DesktopConfig::new("test");
        c.mounts.push(share("/var/lib/cabin"));
        assert!(c.check_mounts().is_err());
        c.mounts[0] = share("/home/cabin");
        assert!(c.check_mounts().is_err());
    }

    #[test]
    fn only_loopback_keeps_the_desktop_private() {
        assert!(!Exposure::Loopback.reaches_other_devices());
        assert!(Exposure::Everywhere.reaches_other_devices());
        assert!(Exposure::Interfaces { names: Vec::new() }.reaches_other_devices());
    }

    #[test]
    fn an_exposure_change_recreates_the_container() {
        let a = DesktopConfig::new("test");
        let mut b = a.clone();
        b.exposure = Exposure::Everywhere;
        assert!(a.needs_recreate(&b));
    }

    #[test]
    fn resolution_change_alone_does_not_recreate() {
        let a = DesktopConfig::new("test");
        let mut b = a.clone();
        b.width = 2560;
        assert!(!a.needs_recreate(&b));
    }

    #[test]
    fn mount_change_forces_recreate() {
        let a = DesktopConfig::new("test");
        let mut b = a.clone();
        b.mounts.push(Mount {
            host_path: "/data".into(),
            guest_path: "/mnt/data".into(),
            mode: crate::mount::MountMode::ReadOnly,
            relabel: true,
            kind: crate::mount::MountKind::Directory,
        });
        assert!(a.needs_recreate(&b));
    }

    #[test]
    fn a_desktop_stays_selinux_confined() {
        // Not label=disable. The container keeps a type and keeps its own MCS
        // categories, so it stays separated from other containers.
        let option = SocketPolicy::None.selinux_label_option();
        assert_eq!(option, format!("type:{GUEST_SELINUX_TYPE}"));
        assert_ne!(option, "disable");
    }

    #[test]
    fn the_type_permits_the_nested_sandbox() {
        // container_t cannot mount the devpts that Fedora's image decoder
        // sandbox needs, and the desktop then shows no icons at all.
        assert_ne!(GUEST_SELINUX_TYPE, "container_t");
        assert_eq!(GUEST_SELINUX_TYPE, "container_engine_t");
    }

    #[test]
    fn exposing_a_host_socket_still_turns_labelling_off() {
        // The socket is labelled user_tmp_t and a confined container cannot
        // open it, so this one case needs more than a different type.
        for policy in [SocketPolicy::HostPodman, SocketPolicy::HostDocker] {
            assert_eq!(policy.selinux_label_option(), "disable", "{policy:?}");
        }
    }
}
