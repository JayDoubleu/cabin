use crate::error::{Error, Result};
use crate::http;
use crate::model::*;
use cabin_core::{
    labels, Desktop, DesktopConfig, Exposure, NetInterface, SocketPolicy, State, LABEL_MANAGED,
};
use std::collections::HashMap;

/// The libpod API version prefix. Podman keeps this stable.
const API: &str = "/v5.0.0/libpod";

/// The guest listens on this port inside its own network namespace.
const GUEST_RDP_PORT: u16 = 3389;

/// systemd shuts down cleanly on SIGRTMIN+3, which is signal 37.
const SIGRTMIN_3: i32 = 37;

/// How long a recreate gives a running desktop to shut down.
const STOP_TIMEOUT_SECS: u32 = 20;

/// The host port a recreate uses when Podman reports none for the desktop.
const FALLBACK_RDP_PORT: u16 = 13389;

/// The suffix a recreate gives the old container while it builds the new one.
///
/// The old container is the only copy of the desktop's configuration and
/// password, because Cabin keeps no state of its own. So it is kept under
/// this name until the new container exists, and it comes back if the new
/// one cannot be made.
/// `check_name` refuses names with this ending, so no new desktop can take it.
const OLD_SUFFIX: &str = cabin_core::OLD_CONTAINER_SUFFIX;

/// The name the old container carries during a recreate.
fn old_name(name: &str) -> String {
    format!("{name}{OLD_SUFFIX}")
}

pub struct Podman {
    socket: String,
}

impl Podman {
    /// Connects to the rootless user socket.
    pub fn new() -> Self {
        let runtime = std::env::var("XDG_RUNTIME_DIR")
            .unwrap_or_else(|_| format!("/run/user/{}", nix_getuid()));
        Self {
            socket: format!("{runtime}/podman/podman.sock"),
        }
    }

    pub fn with_socket(socket: impl Into<String>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    pub fn socket_path(&self) -> &str {
        &self.socket
    }

    /// Confirms the socket answers. Call this at start-up so the user gets a
    /// clear message instead of a failure later.
    pub async fn ping(&self) -> Result<()> {
        self.get_raw("/_ping").await.map(|_| ())
    }

    /// Lists every container that carries the Cabin label.
    pub async fn list_desktops(&self) -> Result<Vec<Desktop>> {
        let filters = urlencode(&format!(r#"{{"label":["{LABEL_MANAGED}=true"]}}"#));
        let body = self
            .get_raw(&format!("{API}/containers/json?all=true&filters={filters}"))
            .await?;
        let summaries: Vec<ContainerSummary> = serde_json::from_slice(&body)?;
        let names: Vec<String> = summaries
            .iter()
            .filter_map(|c| c.names.first())
            .map(|n| n.trim_start_matches('/').to_string())
            .collect();

        Ok(summaries
            .into_iter()
            .filter_map(|c| {
                // A container whose labels will not parse is skipped rather
                // than failing the whole list. It may come from a newer Cabin.
                let config = labels::from_labels(&c.labels)
                    .map_err(|e| tracing::warn!("skipping container {}: {e}", c.id))
                    .ok()?;
                let container_name = c.names.first().map(|n| n.trim_start_matches('/'));
                let name = listed_name(container_name, &config.name, &names)?;
                let rdp_port = c
                    .ports
                    .iter()
                    .find(|p| p.container_port == GUEST_RDP_PORT)
                    .map(|p| p.host_port);
                Some(Desktop {
                    name,
                    id: c.id,
                    state: State::from_podman(&c.state),
                    config,
                    rdp_port,
                    started_at: c.started_at,
                })
            })
            .collect())
    }

    /// Creates a container for `config` and returns its id.
    ///
    /// `render_nodes` lists the DRM render nodes on the host as
    /// `(path, driver)`, which the graphics mode uses to choose a device.
    /// Creates a desktop container.
    ///
    /// `render_nodes` and `interfaces` are facts about the host. This crate
    /// reads none of them itself, so the caller supplies both and the domain
    /// model decides what to do with them.
    pub async fn create_desktop(
        &self,
        config: &DesktopConfig,
        host_port: u16,
        render_nodes: &[(String, String)],
        interfaces: &[NetInterface],
        password: &str,
    ) -> Result<String> {
        // Podman would refuse these too, but with a message about the
        // request rather than about the shared folder.
        config.check_mounts().map_err(Error::Settings)?;

        let mut env = HashMap::new();
        env.insert("CABIN_PASSWORD".to_string(), password.to_string());
        // Whatever the socket policy needs the guest to know.
        for (key, value) in config.socket.guest_env() {
            env.insert(key, value);
        }
        if config.socket.is_nested() {
            // Tells the guest's first boot to prepare rootless Podman storage.
            env.insert("CABIN_NESTED_PODMAN".to_string(), "1".to_string());
        }
        env.insert("CABIN_WIDTH".to_string(), config.width.to_string());
        env.insert("CABIN_HEIGHT".to_string(), config.height.to_string());
        env.insert("CABIN_RDP_PORT".to_string(), GUEST_RDP_PORT.to_string());

        // The home directory lives in a named volume, so reconfiguring a
        // desktop can recreate the container without destroying the user's
        // files. Podman seeds a new volume from the image on first use.
        let volumes = vec![
            NamedVolume {
                name: cabin_core::home_volume_name(&config.name),
                dest: cabin_core::GUEST_HOME.to_string(),
                options: vec!["rw".to_string()],
            },
            // Separate from the home volume, because it holds the TLS private
            // key. Its own volume keeps the certificate stable when a
            // recreate replaces the container, and keeps the key out of the
            // home volume, which the user shares and backs up. The session
            // user still owns the key, because gnome-remote-desktop runs as
            // that user and has to read it.
            NamedVolume {
                name: cabin_core::state_volume_name(&config.name),
                dest: cabin_core::GUEST_STATE.to_string(),
                options: vec!["rw".to_string()],
            },
        ];

        let mut mounts = Vec::from_iter(config.mounts.iter().map(|m| {
            let mut options = vec![m.mode.as_podman_option().to_string()];
            if m.relabel {
                options.push("z".to_string());
            }
            CreateMount {
                destination: m.guest_path.clone(),
                source: m.host_path.clone(),
                kind: "bind".to_string(),
                options,
            }
        }));

        // Bind the host container socket in, when the user asked for it and
        // accepted the warning. See DESIGN.md section 7.2.
        // The mount is always added when the policy names one, so the
        // environment and the SELinux label below always match a socket that
        // is really there.
        if let Some(guest_path) = config.socket.guest_socket_path() {
            if let Some(host_path) = self.host_socket_for(config.socket)? {
                mounts.push(CreateMount {
                    destination: guest_path.to_string(),
                    source: host_path,
                    kind: "bind".to_string(),
                    options: vec!["rw".to_string()],
                });
            }
        }

        let mut devices: Vec<Device> = config
            .graphics
            .resolve(render_nodes)
            .map(|path| {
                vec![Device {
                    path: path.to_string(),
                }]
            })
            .unwrap_or_default();
        for path in config.socket.required_devices() {
            devices.push(Device {
                path: (*path).to_string(),
            });
        }

        let resource_limits = build_limits(config);

        // Podman keeps the SELinux label only when the option and the
        // annotation agree, so they are built once and used twice.
        let label_option = config.socket.selinux_label_option();
        let request = CreateContainer {
            name: config.name.clone(),
            image: config.image.clone(),
            labels: labels::to_labels(config),
            env,
            mounts,
            volumes,
            devices,
            // One mapping per address. Choosing interfaces means binding
            // each of their addresses, rather than binding everything and
            // hoping a firewall covers the difference.
            portmappings: config
                .exposure
                .bind_addresses(interfaces)
                .into_iter()
                .map(|host_ip| CreatePortMapping {
                    host_ip,
                    host_port,
                    container_port: GUEST_RDP_PORT,
                    protocol: "tcp".to_string(),
                })
                .collect(),
            systemd: "always".to_string(),
            // Always set, because the default type cannot mount the devpts
            // that Fedora's image decoder needs and the desktop then has no
            // icons at all. See GUEST_SELINUX_TYPE. Both fields are needed:
            // the option alone is discarded by the systemd integration.
            selinux_opts: Some(vec![label_option.clone()]),
            annotations: std::collections::BTreeMap::from([(
                PODMAN_LABEL_ANNOTATION.to_string(),
                label_option,
            )]),
            resource_limits,
            stop_signal: SIGRTMIN_3,
            timezone: "local".to_string(),
            hostname: config.name.clone(),
        };

        let body = serde_json::to_vec(&request)?;
        let response = self
            .send("POST", &format!("{API}/containers/create"), Some(&body))
            .await?;
        let created: CreateResponse = serde_json::from_slice(&response)?;
        Ok(created.id)
    }

    /// Reads the RDP password for a desktop.
    ///
    /// The password lives in the container environment, because the guest
    /// needs it at boot. That means `podman inspect` reveals it. DESIGN.md
    /// section 13.4 records that the host keyring is where it belongs.
    pub async fn desktop_password(&self, id: &str) -> Result<String> {
        let body = self.get_raw(&format!("{API}/containers/{id}/json")).await?;
        let inspect: InspectResponse = serde_json::from_slice(&body)?;
        inspect
            .config
            .env
            .iter()
            .find_map(|entry| entry.strip_prefix("CABIN_PASSWORD="))
            .map(str::to_string)
            .ok_or_else(|| Error::PasswordMissing(id.to_string()))
    }

    /// Reads a container's inspect data, or `None` when it does not exist.
    async fn inspect(&self, id_or_name: &str) -> Result<Option<InspectResponse>> {
        match self
            .get_raw(&format!("{API}/containers/{}/json", urlencode(id_or_name)))
            .await
        {
            Ok(body) => Ok(Some(serde_json::from_slice(&body)?)),
            Err(Error::Api { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Recreates a desktop with a new configuration and returns the new id.
    ///
    /// Every setting needs this, because Podman fixes a container's
    /// configuration when it creates it. The named home and state volumes are
    /// attached by name, so the user's files and the pinned certificate
    /// survive.
    ///
    /// The old container holds the only copy of the configuration and the
    /// password, so it is not removed until the new one exists. It waits
    /// under a second name, and if Podman refuses the new configuration it
    /// gets its name back and starts again if it was running.
    pub async fn recreate_desktop(
        &self,
        desktop: &Desktop,
        updated: &DesktopConfig,
        render_nodes: &[(String, String)],
        interfaces: &[NetInterface],
        start_after: bool,
    ) -> Result<String> {
        // Refuse settings Podman would refuse, before anything changes.
        updated.check_mounts().map_err(Error::Settings)?;

        let name = desktop.config.name.clone();
        let password = self.desktop_password(&desktop.id).await?;
        let port = desktop.rdp_port.unwrap_or(FALLBACK_RDP_PORT);
        let was_running = desktop.state.is_running();

        // This can refuse, and it does not need the desktop stopped, so it
        // runs first. A refusal then leaves the desktop as it was.
        let temporary = old_name(&name);
        self.clear_leftover(&temporary, &desktop.id, &name).await?;

        if was_running {
            self.stop(&desktop.id, STOP_TIMEOUT_SECS).await?;
        }
        if let Err(e) = self.park(&desktop.id, &temporary).await {
            // Nothing has changed yet, so the desktop goes back to how it was.
            if was_running {
                if let Err(start) = self.start_desktop(&desktop.id, &name).await {
                    tracing::warn!("{name} did not start again: {start}");
                }
            }
            return Err(e);
        }

        let new_id = match self
            .create_desktop(updated, port, render_nodes, interfaces, &password)
            .await
        {
            Ok(id) => id,
            Err(refused) => {
                let mut message = refused.to_string();
                if let Err(e) = self.rename(&desktop.id, &name).await {
                    message.push_str(&format!(
                        "\n\nThe desktop is still there, but it is called {temporary} \
                         until it can be renamed: {e}"
                    ));
                } else if was_running {
                    if let Err(e) = self.start_desktop(&desktop.id, &name).await {
                        message.push_str(&format!("\n\nIt did not start again: {e}"));
                    }
                }
                return Err(Error::RecreateRefused { name, message });
            }
        };

        // The named volumes stay, which is what the recreate depends on.
        let removed = self.remove(&desktop.id, true).await;

        // The new desktop starts even when the old container stays behind.
        // That failure is only clutter, and a start that a rebind asked for
        // must still happen.
        let started = if start_after {
            self.start_desktop(&new_id, &name).await
        } else {
            Ok(())
        };

        if let Err(e) = removed {
            let mut message = e.to_string();
            if let Err(start) = started {
                message.push_str(&format!("\n\nThe desktop did not start: {start}"));
            }
            return Err(Error::OldContainerLeft {
                name,
                old_name: temporary,
                message,
            });
        }
        started.map_err(|e| Error::StartAfterRecreate(Box::new(e)))?;
        Ok(new_id)
    }

    /// Moves the old container to the recreate's temporary name, unless an
    /// earlier recreate that was cut short left it there.
    async fn park(&self, id: &str, temporary: &str) -> Result<()> {
        let current = self
            .inspect(id)
            .await?
            .map(|i| i.name.trim_start_matches('/').to_string())
            .unwrap_or_default();
        if current != temporary {
            self.rename(id, temporary).await?;
        }
        Ok(())
    }

    /// Removes a container left under a recreate's temporary name.
    ///
    /// Such a container is left when removing the old container failed after
    /// the new one was made, so it is safe to remove. Anything else under
    /// that name belongs to somebody else, and the recreate stops instead.
    async fn clear_leftover(&self, temporary: &str, desktop_id: &str, name: &str) -> Result<()> {
        let Some(leftover) = self.inspect(temporary).await? else {
            return Ok(());
        };
        if leftover.id == desktop_id {
            // The desktop itself, from a recreate that was cut short.
            return Ok(());
        }
        let ours = leftover
            .config
            .labels
            .as_ref()
            .and_then(|l| labels::from_labels(l).ok())
            .is_some_and(|config| config.name == name);
        if !ours {
            return Err(Error::Settings(format!(
                "A container called {temporary} already exists and Cabin did not \
                 make it. Rename or remove it, then apply the settings again."
            )));
        }
        self.remove(&leftover.id, true).await
    }

    /// Starts a desktop, recreating it first when the addresses of its
    /// chosen networks have changed. Returns the id, which changes when a
    /// recreate happens.
    ///
    /// Podman binds the addresses it was given at create time, so after a
    /// DHCP lease changes the start would fail. The interface names are what
    /// Cabin stores, and they are resolved again here.
    pub async fn start_with_current_addresses(
        &self,
        desktop: &Desktop,
        render_nodes: &[(String, String)],
        interfaces: &[NetInterface],
    ) -> Result<String> {
        if self.needs_rebind(desktop, interfaces).await? {
            return self
                .recreate_desktop(desktop, &desktop.config, render_nodes, interfaces, true)
                .await;
        }
        self.start_desktop(&desktop.id, &desktop.name).await?;
        Ok(desktop.id.clone())
    }

    /// Like [`Podman::start_with_current_addresses`], for a desktop that may
    /// be running.
    pub async fn restart_with_current_addresses(
        &self,
        desktop: &Desktop,
        render_nodes: &[(String, String)],
        interfaces: &[NetInterface],
    ) -> Result<String> {
        if self.needs_rebind(desktop, interfaces).await? {
            return self
                .recreate_desktop(desktop, &desktop.config, render_nodes, interfaces, true)
                .await;
        }
        self.restart(&desktop.id, STOP_TIMEOUT_SECS)
            .await
            .map_err(|e| explain_bind_failure(&desktop.name, e))?;
        Ok(desktop.id.clone())
    }

    async fn needs_rebind(&self, desktop: &Desktop, interfaces: &[NetInterface]) -> Result<bool> {
        // Only chosen networks resolve to addresses that can change. Asking
        // Podman for the others would cost a request on every start.
        if !matches!(desktop.config.exposure, Exposure::Interfaces { .. }) {
            return Ok(false);
        }
        let inspect = self
            .inspect(&desktop.id)
            .await?
            .ok_or_else(|| Error::NotFound(desktop.name.clone()))?;
        let bound = inspect.bound_addresses(GUEST_RDP_PORT);
        Ok(desktop.config.exposure.needs_rebind(&bound, interfaces))
    }

    pub async fn start(&self, id: &str) -> Result<()> {
        self.send("POST", &format!("{API}/containers/{id}/start"), None)
            .await
            .map(|_| ())
    }

    /// Starts a desktop and confirms it is still running a moment later.
    ///
    /// Podman reports success as soon as the container is handed to the
    /// runtime, so a guest whose systemd dies on the first millisecond looks
    /// like a successful start. Without this check the interface shows a
    /// desktop as running while it is already gone.
    pub async fn start_desktop(&self, id: &str, name: &str) -> Result<()> {
        self.start(id)
            .await
            .map_err(|e| explain_bind_failure(name, e))?;

        // Long enough for an immediate failure to have happened, short enough
        // not to be felt. A healthy guest reaches multi-user.target in about
        // 650 ms, but it is already "running" well before that.
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;

        let body = self.get_raw(&format!("{API}/containers/{id}/json")).await?;
        let inspect: InspectResponse = serde_json::from_slice(&body)?;
        let Some(state) = inspect.state else {
            return Ok(());
        };
        if state.status == "running" {
            return Ok(());
        }
        if is_bind_failure(&state.error) {
            return Err(Error::AddressGone {
                name: name.to_string(),
                message: state.error.clone(),
            });
        }

        Err(Error::StartedThenExited {
            name: name.to_string(),
            likely_cause: diagnose_early_exit(&state),
        })
    }

    /// Stops a desktop. systemd inside the guest shuts the session down
    /// cleanly, so allow it time rather than killing it.
    pub async fn stop(&self, id: &str, timeout_secs: u32) -> Result<()> {
        self.send(
            "POST",
            &format!("{API}/containers/{id}/stop?timeout={timeout_secs}"),
            None,
        )
        .await
        .map(|_| ())
    }

    pub async fn restart(&self, id: &str, timeout_secs: u32) -> Result<()> {
        self.send(
            "POST",
            &format!("{API}/containers/{id}/restart?timeout={timeout_secs}"),
            None,
        )
        .await
        .map(|_| ())
    }

    /// Removes a container.
    ///
    /// The named home and state volumes survive this. `v=true` removes only
    /// anonymous volumes. A recreate depends on that: it removes the old
    /// container and keeps the user's files and the pinned certificate. Use
    /// [`Podman::delete_desktop`] to remove a desktop for good.
    pub async fn remove(&self, id: &str, force: bool) -> Result<()> {
        self.send(
            "DELETE",
            &format!("{API}/containers/{id}?force={force}&v=true"),
            None,
        )
        .await
        .map(|_| ())
    }

    /// Removes a named volume. A volume that is already gone is not an error.
    pub async fn remove_volume(&self, name: &str) -> Result<()> {
        match self
            .send(
                "DELETE",
                &format!("{API}/volumes/{}?force=true", urlencode(name)),
                None,
            )
            .await
        {
            Ok(_) | Err(Error::Api { status: 404, .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Removes a desktop for good: the container, then its home and state
    /// volumes.
    ///
    /// The volumes go only after the container, because Podman will not
    /// remove a volume that a container still uses. Without this a new
    /// desktop with the same name would silently inherit the old files and
    /// the old certificate.
    ///
    /// `config_name` is the name in the desktop's configuration, which named
    /// the volumes when the desktop was created. It is not always the
    /// container's name: a container renamed outside Cabin keeps its old
    /// volumes, and a delete by the new name would miss them and say nothing.
    pub async fn delete_desktop(&self, id: &str, config_name: &str) -> Result<()> {
        // A container that is already gone is not an error, so a delete
        // that stopped part way can run again and finish.
        match self.remove(id, true).await {
            Ok(()) | Err(Error::Api { status: 404, .. }) => {}
            Err(e) => return Err(e),
        }
        let mut left = Vec::new();
        let mut messages = Vec::new();
        for volume in [
            cabin_core::home_volume_name(config_name),
            cabin_core::state_volume_name(config_name),
        ] {
            if let Err(e) = self.remove_volume(&volume).await {
                messages.push(e.to_string());
                left.push(volume);
            }
        }
        if left.is_empty() {
            return Ok(());
        }
        Err(Error::VolumesLeft {
            volumes: left.join(" and "),
            message: messages.join("\n"),
        })
    }

    /// Gives a container a new name. Named volumes are attached by name, so
    /// a rename does not affect them.
    pub async fn rename(&self, id: &str, new_name: &str) -> Result<()> {
        self.send(
            "POST",
            &format!("{API}/containers/{id}/rename?name={}", urlencode(new_name)),
            None,
        )
        .await
        .map(|_| ())
    }

    /// The name of every container on the host, Cabin's or not, and every
    /// host port those containers publish.
    ///
    /// Podman refuses a create whose name another container holds, and the
    /// refusal would come after the dialogue closed. The ports include those
    /// of stopped containers and of Cabin containers whose labels do not
    /// parse, which `list_desktops` leaves out. The caller uses this only to
    /// refuse names and ports, never to show the containers.
    pub async fn containers_in_use(&self) -> Result<(Vec<String>, Vec<u16>)> {
        let body = self
            .get_raw(&format!("{API}/containers/json?all=true"))
            .await?;
        let summaries: Vec<ContainerSummary> = serde_json::from_slice(&body)?;
        let ports = crate::model::published_ports(&summaries);
        let names = summaries
            .into_iter()
            .flat_map(|c| c.names)
            .map(|n| n.trim_start_matches('/').to_string())
            .collect();
        Ok((names, ports))
    }

    /// The name of every volume on the host.
    ///
    /// A desktop's volumes are named after it, so a volume left by an older
    /// version would hand its files to a new desktop with the same name.
    pub async fn volume_names(&self) -> Result<Vec<String>> {
        let body = self.get_raw(&format!("{API}/volumes/json")).await?;
        // Podman answers null rather than an empty list when there are none.
        let volumes: Option<Vec<VolumeSummary>> = serde_json::from_slice(&body)?;
        Ok(volumes
            .unwrap_or_default()
            .into_iter()
            .map(|v| v.name)
            .collect())
    }

    /// Follows the container event feed and calls `on_event` for each entry.
    ///
    /// This returns when the socket closes, for example when Podman restarts.
    /// The caller must reconnect, or changes made outside Cabin stop
    /// reaching it.
    ///
    /// `on_open` runs once Podman has accepted the request. From that point
    /// every change sends an event, so a caller that missed changes while
    /// it was not connected refreshes then.
    pub async fn watch_events<O, F>(&self, on_open: O, mut on_event: F) -> Result<()>
    where
        O: FnOnce(),
        F: FnMut(Event),
    {
        let filters = urlencode(r#"{"type":["container"]}"#);
        http::stream_request_opened(
            &self.socket,
            "GET",
            &format!("{API}/events?stream=true&filters={filters}"),
            None,
            on_open,
            |line| match serde_json::from_str::<Event>(line) {
                Ok(event) => on_event(event),
                Err(e) => tracing::debug!("ignoring unparsable event: {e}"),
            },
        )
        .await
    }

    /// Finds the host socket to bind into the guest for a policy.
    ///
    /// The Docker socket is not tested from here. Podman resolves a bind
    /// source on the host, and inside the Flatpak this process cannot see
    /// `/var/run` at all, so a test here would always fail. If the socket is
    /// absent, Podman refuses the create with an error the user sees.
    ///
    /// The Podman socket is the one this client talks to, so this process
    /// can see it. A missing one is an error rather than a skipped mount: a
    /// skip would leave the guest pointed at a socket with nothing behind it,
    /// and with SELinux labelling turned off for no reason.
    fn host_socket_for(&self, policy: SocketPolicy) -> Result<Option<String>> {
        match policy {
            SocketPolicy::HostPodman => {
                if std::path::Path::new(&self.socket).exists() {
                    Ok(Some(self.socket.clone()))
                } else {
                    Err(Error::HostSocketMissing {
                        path: self.socket.clone(),
                    })
                }
            }
            SocketPolicy::HostDocker => Ok(Some("/var/run/docker.sock".to_string())),
            SocketPolicy::None | SocketPolicy::Nested => Ok(None),
        }
    }

    async fn get_raw(&self, path: &str) -> Result<Vec<u8>> {
        self.send("GET", path, None).await
    }

    async fn send(&self, method: &str, path: &str, body: Option<&[u8]>) -> Result<Vec<u8>> {
        let response = http::request(&self.socket, method, path, body).await?;
        if response.status >= 400 {
            return Err(Error::Api {
                status: response.status,
                message: extract_message(&response.body),
            });
        }
        Ok(response.body)
    }
}

impl Default for Podman {
    fn default() -> Self {
        Self::new()
    }
}

/// Explains why a guest died on start-up, in terms the user can act on.
///
/// The inotify case is the one that actually happens. Every desktop runs
/// systemd, which needs several inotify instances, and Fedora's default limit
/// of 128 per user is reached after a handful of desktops. systemd then fails
/// with "Too many open files" and exits before it can log anything, so the
/// container dies with status 255 and an empty log. Nothing about that points
/// at inotify unless you already know.
fn diagnose_early_exit(state: &InspectState) -> String {
    if !state.error.is_empty() {
        return state.error.clone();
    }
    if state.exit_code == 255 {
        return "The most likely cause is that this computer has run out of \
                inotify instances, which every desktop needs. Raise the limit \
                with:\n\n    \
                sudo sysctl -w fs.inotify.max_user_instances=1024\n\n\
                To keep it after a restart, put \
                fs.inotify.max_user_instances=1024 in \
                /etc/sysctl.d/99-cabin.conf"
            .to_string();
    }
    format!("The desktop exited with status {}.", state.exit_code)
}

/// The name to list a Cabin container under, or `None` to hide it.
///
/// During a recreate the old container waits under `<name>-cabin-old` while
/// the new one is made. It carries the same labels, so without this the
/// desktop shows twice. If the recreate was cut short and nothing holds the
/// real name, the old container is the desktop, and it must stay visible
/// under the desktop's own name. The volumes and the pinned certificate are
/// keyed by that name, not by the container's.
///
/// A leftover is known by its labels: its configured name is the container
/// name without the suffix. A desktop that the user called `x-cabin-old` has
/// that whole string as its configured name, so it is a desktop in its own
/// right and stays visible.
fn listed_name(
    container_name: Option<&str>,
    config_name: &str,
    real_names: &[String],
) -> Option<String> {
    let Some(container_name) = container_name else {
        return Some(config_name.to_string());
    };
    match container_name.strip_suffix(OLD_SUFFIX) {
        Some(real) if real == config_name => {
            if real_names.iter().any(|n| n == real) {
                None
            } else {
                Some(config_name.to_string())
            }
        }
        _ => Some(container_name.to_string()),
    }
}

/// True when an error says that a published address could not be bound.
///
/// Podman binds the exact addresses it was given at create time. When an
/// address of a chosen network has gone, the start fails with one of these,
/// which names neither the network nor the setting that chose it.
///
/// Only EADDRNOTAVAIL means that. pasta and rootlessport both report it as
/// "cannot assign requested address". A port that another program holds
/// also fails "to bind", but with "address already in use", and applying
/// the settings again cannot help with that. So match the errno text only.
fn is_bind_failure(message: &str) -> bool {
    message
        .to_ascii_lowercase()
        .contains("cannot assign requested address")
}

/// Turns a bind failure into an error that names the setting to change.
fn explain_bind_failure(name: &str, error: Error) -> Error {
    match error {
        Error::Api { message, .. } if is_bind_failure(&message) => Error::AddressGone {
            name: name.to_string(),
            message,
        },
        other => other,
    }
}

fn build_limits(config: &DesktopConfig) -> Option<ResourceLimits> {
    let memory = config.memory_mb.map(|mb| Memory {
        limit: (mb * 1024 * 1024) as i64,
    });
    // Podman expresses a CPU limit as a quota over a period.
    const PERIOD: u64 = 100_000;
    let cpu = config.cpus.map(|c| Cpu {
        quota: (c * PERIOD as f64) as i64,
        period: PERIOD,
    });
    if memory.is_none() && cpu.is_none() {
        return None;
    }
    Some(ResourceLimits { memory, cpu })
}

/// Podman reports errors as `{"cause":..,"message":..,"response":..}`.
pub(crate) fn extract_message(body: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| String::from_utf8_lossy(body).trim().to_string())
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn nix_getuid() -> u32 {
    // SAFETY: getuid cannot fail and takes no arguments.
    unsafe { libc_getuid() }
}

extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_log_and_255_points_at_the_inotify_limit() {
        let state = InspectState {
            status: "exited".into(),
            exit_code: 255,
            error: String::new(),
        };
        let message = diagnose_early_exit(&state);
        assert!(message.contains("inotify"), "should name the real cause");
        assert!(
            message.contains("max_user_instances"),
            "should give the setting to change"
        );
    }

    #[test]
    fn a_reported_error_is_preferred_over_the_guess() {
        let state = InspectState {
            status: "exited".into(),
            exit_code: 1,
            error: "no such image".into(),
        };
        assert_eq!(diagnose_early_exit(&state), "no such image");
    }

    #[test]
    fn other_exit_codes_are_reported_plainly() {
        let state = InspectState {
            status: "exited".into(),
            exit_code: 3,
            error: String::new(),
        };
        assert!(diagnose_early_exit(&state).contains("status 3"));
    }

    #[test]
    fn a_lost_address_is_named_as_the_cause() {
        let refused = Error::Api {
            status: 500,
            message: "pasta failed: Failed to bind port 13389 (Cannot assign requested address)"
                .into(),
        };
        let explained = explain_bind_failure("fedora", refused).to_string();
        assert!(explained.contains("fedora"), "{explained}");
        assert!(explained.contains("apply them again"), "{explained}");
    }

    #[test]
    fn a_port_in_use_is_not_a_lost_address() {
        let refused = Error::Api {
            status: 500,
            message: "pasta failed: Failed to bind port 13389 (Address already in use)".into(),
        };
        assert!(matches!(
            explain_bind_failure("fedora", refused),
            Error::Api { .. }
        ));
        assert!(!is_bind_failure(
            "rootlessport listen tcp 0.0.0.0:13389: bind: address already in use"
        ));
    }

    #[test]
    fn other_start_errors_pass_through() {
        let refused = Error::Api {
            status: 500,
            message: "no such image".into(),
        };
        assert!(matches!(
            explain_bind_failure("fedora", refused),
            Error::Api { .. }
        ));
    }

    #[test]
    fn the_old_container_name_is_recognised() {
        // list_desktops hides the old container by this suffix while a
        // recreate runs, so the two must agree.
        assert!(old_name("fedora").ends_with(OLD_SUFFIX));
        assert_eq!(old_name("fedora").strip_suffix(OLD_SUFFIX), Some("fedora"));
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn a_leftover_is_hidden_while_the_desktop_exists() {
        let real = names(&["work", "work-cabin-old"]);
        assert_eq!(listed_name(Some("work-cabin-old"), "work", &real), None);
        assert_eq!(
            listed_name(Some("work"), "work", &real),
            Some("work".to_string())
        );
    }

    #[test]
    fn a_leftover_is_the_desktop_when_nothing_holds_its_name() {
        let real = names(&["work-cabin-old"]);
        assert_eq!(
            listed_name(Some("work-cabin-old"), "work", &real),
            Some("work".to_string())
        );
    }

    #[test]
    fn a_desktop_named_like_a_leftover_stays_visible() {
        // check_name refuses this ending, but a container made outside
        // Cabin, or by an older release, can still carry it. It must stay
        // visible as its own desktop.
        let real = names(&["work", "work-cabin-old"]);
        assert_eq!(
            listed_name(Some("work-cabin-old"), "work-cabin-old", &real),
            Some("work-cabin-old".to_string())
        );
    }

    #[test]
    fn encodes_filter_json() {
        assert_eq!(urlencode(r#"{"a":1}"#), "%7B%22a%22%3A1%7D");
    }

    #[test]
    fn extracts_podman_error_message() {
        let body = br#"{"cause":"x","message":"no such container","response":404}"#;
        assert_eq!(extract_message(body), "no such container");
    }

    #[test]
    fn falls_back_to_raw_text() {
        assert_eq!(extract_message(b"plain failure"), "plain failure");
    }

    #[test]
    fn builds_cpu_quota_from_cores() {
        let mut c = DesktopConfig::new("t");
        c.cpus = Some(2.5);
        c.memory_mb = None;
        let limits = build_limits(&c).unwrap();
        let cpu = limits.cpu.unwrap();
        assert_eq!(cpu.quota, 250_000);
        assert_eq!(cpu.period, 100_000);
    }

    #[test]
    fn no_limits_yields_none() {
        let mut c = DesktopConfig::new("t");
        c.cpus = None;
        c.memory_mb = None;
        assert!(build_limits(&c).is_none());
    }
}
