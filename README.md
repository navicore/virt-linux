# virt-linux

A CLI tool for managing Linux VMs on Linux hosts (x86_64 and aarch64)
via QEMU/KVM. Rust sibling of
[virt-macos](https://git.navicore.tech/navicore/virt-macos) — same command
surface, same per-VM on-disk model, same UX; where virt-macos embeds
Apple's Virtualization.framework, this tool supervises a QEMU
subprocess per VM.

The binary is installed as **`virt`** — the same name as virt-macos,
for muscle-memory parity across the two tools. VM state lives under
`~/.virt/vms/` — the same base path and layout as virt-macos, so
disks, kernels, and configs move between the two with plain copies.

**Status: M4 complete — full command surface.** `create`, `list`,
`set`, `delete`, `completions`, `doctor`, `start`, `stop`, `install`,
and `kernel-import` all work. Remaining roadmap items are
enhancements (guest-agent IP column, hostfwd SSH, cluster docs) in
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

`virt doctor` checks all runtime dependencies and prints
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
virt create milford --description "medium size vm with rocky 10 os" \
  --disk 100 --cpus 4 --memory 8192
virt create k3s-a --description "k3s control plane" --disk 20 --memory 4096 \
  --network lan:k3s
virt list
virt set milford --description "new purpose"
virt delete milford --force
virt doctor
```

Networking modes: `nat` (default; private per-VM slirp with internet),
`bridge` (VM directly on a host bridge; one-time host setup), and
`lan:NAME` — VMs sharing a lan name share one virtual L2 segment
(a QEMU multicast-socket fabric, no root needed) for k3s-style
clusters. Give lan nodes static IPs on the second NIC and point k3s at
them (`--node-ip <ip> --flannel-iface <eth1>`).

### Install an OS from ISO

```
virt install myvm --iso ~/Downloads/debian-13-amd64-netinst.iso
```

A remote-viewer window opens showing the VM's display; install the OS
as usual. Mismatched-architecture ISOs are rejected up front (the
marker scan from virt-macos, including its multi-arch and
stray-package-text handling). On headless hosts, pass `--no-viewer`
and connect manually with the printed `spice+unix://` URI.

Closing the viewer window requests a graceful guest shutdown (10s
cap, then forced — closing mid-install is a power cut). When the guest
powers off from inside (install finished), the session ends and the
viewer is dismissed.

**Clipboard sharing** between the host and the Linux guest works in
the install window once the SPICE agent runs in the guest:

```
apt install spice-vdagent
systemctl enable --now spice-vdagentd
```

### Start and stop

```
virt start myvm     # headless; guest console in this terminal
virt stop myvm      # from any terminal: graceful ACPI shutdown,
                          # then force kill after 10s
```

Boot mode is automatic: direct kernel when `kernel`+`initrd` are
imported, EFI/GRUB otherwise (OVMF NVRAM is seeded on first boot). A
force-killed VM can never be orphaned — QEMU dies with its
supervisor under any kill, including SIGKILL.

### Direct kernel boot (recommended)

Skip EFI/GRUB for daily use: boot the guest kernel directly. Console
output starts in ~1s and no GRUB configuration is needed.

Copy the kernel out of the guest during a GUI session (Debian/Ubuntu
provide stable `/vmlinuz` and `/initrd.img` symlinks):

```
virt install myvm --share ~/vm-share
# inside the guest:
 mkdir -p /mnt/share
 mount -t virtiofs share /mnt/share
 cp -L /vmlinuz /initrd.img /mnt/share/
```

Then on the host:

```
virt kernel-import myvm --from ~/vm-share
```

`virt start` detects the kernel and boots it directly. On x86_64
hosts bzImages import as-is; gzip/zboot arm64 wrappers are decompressed
automatically (in-process, like virt-macos). If your root filesystem
is not on `/dev/vda2`, pass `--root` (check with `lsblk` in the guest).
After a kernel upgrade in the guest, repeat the copy + import.

### Shared folders

Share a host directory with the VM (`--share` works with both `start`
and `install`; requires `virtiofsd` on the host):

```
virt start myvm --share ~/code
```

Inside the VM, mount it:

```
mkdir -p /mnt/share
mount -t virtiofs share /mnt/share
```

For persistent mounting, add to `/etc/fstab`:

```
share /mnt/share virtiofs defaults 0 0
```

`--version` reports the version. Shell completions:

```
source <(virt completions zsh)   # also: bash, fish
```

## Layout

Each VM is one directory — `~/.virt/vms/<name>/` holds
`config.json`, `disk.raw`, `nvram.bin`, and the runtime files
(`vm.pid`, `vm.lock`, `vm.log`, `qmp.sock`, …). `virt delete` is an
`rm -rf` of that directory; nothing else on the host is stateful.

## License

MIT — see [LICENSE](LICENSE).
