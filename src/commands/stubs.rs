//! Not-yet-implemented commands — each lands with its design milestone
//! (see docs/design/001-architecture.md roadmap). Present from day one
//! so the CLI surface and completions match virt-macos exactly.

use anyhow::{Result, bail};

const ROADMAP: &str = "docs/design/001-architecture.md";

pub fn kernel_import(name: &str, from: &str, root: Option<&str>) -> Result<()> {
    let _ = (name, from, root);
    bail!(
        "'virt kernel-import' is not implemented yet (M4 — decompress + direct boot).\n\
         See {ROADMAP}."
    )
}
