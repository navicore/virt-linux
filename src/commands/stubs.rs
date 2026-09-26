//! Not-yet-implemented commands — each lands with its design milestone
//! (see docs/design/001-architecture.md roadmap). Present from day one
//! so the CLI surface and completions match virt-macos exactly.

use anyhow::{Result, bail};

const ROADMAP: &str = "docs/design/001-architecture.md";

pub fn install(name: &str) -> Result<()> {
    let _ = name;
    bail!(
        "'virt install' is not implemented yet (M3 — GUI install via SPICE + remote-viewer).\n\
         See {ROADMAP}."
    )
}

pub fn start(name: &str) -> Result<()> {
    let _ = name;
    bail!(
        "'virt start' is not implemented yet (M2 — QEMU supervisor + QMP stop ladder).\n\
         See {ROADMAP}."
    )
}

pub fn stop(name: &str) -> Result<()> {
    let _ = name;
    bail!(
        "'virt stop' is not implemented yet (M2 — QMP system_powerdown + force kill).\n\
         See {ROADMAP}."
    )
}

pub fn kernel_import(name: &str, from: &str, root: Option<&str>) -> Result<()> {
    let _ = (name, from, root);
    bail!(
        "'virt kernel-import' is not implemented yet (M4 — decompress + direct boot).\n\
         See {ROADMAP}."
    )
}
