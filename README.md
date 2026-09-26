# virt-linux

A CLI tool for managing Linux VMs on Linux hosts (x86_64 and aarch64)
via QEMU/KVM. Rust sibling of
[virt-macos](https://git.navicore.tech/navicore/virt-macos) — same command
surface, same per-VM on-disk model, same UX; where virt-macos embeds
Apple's Virtualization.framework, this tool supervises a QEMU
subprocess per VM.

**Status: scaffold (M1).** `create`, `list`, `set`, `delete`,
`completions`, and `doctor` work. `install`, `start`, `stop`, and
`kernel-import` fail with pointers to the roadmap in
[docs/design/001-architecture.md](docs/design/001-architecture.md).

## Requirements

- Rust 1.85+ (pinned via `rust-toolchain.toml`)
- `qemu-system-x86_64` and/or `qemu-system-aarch64`
- edk2 firmware (OVMF/AAVMF) for EFI installs
- `/dev/kvm` for full-speed guests (TCG emulation works without it,
  slowly — including cross-arch guests, which virt-macos cannot do)
- `virt-viewer` (remote-viewer) for GUI installs — M3
- `virtiofsd` for `--share` — M4

`virt-linux doctor` diagnoses what is present.

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
