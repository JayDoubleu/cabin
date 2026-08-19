//! Finds the DRM render nodes on the host.
//!
//! Cabin shows the driver next to each node so the user can tell the Intel one
//! from the NVIDIA one. See DESIGN.md section 7.3.

use std::path::Path;

/// Returns `(device path, driver name)` for every render node found.
pub fn render_nodes() -> Vec<(String, String)> {
    let mut nodes = Vec::new();
    let Ok(entries) = std::fs::read_dir("/dev/dri") else {
        return nodes;
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("renderD") {
            continue;
        }
        let path = format!("/dev/dri/{name}");
        let driver = driver_for(&name).unwrap_or_else(|| "unknown".to_string());
        nodes.push((path, driver));
    }
    nodes.sort();
    nodes
}

/// Reads the driver name from sysfs by following the driver symlink.
fn driver_for(node: &str) -> Option<String> {
    let link = Path::new("/sys/class/drm").join(node).join("device/driver");
    std::fs::read_link(link)
        .ok()?
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_paths_under_dev_dri() {
        // The host running the tests may have no GPU, so only check the shape.
        for (path, driver) in render_nodes() {
            assert!(path.starts_with("/dev/dri/renderD"), "bad path {path}");
            assert!(!driver.is_empty());
        }
    }
}
