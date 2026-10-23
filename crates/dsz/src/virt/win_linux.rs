//! Native sysfs device identities behind the historical `virt::win` API.

use std::fs;
use std::path::{Path, PathBuf};

fn device_path(id: &str) -> Option<PathBuf> {
    let input = Path::new(id);
    let path = if input.starts_with("/dev") {
        let name = input.file_name()?.to_str()?;
        if name.starts_with("hidraw") {
            fs::canonicalize(Path::new("/sys/class/hidraw").join(name).join("device")).ok()?
        } else if name.starts_with("event") {
            fs::canonicalize(Path::new("/sys/class/input").join(name).join("device")).ok()?
        } else { return None; }
    } else {
        fs::canonicalize(input).ok()?
    };
    path.starts_with("/sys/devices").then_some(path)
}

pub fn instance_id_for_interface(path: &str) -> Option<String> {
    device_path(path).map(|p| p.to_string_lossy().into_owned())
}

#[cfg(test)]
pub fn interface_on_software_bus(path: &str) -> bool {
    device_path(path).is_some_and(|p| {
        p.starts_with("/sys/devices/virtual") || p.components().any(|c| {
            c.as_os_str().to_string_lossy().starts_with("vhci_hcd.")
        })
    })
}

pub fn descendant_ids(id: &str) -> Vec<String> {
    let Some(root) = device_path(id) else { return Vec::new() };
    let mut result = Vec::new();
    let mut pending = vec![(root, 0)];
    while let Some((path, depth)) = pending.pop() {
        if depth >= 12 { continue; }
        let Ok(entries) = fs::read_dir(path) else { continue };
        for entry in entries.filter_map(Result::ok) {
            // Sysfs parent/subsystem links form cycles. Only descend into real nodes.
            if !entry.file_type().is_ok_and(|t| t.is_dir()) { continue; }
            let path = entry.path();
            if path.join("uevent").is_file() {
                result.push(path.to_string_lossy().into_owned());
            }
            pending.push((path, depth + 1));
        }
    }
    result
}

/// A Linux event node is enumerated by the kernel, but games may hold an
/// already-open hidraw descriptor. This says nothing about effective hiding.
pub fn raw_input_lists(id: &str) -> bool {
    let Some(path) = device_path(id) else { return false };
    ["/sys/class/input", "/sys/class/hidraw"].iter().any(|root| {
        fs::read_dir(root).is_ok_and(|entries| entries.filter_map(Result::ok).any(|entry| {
            fs::canonicalize(entry.path().join("device")).is_ok_and(|p| p == path || p.starts_with(&path))
        }))
    })
}

/// The shared hider stores canonical application identities on both platforms.
pub fn dos_device_path(path: &str) -> Option<String> {
    fs::canonicalize(path).ok().map(|p| p.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_cannot_escape_sysfs() {
        assert!(instance_id_for_interface("/etc/passwd").is_none());
        assert!(instance_id_for_interface("/dev/null").is_none());
        assert!(!interface_on_software_bus("/etc"));
        assert!(descendant_ids("/etc").is_empty());
    }
}
