use crate::error::{Error, Result};
use crate::http;
use crate::model::*;
use cabin_core::{labels, Desktop, DesktopConfig, State, LABEL_MANAGED};
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
    pub async fn create_desktop(
        &self,
        config: &DesktopConfig,
        host_port: u16,
        render_nodes: &[(String, String)],
        password: &str,
    ) -> Result<String> {
        let mut env = HashMap::new();
        env.insert("CABIN_PASSWORD".to_string(), password.to_string());
        env.insert("CABIN_WIDTH".to_string(), config.width.to_string());
        env.insert("CABIN_HEIGHT".to_string(), config.height.to_string());
        env.insert("CABIN_RDP_PORT".to_string(), GUEST_RDP_PORT.to_string());

        // The home directory lives in a named volume, so reconfiguring a
        // desktop can recreate the container without destroying the user's
        // files. Podman seeds a new volume from the image on first use.
        let mut mounts = vec![CreateMount {
            destination: cabin_core::GUEST_HOME.to_string(),
            source: cabin_core::home_volume_name(&config.name),
            kind: "volume".to_string(),
            options: vec!["rw".to_string()],
        }];

        mounts.extend(config.mounts.iter().map(|m| {
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

        let devices = config
            .graphics
            .resolve(render_nodes)
            .map(|path| {
                vec![Device {
                    path: path.to_string(),
                }]
            })
            .unwrap_or_default();

        let resource_limits = build_limits(config);

        let request = CreateContainer {
            name: config.name.clone(),
            image: config.image.clone(),
            labels: labels::to_labels(config),
            env,
            mounts,
            devices,
            portmappings: vec![CreatePortMapping {
                host_ip: config.bind_address().to_string(),
                host_port,
                container_port: GUEST_RDP_PORT,
                protocol: "tcp".to_string(),
            }],
            systemd: "always".to_string(),
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
