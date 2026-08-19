use crate::error::{Error, Result};
use crate::http;
use crate::model::*;
use cabin_core::{
    labels, Desktop, DesktopConfig, NetInterface, SocketPolicy, State, LABEL_MANAGED,
};
use std::collections::HashMap;

/// The libpod API version prefix. Podman keeps this stable.
const API: &str = "/v5.0.0/libpod";

/// The guest listens on this port inside its own network namespace.
const GUEST_RDP_PORT: u16 = 3389;

/// systemd shuts down cleanly on SIGRTMIN+3, which is signal 37.
const SIGRTMIN_3: i32 = 37;

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

        Ok(summaries
            .into_iter()
            .filter_map(|c| {
                // A container whose labels will not parse is skipped rather
                // than failing the whole list. It may come from a newer Cabin.
                let config = labels::from_labels(&c.labels)
                    .map_err(|e| tracing::warn!("skipping container {}: {e}", c.id))
                    .ok()?;
                let rdp_port = c
                    .ports
                    .iter()
                    .find(|p| p.container_port == GUEST_RDP_PORT)
                    .map(|p| p.host_port);
                Some(Desktop {
                    name: c
                        .names
                        .first()
                        .cloned()
                        .unwrap_or_else(|| config.name.clone()),
                    id: c.id,
                    state: State::from_podman(&c.state),
                    config,
                    rdp_port,
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
            // key and nothing in the desktop session should be able to read it.
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
        if let Some(guest_path) = config.socket.guest_socket_path() {
            if let Some(host_path) = self.host_socket_for(config.socket) {
                mounts.push(CreateMount {
                    destination: guest_path.to_string(),
                    source: host_path,
                    kind: "bind".to_string(),
                    options: vec!["rw".to_string()],
                });
            } else {
                tracing::warn!(
                    "{:?} was requested but no such socket exists on this host",
                    config.socket
                );
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
            selinux_opts: config
                .socket
                .needs_selinux_disabled()
                .then(|| vec!["disable".to_string()]),
            resource_limits,
            stop_signal: SIGRTMIN_3,
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
            .ok_or_else(|| Error::NotFound(format!("no password recorded for {id}")))
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
        self.start(id).await?;

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

    pub async fn remove(&self, id: &str, force: bool) -> Result<()> {
        self.send(
            "DELETE",
            &format!("{API}/containers/{id}?force={force}&v=true"),
            None,
        )
        .await
        .map(|_| ())
    }

    /// Follows the container event feed and calls `on_event` for each entry.
    /// This never returns while the socket stays open.
    pub async fn watch_events<F>(&self, mut on_event: F) -> Result<()>
    where
        F: FnMut(Event),
    {
        let filters = urlencode(r#"{"type":["container"]}"#);
        http::stream_lines(
            &self.socket,
            &format!("{API}/events?stream=true&filters={filters}"),
            |line| match serde_json::from_str::<Event>(line) {
                Ok(event) => on_event(event),
                Err(e) => tracing::debug!("ignoring unparsable event: {e}"),
            },
        )
        .await
    }

    /// Finds the host socket for a policy, if it is actually there.
    ///
    /// A missing socket is not an error at create time. Reporting it as one
    /// would block a desktop from being made at all, when the user may simply
    /// not have started that service yet.
    fn host_socket_for(&self, policy: SocketPolicy) -> Option<String> {
        let candidate = match policy {
            SocketPolicy::HostPodman => self.socket.clone(),
            SocketPolicy::HostDocker => "/var/run/docker.sock".to_string(),
            SocketPolicy::None | SocketPolicy::Nested => return None,
        };
        std::path::Path::new(&candidate)
            .exists()
            .then_some(candidate)
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
fn extract_message(body: &[u8]) -> String {
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
