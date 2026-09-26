//! Stable, locally-administered MAC addresses, generated from urandom.

use anyhow::{Context, Result};
use std::fs::File;
use std::io::Read;

/// Random locally-administered unicast MAC (`02:xx:xx:xx:xx:xx`),
/// matching what virt-macos's VZMACAddress.randomLocallyAdministered
/// produces at create time.
pub fn random_locally_administered() -> Result<String> {
    let mut file = File::open("/dev/urandom").context("cannot open /dev/urandom")?;
    let mut bytes = [0u8; 6];
    file.read_exact(&mut bytes).context("cannot read urandom")?;
    // Locally administered (bit 1) + unicast (bit 0 clear).
    bytes[0] = (bytes[0] & 0xFC) | 0x02;
    Ok(bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_is_locally_administered_and_well_formed() {
        for _ in 0..16 {
            let mac = random_locally_administered().unwrap();
            let parts: Vec<&str> = mac.split(':').collect();
            assert_eq!(parts.len(), 6, "{mac}");
            let first = u8::from_str_radix(parts[0], 16).unwrap();
            // bit 1 set (locally administered), bit 0 clear (unicast)
            assert_eq!(first & 0x03, 0x02, "{mac} not locally-administered unicast");
        }
    }
}
