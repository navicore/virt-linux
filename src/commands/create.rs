//! `virt create` — allocate a VM directory, config, and sparse disk.

use crate::cli::Create;
use crate::config::VmConfig;
use crate::mac;
use crate::vmdir::{VmDir, validate_name};
use anyhow::{Context, Result, bail};
use std::fs::OpenOptions;

pub fn run(args: &Create) -> Result<()> {
    let Create {
        name,
        description,
        disk,
        memory,
        cpus,
        network,
        bridge_interface,
    } = args;

    validate_name(name)?;
    if *cpus < 1 {
        bail!("--cpus must be at least 1");
    }
    if *memory < 512 {
        bail!("--memory must be at least 512 MB");
    }
    if *disk < 1 {
        bail!("--disk must be at least 1 GB");
    }

    let (network_mode, lan_name) = parse_network(network)?;
    if bridge_interface.is_some() && network_mode != "bridge" {
        bail!("--bridge-interface requires --network bridge");
    }

    let dir = VmDir::new(name);
    if dir.exists() {
        bail!("VM '{name}' already exists.");
    }

    // Stable MAC so the guest keeps its network identity (and DHCP
    // lease) across reboots.
    let mac = mac::random_locally_administered()?;

    dir.create()?;
    let result = (|| -> Result<()> {
        let config = VmConfig {
            name: name.clone(),
            cpus: *cpus,
            memory_mb: *memory,
            disk_size_gb: *disk,
            description: Some(description.clone()),
            mac_address: Some(mac.clone()),
            root_device: None,
            extra_kernel_args: None,
            network_mode: Some(network_mode.clone()),
            bridge_interface: bridge_interface.clone(),
            lan_name: lan_name.clone(),
        };
        config.write(&dir.config_path())?;

        // Allocate raw disk image — sparse, actual usage is near zero
        // until the guest writes.
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(dir.disk_path())
            .with_context(|| format!("cannot create {}", dir.disk_path().display()))?;
        file.set_len(u64::from(*disk) * 1024 * 1024 * 1024)
            .with_context(|| format!("cannot size {}", dir.disk_path().display()))?;
        Ok(())
    })();

    if let Err(e) = result {
        // Leave no half-created VM behind.
        let _ = dir.remove();
        return Err(e);
    }

    println!("Created VM '{name}'");
    println!("  Desc:   {description}");
    println!("  CPUs:   {cpus}");
    println!("  Memory: {memory} MB");
    println!("  Disk:   {disk} GB");
    println!("  MAC:    {mac}");
    println!(
        "  Net:    {}",
        network_display(&network_mode, lan_name.as_deref())
    );
    println!("  Path:   {}", dir.root.display());
    Ok(())
}

/// Split `nat` | `bridge` | `lan:NAME` into (mode, lan name).
fn parse_network(network: &str) -> Result<(String, Option<String>)> {
    if network == "nat" || network == "bridge" {
        return Ok((network.to_string(), None));
    }
    if let Some(lan) = network.strip_prefix("lan:") {
        anyhow::ensure!(
            !lan.is_empty() && !lan.contains(':'),
            "invalid lan name in --network {network}"
        );
        validate_name(lan).context("invalid lan name")?;
        return Ok(("lan".to_string(), Some(lan.to_string())));
    }
    bail!("--network must be 'nat', 'bridge', or 'lan:NAME'")
}

fn network_display(mode: &str, lan: Option<&str>) -> String {
    match (mode, lan) {
        ("lan", Some(lan)) => format!("lan:{lan} (shared L2 with other '{lan}' VMs)"),
        ("bridge", _) => "bridge (VM directly on the LAN)".to_string(),
        _ => "nat".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_parsing() {
        assert_eq!(parse_network("nat").unwrap(), ("nat".into(), None));
        assert_eq!(parse_network("bridge").unwrap(), ("bridge".into(), None));
        assert_eq!(
            parse_network("lan:k3s").unwrap(),
            ("lan".into(), Some("k3s".into()))
        );
        assert!(parse_network("lan:").is_err());
        assert!(parse_network("lan:a:b").is_err());
        assert!(parse_network("natted").is_err());
    }
}
