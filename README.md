# virt-linux

A CLI tool for managing Linux VMs on Linux hosts (x86_64 and aarch64)
via QEMU/KVM. Rust sibling of
[virt-macos](https://git.navicore.tech/navicore/virt-macos) — same command
surface, same per-VM on-disk model, same UX; where virt-macos embeds
Apple's Virtualization.framework, this tool supervises a QEMU
subprocess per VM.

**Status: M2 complete.** `create`, `list`, `set`, `delete`,
`completions`, `doctor`, `start`, and `stop` work — VMs boot headless
(EFI or direct kernel) with the console in your terminal and the full
graceful-stop ladder. `install` (GUI) and `kernel-import` fail with
pointers to the roadmap in
[docs/design/001-architecture.md](docs/design/001-architecture.md).

## Dependencies

### Runtime (host packages)

| What | Debian/Ubuntu/Pop | Fedora | Arch | Needed for |
|---|---|---|---|---|
| QEMU (x86_64 host) | `qemu-system-x86` | `qemu-kvm` | `qemu-system-x86_64` | M2 — running VMs |
| UEFI firmware (x86_64) | `ovmf` | `edk2-ovmf` | `edk2-ovmf` | M2 — EFI installs |
| `qemu-img` | `qemu-utils` | `qemu-img` | `qemu-tools` | disk tooling |
| `remote-viewer` | `virt-viewer` | `virt-viewer` | `virt-viewer` | M3 — GUI install window |
| `virtiofsd` | `virtiofsd` | `virtiofsd` | `virtiofsd` | M4 — `--share` |
| QEMU (aarch64, optional) | `qemu-system-arm` | `qemu-system-arm` | `qemu-system-aarch64` | cross-arch guests (TCG) |
| AAVMF firmware (optional) | `qemu-efi-aarch64` | `edk2-aarch64` | `edk2-aarch64` | aarch64 EFI installs |

On this class of host (Pop!_OS 24.04):

```
sudo apt install qemu-system-x86 qemu-utils ovmf virt-viewer virtiofsd
sudo usermod -aG kvm $USER   # then log out and back in
```

`/dev/kvm` access: the device is `0660 root:kvm`; membership in the
`kvm` group enables full-speed guests. Without it, VMs still run via
TCG emulation (slow, but works — including cross-arch, which virt-macos
cannot do at all).

Guest-side packages (installed inside VMs, documented per feature):
`qemu-guest-agent` (IP reporting for `virt list`, M5) and
`spice-vdagent` (clipboard in GUI mode, M3).

### Development

- Rust — any rustup install; the repo's `rust-toolchain.toml` pins the
  exact toolchain (1.98.1) and components
- `just` — recipe runner (`just ci` is the gate)
- `cargo-deny` — license audit (`cargo install cargo-deny --locked`)
- optional: `scc` and `cargo-modules` for `just stats`

`virt-linux doctor` checks all runtime dependencies and prints
distro-specific package names for anything missing.

## Build

```
just build
```

## CI

Run all CI checks locally before pushing:

```
just ci
```

This checks formatting (`cargo fmt --check`), lints
(`cargo clippy -D warnings`), audits dependency licenses
(`cargo deny check licenses` against `deny.toml`), runs tests, and
builds the release binary — the exact recipe set that CI runs. CI runs
on Forgejo Actions (`.forgejo/workflows/ci-linux.yml`, `navicore-rust`
runner) and calls `just ci` — the justfile is the single source of
truth, so local and CI can never drift.

## Usage

```
virt-linux create milford --description "medium size vm with rocky 10 os" \
  --disk 100 --cpus 4 --memory 8192
virt-linux create k3s-a --description "k3s control plane" --disk 20 --memory 4096 \
  --network lan:k3s
virt-linux list
virt-linux set milford --description "new purpose"
virt-linux delete milford --force
virt-linux doctor
```

Networking modes: `nat` (default; private per-VM slirp with internet),
`bridge` (VM directly on a host bridge; one-time host setup), and
`lan:NAME` — VMs sharing a lan name share one virtual L2 segment
(a QEMU multicast-socket fabric, no root needed) for k3s-style
clusters. Give lan nodes static IPs on the second NIC and point k3s at
them (`--node-ip <ip> --flannel-iface <eth1>`).

### Start and stop

```
virt-linux start myvm     # headless; guest console in this terminal
virt-linux stop myvm      # from any terminal: graceful ACPI shutdown,
                          # then force kill after 10s
```

Boot mode is automatic: direct kernel when `kernel`+`initrd` are
imported (M4), EFI/GRUB otherwise (OVMF NVRAM is seeded on first
boot). A force-killed VM can never be orphaned — QEMU dies with its
supervisor under any kill, including SIGKILL.

`--version` reports the version. Shell completions:

```
source <(virt-linux completions zsh)   # also: bash, fish
```

## Layout

Each VM is one directory — `~/.virt-linux/vms/<name>/` holds
`config.json`, `disk.raw`, `nvram.bin`, and the runtime files
(`vm.pid`, `vm.lock`, `vm.log`, `qmp.sock`, …). `virt delete` is an
`rm -rf` of that directory; nothing else on the host is stateful.

## License

MIT — see [LICENSE](LICENSE).
