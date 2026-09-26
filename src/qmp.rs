//! Minimal QMP client: newline-delimited JSON over a unix socket.
//!
//! The whole protocol surface virt needs is `qmp_capabilities`,
//! `system_powerdown`, and `query-status` — smaller than any crate
//! that wraps it. Async events (SHUTDOWN, RESET, …) arrive
//! interleaved with command returns and are skipped.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

pub struct Qmp {
    stream: UnixStream,
    buf: Vec<u8>,
}

impl Qmp {
    pub fn connect(path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(path)
            .with_context(|| format!("cannot connect QMP socket {}", path.display()))?;
        let mut qmp = Self {
            stream,
            buf: Vec::new(),
        };
        let greeting = qmp.read_message()?;
        if greeting.get("QMP").is_none() {
            bail!("unexpected QMP greeting: {greeting}");
        }
        qmp.execute("qmp_capabilities", None)?;
        Ok(qmp)
    }

    /// ACPI power-button press. Guests translate this to a graceful
    /// shutdown; re-issuing periodically during the grace window
    /// mirrors virt-macos's retry-until-accepted `requestStop`.
    pub fn system_powerdown(&mut self) -> Result<()> {
        self.execute("system_powerdown", None).map(|_| ())
    }

    /// Guest run state, e.g. "running", "paused", "shutdown".
    pub fn status(&mut self) -> Result<String> {
        let ret = self.execute("query-status", None)?;
        Ok(ret
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string())
    }

    fn execute(&mut self, name: &str, args: Option<Value>) -> Result<Value> {
        let mut msg = json!({ "execute": name });
        if let Some(args) = args {
            msg["arguments"] = args;
        }
        let line = format!("{msg}\n");
        self.stream
            .write_all(line.as_bytes())
            .with_context(|| format!("QMP write failed: {name}"))?;
        loop {
            let resp = self.read_message()?;
            if let Some(err) = resp.get("error") {
                bail!("QMP error from {name}: {err}");
            }
            if let Some(ret) = resp.get("return") {
                return Ok(ret.clone());
            }
            // Async event — skip.
        }
    }

    /// Read one newline-terminated JSON message.
    fn read_message(&mut self) -> Result<Value> {
        loop {
            if let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=pos).collect();
                let text = String::from_utf8_lossy(&line).trim().to_string();
                if text.is_empty() {
                    continue;
                }
                return serde_json::from_str(&text).context("malformed QMP message");
            }
            let mut chunk = [0u8; 4096];
            let n = self.stream.read(&mut chunk).context("QMP socket closed")?;
            if n == 0 {
                bail!("QMP socket closed mid-message");
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::io::{BufRead, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;

    /// Scripted QMP server: greeting, then a return for each command,
    /// with an async event injected before every return to prove the
    /// client skips events.
    #[test]
    fn client_handles_greeting_events_and_commands() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("virt-qmp-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let (tx, rx) = mpsc::channel::<String>();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            writeln!(stream, r#"{{"QMP":{{"version":{{"qemu":8.2}}}}}}"#).unwrap();
            for _ in 0..3 {
                line.clear();
                reader.read_line(&mut line).unwrap();
                tx.send(line.trim().to_string()).unwrap();
                // Interleave an async event before the return.
                writeln!(stream, r#"{{"event":"RESET"}}"#).unwrap();
                writeln!(stream, r#"{{"return":{{"status":"running"}}}}"#).unwrap();
            }
        });

        let mut qmp = Qmp::connect(&path).unwrap();
        qmp.system_powerdown().unwrap();
        assert_eq!(qmp.status().unwrap(), "running");

        let cmds: Vec<Value> = rx
            .try_iter()
            .map(|l| serde_json::from_str(&l).unwrap())
            .collect();
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds[0]["execute"], "qmp_capabilities");
        assert_eq!(cmds[1]["execute"], "system_powerdown");
        assert_eq!(cmds[2]["execute"], "query-status");

        server.join().unwrap();
        let _ = std::fs::remove_file(&path);
    }
}
