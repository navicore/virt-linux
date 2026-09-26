//! `virt list` — all VMs, hardware, status, and network mode.

use crate::config::VmConfig;
use crate::lock::VmLock;
use crate::vmdir::VmDir;
use anyhow::Result;

pub fn run() -> Result<()> {
    let dirs = VmDir::all()
        .into_iter()
        .filter(|d| d.config_path().exists())
        .collect::<Vec<_>>();

    if dirs.is_empty() {
        println!("No VMs found.");
        return Ok(());
    }

    let headers = [
        "NAME",
        "CPUS",
        "MEMORY",
        "DISK",
        "STATUS",
        "NETWORK",
        "DESCRIPTION",
    ];
    let right_align = [false, true, true, true, false, false, false];

    let mut rows: Vec<Vec<String>> = Vec::new();
    for dir in &dirs {
        match VmConfig::load(&dir.config_path()) {
            Ok(c) => rows.push(vec![
                c.name.clone(),
                c.cpus.to_string(),
                format!("{} MB", c.memory_mb),
                format!("{} GB", c.disk_size_gb),
                status(dir),
                c.network_display(),
                truncate(c.description.as_deref().unwrap_or("—"), 40),
            ]),
            Err(_) => rows.push(vec![
                dir.name.clone(),
                "-".into(),
                "-".into(),
                "-".into(),
                "(corrupt config)".into(),
                "-".into(),
                "-".into(),
            ]),
        }
    }

    let widths: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(col, h)| {
            rows.iter()
                .map(|r| r[col].len())
                .chain([h.len()])
                .max()
                .unwrap()
        })
        .collect();

    fn line(cells: &[String], widths: &[usize], right: &[bool]) -> String {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let pad = " ".repeat(widths[i].saturating_sub(c.len()));
                if right[i] {
                    format!("{pad}{c}")
                } else {
                    format!("{c}{pad}")
                }
            })
            .collect::<Vec<_>>()
            .join("  ")
    }

    let header_cells: Vec<String> = headers.iter().map(|s| s.to_string()).collect();
    let header_line = line(&header_cells, &widths, &right_align);
    println!("{header_line}");
    println!("{}", "-".repeat(header_line.len()));
    for row in &rows {
        println!("{}", line(row, &widths, &right_align));
    }
    Ok(())
}

/// Running state comes from the flock, not the PID file — the lock is
/// released by the kernel when the supervisor dies, so it can't lie.
fn status(dir: &VmDir) -> String {
    if !VmLock::is_locked(dir) {
        return "stopped".into();
    }
    match dir.pid() {
        Some(pid) => format!("running (PID {pid})"),
        None => "running".into(),
    }
}

/// Descriptions are free text; cap the column so one long string can't
/// blow out the table.
fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    let cut: String = value.chars().take(width.saturating_sub(1)).collect();
    format!("{cut}…")
}
