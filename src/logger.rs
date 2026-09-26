//! VM logging — timestamped lines appended to the per-VM `vm.log`,
//! mirroring virt-macos's VMLogger. Logging must never fail a VM
//! operation, so errors are swallowed.

use crate::vmdir::VmDir;
use std::io::Write;

pub fn log(dir: &VmDir, msg: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let line = format!("[{}.{:03}] {msg}\n", now.as_secs(), now.subsec_millis());
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.log_path())
    {
        let _ = file.write_all(line.as_bytes());
    }
}
