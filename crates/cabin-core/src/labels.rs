//! Converts a [`DesktopConfig`] to and from container labels.
//!
//! Podman holds all desktop state. Cabin keeps no database, so a desktop's
//! configuration travels with its container. See DESIGN.md section 5.1.

use crate::config::DesktopConfig;
use crate::{LABEL_MANAGED, LABEL_PREFIX};
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum LabelError {
    #[error("container is not managed by Cabin")]
    NotManaged,
    #[error("the {0} label is missing")]
    Missing(&'static str),
    #[error("could not parse the {label} label: {source}")]
    Parse {
        label: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

const LABEL_CONFIG: &str = "io.github.jaydoubleu.cabin.config";
const LABEL_VERSION: &str = "io.github.jaydoubleu.cabin.version";

/// The label schema version. Raise it when the format changes in a way that
/// older releases cannot read.
pub const SCHEMA_VERSION: &str = "1";

pub fn to_labels(config: &DesktopConfig) -> HashMap<String, String> {
    let mut labels = HashMap::new();
    labels.insert(LABEL_MANAGED.to_string(), "true".to_string());
    labels.insert(LABEL_VERSION.to_string(), SCHEMA_VERSION.to_string());
    // The whole configuration goes in one JSON label. Individual labels would
    // need escaping rules for lists, and would drift from the struct.
    labels.insert(
        LABEL_CONFIG.to_string(),
        serde_json::to_string(config).expect("DesktopConfig always serialises"),
    );
    // A human-readable copy, so `podman inspect` is useful without Cabin.
    labels.insert(format!("{LABEL_PREFIX}.name"), config.name.clone());
    labels
}

pub fn from_labels(labels: &HashMap<String, String>) -> Result<DesktopConfig, LabelError> {
    if labels.get(LABEL_MANAGED).map(String::as_str) != Some("true") {
        return Err(LabelError::NotManaged);
    }
    let raw = labels
        .get(LABEL_CONFIG)
        .ok_or(LabelError::Missing(LABEL_CONFIG))?;
    serde_json::from_str::<DesktopConfig>(raw)
        .map(DesktopConfig::migrate)
        .map_err(|source| LabelError::Parse {
            label: LABEL_CONFIG,
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mount::{Mount, MountMode};

    #[test]
    fn round_trips() {
        let mut original = DesktopConfig::new("fedora-dev");
        original.mounts.push(Mount {
            host_path: "/home/jay/code".into(),
            guest_path: "/mnt/code".into(),
            mode: MountMode::ReadWrite,
            relabel: true,
            kind: crate::mount::MountKind::Directory,
        });
        original.width = 2560;

        let restored = from_labels(&to_labels(&original)).unwrap();
        assert_eq!(original, restored);
    }

    #[test]
    fn an_older_exposed_desktop_keeps_its_exposure() {
        // Written by a version that only had a boolean.
        let json = r#"{"name":"old","image":"x","mounts":[],"width":1920,
                       "height":1080,"expose_on_lan":true}"#;
        let labels = HashMap::from([
            (LABEL_MANAGED.to_string(), "true".to_string()),
            (LABEL_CONFIG.to_string(), json.to_string()),
        ]);
        let config = from_labels(&labels).unwrap();
        assert_eq!(config.exposure, crate::config::Exposure::Everywhere);
    }

    #[test]
    fn an_older_private_desktop_stays_private() {
        let json = r#"{"name":"old","image":"x","mounts":[],"width":1920,
                       "height":1080,"expose_on_lan":false}"#;
        let labels = HashMap::from([
            (LABEL_MANAGED.to_string(), "true".to_string()),
            (LABEL_CONFIG.to_string(), json.to_string()),
        ]);
        let config = from_labels(&labels).unwrap();
        assert_eq!(config.exposure, crate::config::Exposure::Loopback);
    }

    #[test]
    fn rejects_unmanaged_containers() {
        let labels = HashMap::from([("some.other.label".to_string(), "x".to_string())]);
        assert!(matches!(from_labels(&labels), Err(LabelError::NotManaged)));
    }

    #[test]
    fn reports_bad_json_rather_than_panicking() {
        let labels = HashMap::from([
            (LABEL_MANAGED.to_string(), "true".to_string()),
            (LABEL_CONFIG.to_string(), "{not json".to_string()),
        ]);
        assert!(matches!(
            from_labels(&labels),
            Err(LabelError::Parse { .. })
        ));
    }
}
