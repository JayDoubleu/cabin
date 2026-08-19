//! The parts of the libpod API payloads that Cabin reads and writes.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct ContainerSummary {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Names", default)]
    pub names: Vec<String>,
    #[serde(rename = "State")]
    pub state: String,
    #[serde(rename = "Labels", default)]
    pub labels: HashMap<String, String>,
    #[serde(rename = "Ports", default)]
    pub ports: Vec<PortMapping>,
}

#[derive(Debug, Deserialize)]
pub struct PortMapping {
    #[serde(rename = "host_port", default)]
    pub host_port: u16,
    #[serde(rename = "container_port", default)]
    pub container_port: u16,
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
    /// Turns off SELinux labelling for this container. Only set when a host
    /// container socket is bound in, because its label is not one a confined
    /// container may open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selinux_opts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_limits: Option<ResourceLimits>,
    pub stop_signal: i32,
}

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
    #[serde(rename = "Attributes", default)]
    pub attributes: HashMap<String, String>,
}

/// The parts of a container inspect response that Cabin reads.
#[derive(Debug, Deserialize)]
pub struct InspectResponse {
    #[serde(rename = "Config")]
    pub config: InspectConfig,
    #[serde(rename = "State", default)]
    pub state: Option<InspectState>,
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
    #[serde(rename = "Env", default)]
    pub env: Vec<String>,
}
