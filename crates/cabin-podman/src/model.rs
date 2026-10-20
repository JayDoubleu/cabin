//! The parts of the libpod API payloads that Cabin reads and writes.

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

/// Reads a field that Podman sends as null when it is empty.
///
/// `#[serde(default)]` covers only a missing field, not an explicit null.
/// libpod sends `"Ports": null` for a container that publishes nothing and
/// `"Labels": null` for one with no labels. Without this, one such container
/// fails the decode of the whole list.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Deserialize)]
pub struct ContainerSummary {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Names", default, deserialize_with = "null_as_default")]
    pub names: Vec<String>,
    #[serde(rename = "State")]
    pub state: String,
    #[serde(rename = "Labels", default, deserialize_with = "null_as_default")]
    pub labels: HashMap<String, String>,
    #[serde(rename = "Ports", default, deserialize_with = "null_as_default")]
    pub ports: Vec<PortMapping>,
}

#[derive(Debug, Deserialize)]
pub struct PortMapping {
    #[serde(rename = "host_port", default)]
    pub host_port: u16,
    #[serde(rename = "container_port", default)]
    pub container_port: u16,
    /// How many consecutive ports the mapping covers, from `host_port`.
    #[serde(rename = "range", default)]
    pub range: u16,
}

/// Every host port that the listed containers publish, running or not.
///
/// A stopped container holds no socket, so a bind test passes on its port,
/// yet the two containers then cannot run together.
pub fn published_ports(containers: &[ContainerSummary]) -> Vec<u16> {
    containers
        .iter()
        .flat_map(|c| &c.ports)
        .filter(|p| p.host_port != 0)
        .flat_map(|p| {
            // Podman omits the range or sends 0 for a single port.
            let count = p.range.max(1);
            (0..count).filter_map(move |offset| p.host_port.checked_add(offset))
        })
        .collect()
}

/// The body of a libpod container create request.
#[derive(Debug, Serialize)]
pub struct CreateContainer {
    pub name: String,
    pub image: String,
    pub labels: HashMap<String, String>,
    pub env: HashMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<CreateMount>,
    /// Named volumes. These are NOT the same field as `mounts`, which carries
    /// OCI mounts whose source is a host path. Putting a volume name there
    /// makes the container create successfully and then fail to start with
    /// "No such device".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<NamedVolume>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<Device>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub portmappings: Vec<CreatePortMapping>,
    /// systemd runs as PID 1, so the container needs the systemd integration.
    pub systemd: String,
    /// The SELinux label for this container, as `--security-opt label=` takes
    /// it. See `cabin_core::GUEST_SELINUX_TYPE` for why it is not the default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selinux_opts: Option<Vec<String>>,
    /// The same label again, which Podman needs in order to keep it.
    ///
    /// `selinux_opts` alone is accepted and then discarded: the systemd
    /// integration rewrites the type to `container_init_t`, and a container
    /// running `/sbin/init` gets that integration whether it asks for it or
    /// not. The annotation is what survives, and it is what `podman run`
    /// itself sends beside the option. Sending one without the other silently
    /// produces the default label.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub annotations: std::collections::BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_limits: Option<ResourceLimits>,
    pub stop_signal: i32,
}

/// The annotation Podman reads the container's SELinux label back from.
///
/// `podman run` sends this beside `--security-opt label=`, and the label does
/// not stick without it.
pub const PODMAN_LABEL_ANNOTATION: &str = "io.podman.annotations.label";

#[derive(Debug, Serialize)]
pub struct CreateMount {
    pub destination: String,
    pub source: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub options: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct NamedVolume {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Dest")]
    pub dest: String,
    #[serde(rename = "Options")]
    pub options: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Device {
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct CreatePortMapping {
    pub host_ip: String,
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: String,
}

#[derive(Debug, Serialize, Default)]
pub struct ResourceLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<Memory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<Cpu>,
}

#[derive(Debug, Serialize)]
pub struct Memory {
    pub limit: i64,
}

#[derive(Debug, Serialize)]
pub struct Cpu {
    pub quota: i64,
    pub period: u64,
}

#[derive(Debug, Deserialize)]
pub struct CreateResponse {
    #[serde(rename = "Id")]
    pub id: String,
}

/// One entry from the libpod event feed.
#[derive(Debug, Deserialize)]
pub struct Event {
    #[serde(rename = "Type", default)]
    pub kind: String,
    #[serde(rename = "Action", default)]
    pub action: String,
    #[serde(rename = "Actor", default)]
    pub actor: Option<Actor>,
}

#[derive(Debug, Deserialize)]
pub struct Actor {
    #[serde(rename = "ID", default)]
    pub id: String,
    #[serde(rename = "Attributes", default, deserialize_with = "null_as_default")]
    pub attributes: HashMap<String, String>,
}

/// The parts of a container inspect response that Cabin reads.
#[derive(Debug, Deserialize)]
pub struct InspectResponse {
    #[serde(rename = "Id", default)]
    pub id: String,
    #[serde(rename = "Name", default)]
    pub name: String,
    #[serde(rename = "Config")]
    pub config: InspectConfig,
    #[serde(rename = "State", default)]
    pub state: Option<InspectState>,
    #[serde(rename = "HostConfig", default)]
    pub host_config: Option<InspectHostConfig>,
}

impl InspectResponse {
    /// The host addresses that the RDP port is published on.
    ///
    /// Read from the creation request, not from the live network state, so
    /// a stopped container reports them too. That is the case that matters:
    /// Podman binds these exact addresses when the container starts.
    pub fn bound_addresses(&self, container_port: u16) -> Vec<String> {
        let key = format!("{container_port}/tcp");
        self.host_config
            .as_ref()
            .and_then(|h| h.port_bindings.as_ref())
            .and_then(|bindings| bindings.get(&key))
            .and_then(|list| list.as_ref())
            .map(|list| list.iter().map(|b| b.host_ip.clone()).collect())
            .unwrap_or_default()
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct InspectHostConfig {
    /// Podman sends null rather than an empty map when nothing is published.
    #[serde(rename = "PortBindings", default)]
    pub port_bindings: Option<HashMap<String, Option<Vec<InspectPortBinding>>>>,
}

#[derive(Debug, Deserialize)]
pub struct InspectPortBinding {
    #[serde(rename = "HostIp", default)]
    pub host_ip: String,
}

/// One entry from the volume list.
#[derive(Debug, Deserialize)]
pub struct VolumeSummary {
    #[serde(rename = "Name")]
    pub name: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct InspectState {
    #[serde(rename = "Status", default)]
    pub status: String,
    #[serde(rename = "ExitCode", default)]
    pub exit_code: i32,
    #[serde(rename = "Error", default)]
    pub error: String,
}

#[derive(Debug, Deserialize)]
pub struct InspectConfig {
    #[serde(rename = "Env", default, deserialize_with = "null_as_default")]
    pub env: Vec<String>,
    #[serde(rename = "Labels", default)]
    pub labels: Option<HashMap<String, String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_bound_addresses_of_a_stopped_container() {
        let body = r#"{
            "Id": "abc",
            "Name": "fedora",
            "Config": {"Env": [], "Labels": null},
            "HostConfig": {"PortBindings": {
                "3389/tcp": [
                    {"HostIp": "127.0.0.1", "HostPort": "13389"},
                    {"HostIp": "fd00::24", "HostPort": "13389"}
                ]
            }}
        }"#;
        let inspect: InspectResponse = serde_json::from_str(body).unwrap();
        assert_eq!(inspect.bound_addresses(3389), vec!["127.0.0.1", "fd00::24"]);
    }

    #[test]
    fn a_list_entry_with_null_fields_still_reads() {
        // libpod sends null, not an empty list or map, for a container with
        // no published ports or no labels. Most containers on a host that
        // Cabin did not make look like this.
        let body = r#"[
            {"Id": "a", "Names": ["toolbox"], "State": "running",
             "Labels": null, "Ports": null},
            {"Id": "b", "Names": null, "State": "exited",
             "Labels": {"x": "y"},
             "Ports": [{"host_port": 13389, "container_port": 3389}]}
        ]"#;
        let list: Vec<ContainerSummary> = serde_json::from_str(body).unwrap();
        assert_eq!(list[0].names, vec!["toolbox"]);
        assert!(list[0].labels.is_empty());
        assert!(list[0].ports.is_empty());
        assert!(list[1].names.is_empty());
        assert_eq!(list[1].ports[0].host_port, 13389);
    }

    #[test]
    fn published_ports_cover_every_container_and_range() {
        // The second entry stands for a container whose labels Cabin cannot
        // read, such as one from a newer Cabin. Its port is still taken.
        let body = r#"[
            {"Id": "a", "Names": ["web"], "State": "running", "Labels": null,
             "Ports": [{"host_port": 8080, "container_port": 80, "range": 3}]},
            {"Id": "b", "Names": ["future"], "State": "exited",
             "Labels": {"io.github.jaydoubleu.cabin.managed": "true"},
             "Ports": [{"host_port": 13390, "container_port": 3389}]},
            {"Id": "c", "Names": ["plain"], "State": "exited", "Labels": null,
             "Ports": null}
        ]"#;
        let list: Vec<ContainerSummary> = serde_json::from_str(body).unwrap();
        assert_eq!(published_ports(&list), vec![8080, 8081, 8082, 13390]);
    }

    #[test]
    fn a_container_with_no_bindings_reports_none() {
        let body = r#"{"Config": {"Env": []}, "HostConfig": {"PortBindings": null}}"#;
        let inspect: InspectResponse = serde_json::from_str(body).unwrap();
        assert!(inspect.bound_addresses(3389).is_empty());
    }
}
