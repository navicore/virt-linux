# Design: QEMU/KVM architecture for virt-linux

**Date**: 2026-09-25
**Status**: M1 scaffold implemented; M2+ roadmap below

## Intent

Reimplement the virt-macos CLI on Linux (x86_64 and aarch64) with the
same command surface, the same per-VM on-disk model, and the same UX —
using open-source virtualization (QEMU/KVM) instead of Apple's
Virtualization.framework. virt-macos's README and `docs/design/` are
the porting spec; this document records what changes and why.

## Constraints

- **No daemons** — no libvirt, no system-wide state. Each VM is one
  QEMU subprocess plus (later) one virtiofsd subprocess. Everything a
  VM owns lives in `~/.virt/vms/<name>/`
- **Schema parity where feature sets overlap** — `config.json` uses
  the same camelCase fields as virt-macos, plus `lanName` (new)
- **Same lock contract** — flock on `vm.lock` held for the supervisor's
  life; kernel-released on death; `isLocked` is the authority for
  "running", PID files are advisory display sugar
- **justfile is the single source of truth** — CI (Forgejo Actions on
  the `navicore-rust` runner) calls `just ci`; local and CI are
  byte-identical invocations
- **Pure-function argv builder** — host-dependent choices (accel,
  firmware paths, display mode) are inputs; the QEMU command line is
  pinned by unit tests the way VMConfiguration.swift pins the VZ
  contract

## Approach

### VM lifecycle

`virt start` becomes a QEMU supervisor: spawn `qemu-system-<arch>` with
inherited stdio (`-serial stdio` gives byte-transparent console
passthrough) and a per-VM QMP unix socket. The supervisor — not QEMU —
owns signal handling: SIGINT/SIGTERM translate to QMP
`system_powerdown`, with the same 10s graceful-then-force ladder as
virt-macos's Stop. The child runs in its own session (setsid) so
terminal Ctrl-C reaches only the supervisor.

### Feature mapping

| virt-macos | virt-linux |
|---|---|
| VZVirtualMachine in-process | qemu-system subprocess, supervised |
| VZEFIBootLoader + NVRAM store | edk2 pflash (OVMF/AAVMF), per-VM nvram.bin |
| VZLinuxBootLoader (direct kernel) | `-kernel/-initrd/-append console=ttyS0` |
| VZVirtualMachineView GUI window | SPICE unix socket + remote-viewer |
| spice-vdagent clipboard | same agent, SPICE on the host side |
| VZNATNetworkDeviceAttachment | `-netdev user` (slirp; no DF/MTU pathology) |
| VZBridgedNetworkDeviceAttachment | `-netdev bridge` (one-time host setup) |
| virtiofs directory share | virtiofsd daemon (M4) |
| (nothing) | dual-NIC `lan:` cluster fabric (below) |

### Networking modes

- `nat` — per-VM slirp. Guest is 10.0.2.15 with internet; instances
  are isolated from each other. Slirp terminates TCP in userspace and
  re-originates from the host stack, so Apple-NAT's DF-stripping MTU
  black hole does not exist here.
- `bridge` — guest directly on a host bridge; needs CAP_NET_ADMIN-ish
  one-time host setup (macvtap or qemu-bridge-helper). Same
  "infrastructure friction, different mechanism" as the macOS
  restricted entitlement.
- `lan:NAME` — the k3s cluster fabric. Dual NIC: eth0 stays private
  slirp (internet, no identity), eth1 joins a shared L2 segment built
  from a QEMU multicast-socket netdev. The multicast endpoint is
  FNV-1a-derived from the lan name, deterministic across VMs and
  reboots. No root, no bridge, no daemon. Nodes get static IPs on eth1
  (e.g. 10.100.0.11/12/13) and k3s runs with `--node-ip
  <eth1-ip> --flannel-iface eth1`. The second NIC's MAC is the primary
  MAC +1 on the last octet.

### Guest agent

The argv always carries a qemu-guest-agent channel (virtio-serial +
unix socket). Guests that install `qemu-guest-agent` report
ground truth — IPs for `virt list`, `guest-ping` for doctor, graceful
`guest-shutdown` as a `virt stop` upgrade. virt-macos has no
equivalent channel; this is a Linux-only capability.

### Arch dispatch

`Arch::host()` picks the QEMU binary (`qemu-system-x86_64` /
`-aarch64`), machine (`q35` / `virt`), video (`virtio-vga` /
`virtio-gpu-pci`), and firmware candidates. Same-arch guests run under
KVM; cross-arch guests fall back to TCG (slow, but impossible on
virt-macos). `virt doctor` reports what is actually installed.

## Roadmap

- **M1 (done)** — cargo project, CI parity (justfile + Forgejo),
  config/vmdir/lock/mac, argv builder with tests, create/list/set/
  delete/completions/doctor
- **M2 (done)** — `virt start`/`virt stop`: QEMU supervisor (setsid,
  inherited stdio, PDEATHSIG orphan protection, signal → QMP
  `system_powerdown` translation with 10s-then-SIGKILL ladder),
  minimal QMP client, EFI NVRAM seeding from the distro template on
  first boot, tty save/restore + repair-after-force-kill
- **M3 (done)** — `virt install`: SPICE unix socket + remote-viewer
  spawn (with `--no-viewer` for headless hosts), ISO attach, ISO arch
  detection (ISOCheck port incl. multi-arch and stray-text handling),
  window-close → graceful shutdown semantics (InstallerApp parity),
  viewer dies with supervisor (PDEATHSIG SIGTERM)
- **M4 (done)** — `virt kernel-import` (in-process gzip decompression
  for zboot kernels; bzImages import as-is on x86_64), direct kernel
  boot live-validated, `--share` via virtiofsd (memfd/NUMA shared
  memory + vhost-user-fs, daemon supervised with PDEATHSIG, socket
  readiness wait, clean error when virtiofsd is missing)
- **M5** — guest-agent-backed IP column in `virt list`, hostfwd SSH
  port allocation, cluster docs (k3s walkthrough), and `virt
  console-enable <vm>`: perform the one-time guest serial-console
  setup (grubby or grub.cfg edit) automatically over the guest-agent
  channel, so the last manual step of a fresh install disappears

## Open questions

- qcow2 vs raw for the main disk (raw chosen for parity; qcow2 buys
  snapshots/less host fragmentation — revisit at M2)
- Should `virt create` copy the firmware vars template eagerly (so a
  later package upgrade can't change an existing VM's NVRAM)?
