//! Per-VM configuration, stored as `config.json` in the VM directory.
//!
//! Fields serialize to camelCase (`memoryMB`, `diskSizeGB`, …) to keep
//! the on-disk schema aligned with virt-macos, so the two tools speak
//! the same config dialect wherever their feature sets overlap.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VmConfig {
    pub name: String,
    pub cpus: u32,
    /// Explicit rename: serde camelCase would emit `memoryMb`, but the
    /// virt-macos schema (Swift Codable) uses `memoryMB`.
    #[serde(rename = "memoryMB")]
    pub memory_mb: u32,
    #[serde(rename = "diskSizeGB")]
    pub disk_size_gb: u32,
    /// Human description of the VM's purpose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Stable MAC assigned at create time, so the guest keeps its
    /// network identity (and DHCP lease) across reboots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac_address: Option<String>,
    /// Root device for direct kernel boot (e.g. /dev/vda2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_device: Option<String>,
    /// Extra kernel command-line arguments for direct kernel boot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_kernel_args: Option<String>,
    /// "nat" (default when absent), "bridge", or "lan".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_mode: Option<String>,
    /// Host bridge for bridge mode (e.g. "br0"). Nil = driver default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_interface: Option<String>,
    /// "efi" (default when absent) or "bios" — recorded at install
    /// time (isoboot installs are BIOS installs: -kernel boots via
    /// SeaBIOS, so the installer writes GRUB to the MBR). For older
    /// VMs without the field, resolved by inspecting the disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<String>,
    /// True once the guest bootloader has been given a serial console
    /// (grubby via the offline console-enable pass). Gates the
    /// one-time auto-enable on first `virt start`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console_enabled: Option<bool>,
    /// VMs sharing a lan name share one virtual L2 segment (QEMU
    /// multicast-socket netdev) — the k3s-style cluster fabric.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lan_name: Option<String>,
}

impl VmConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let data = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        serde_json::from_str(&data).with_context(|| format!("cannot parse {}", path.display()))
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let data = serde_json::to_string_pretty(self).context("cannot encode config")?;
        std::fs::write(path, data).with_context(|| format!("cannot write {}", path.display()))
    }

    pub fn with_description(&self, description: &str) -> Self {
        let mut next = self.clone();
        next.description = Some(description.to_string());
        next
    }

    /// Short network descriptor for `virt list`: nat | bridge (br0) | lan:k3s.
    pub fn network_display(&self) -> String {
        match self.network_mode.as_deref() {
            Some("bridge") => {
                let iface = self.bridge_interface.as_deref().unwrap_or("default bridge");
                format!("bridge ({iface})")
            }
            Some("lan") => {
                let lan = self.lan_name.as_deref().unwrap_or("?");
                format!("lan:{lan}")
            }
            _ => "nat".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The on-disk schema must stay byte-compatible with virt-macos's
    /// config.json (Swift Codable keys) wherever fields overlap.
    #[test]
    fn schema_matches_virt_macos() {
        let cfg = VmConfig {
            name: "milford".into(),
            cpus: 4,
            memory_mb: 8192,
            disk_size_gb: 100,
            description: None,
            mac_address: None,
            root_device: None,
            extra_kernel_args: None,
            network_mode: None,
            bridge_interface: None,
            lan_name: None,
            firmware: None,
            console_enabled: None,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        for key in ["\"name\"", "\"cpus\"", "\"memoryMB\"", "\"diskSizeGB\""] {
            assert!(json.contains(key), "missing {key} in {json}");
        }
        // camelCase traps: these spellings must NOT appear.
        assert!(!json.contains("memoryMb"));
        assert!(!json.contains("diskSizeGb"));
    }

    #[test]
    fn network_display_modes() {
        let mut cfg = VmConfig {
            name: "n".into(),
            cpus: 1,
            memory_mb: 512,
            disk_size_gb: 1,
            description: None,
            mac_address: None,
            root_device: None,
            extra_kernel_args: None,
            network_mode: None,
            bridge_interface: None,
            lan_name: None,
            firmware: None,
            console_enabled: None,
        };
        assert_eq!(cfg.network_display(), "nat");
        cfg.network_mode = Some("lan".into());
        cfg.lan_name = Some("k3s".into());
        assert_eq!(cfg.network_display(), "lan:k3s");
        cfg.network_mode = Some("bridge".into());
        cfg.bridge_interface = Some("br0".into());
        assert_eq!(cfg.network_display(), "bridge (br0)");
    }
}
