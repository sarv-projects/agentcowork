# BYO-host pack — the user-operated always-on executor node (P40.3)

The desktop stays the **control plane** (Guard-2, audit, memory, receipts).
This pack deploys the **executor node** — the same `agentcowork-core` binary in
the `--headless` profile — on hardware **you** own or rent, under **your own
credentials** (spec H33). Scheduled/background work (B7) runs 24/7 while your
laptop is off; the node attaches to your encrypted mesh (P8.9) and receipts
land back on the control plane.

## Hard rules (spec §8 + H33 — non-negotiable)

1. **No founder account ever.** Every template here provisions under YOUR
   login. We ship no rented fleet and no cloud-compute product.
2. **Guard-2 never leaves your device.** Approval-required steps park on the
   node as pending and surface on the control surface. Nothing here
   auto-approves.
3. **The node identity is the vault key.** `AGENTCOWORK_VAULT_KEY` is the
   SQLCipher key that unlocks the node's vault (its mesh identity + ledger
   key material). Set it via secrets/env files, never inline, never commit it.
4. **The mesh transport is E2E-encrypted** (X25519 + ChaCha20-Poly1305,
   P8.9). Prefer LAN / Tailscale / WireGuard; if you must expose a port
   publicly, require the bearer token + an IP allowlist.
5. **State lives on a mounted volume** (`AGENTCOWORK_HOME=/data`), never in the
   image. Audit NDJSON retention defaults to 7 days; keep the volume bounded.

## What ships here

| File | Purpose |
|---|---|
| `Dockerfile` | Multi-stage build (Bun-compiled coordinator + Rust core) → headless image |
| `docker-compose.yml` | One-command compose deploy with volume + healthcheck |
| `agentcowork-node.service` | systemd unit (Debian/Ubuntu VPS, Hetzner, Pi) |
| `com.agentcowork.node.plist` | launchd unit (macOS mini / old Mac) |
| `fly.toml` | Fly.io one-click template |
| this file | The per-provider guide below |

## Per-provider guides

### Fly.io
```bash
fly launch --image <your-registry>/agentcowork-node:local --copy-config
fly secrets set AGENTCOWORK_VAULT_KEY=<node key>
fly deploy
```
Pin the image digest and roll updates on your terms. Note the headless node has
**no updater of its own**: the auto-updater is the Tauri updater plugin inside
the desktop app (`docs/updating.md`), so on the node you redeploy the image
yourself.

### DigitalOcean (Droplet)
```bash
# Ubuntu 24.04 LTS droplet (min $6/mo class is fine for light schedules)
apt update && apt install -y docker.io
docker build -f deploy/Dockerfile -t agentcowork-node .
# or pull from your registry
mkdir -p /var/lib/agentcowork && useradd -r -d /var/lib/agentcowork agentcowork
echo 'AGENTCOWORK_VAULT_KEY=...' > /etc/agentcowork-node.env   # chmod 600
cp deploy/agentcowork-node.service /etc/systemd/system/
systemctl daemon-reload && systemctl enable --now agentcowork-node
```
Connect the droplet to your control plane over Tailscale; bind the sync port
to the tailnet IP only.

### AWS EC2
```bash
# t3.micro / t4g.small (ARM) Ubuntu 24.04, same systemd path as DO above.
# Security group: allow 47615/tcp only from your tailnet CIDR (or none if
# the node joins via Tailscale subnet routing).
```
Use an EC2 instance role / SSM rather than a long-lived SSH key pair if you
can; the node itself needs no AWS credentials at all.

### GCP
```bash
# e2-small (or e2-micro for light schedules) Container-Optimized OS or
# Ubuntu 24.04 with Docker. Same systemd/docker path as DO.
# VPC firewall: 47615/tcp restricted to your tailnet CIDR.
```

### Hetzner (CX22 / CPX11 class)
```bash
# Ubuntu 24.04 + Docker, exactly the DO path. Hetzner is the cheapest
# reliable always-on option in this class; a CX22 runs light schedules
# comfortably. Use the volume option for /data.
```

### Raspberry Pi / mini-PC (ARM)
```bash
# Pi 4/5 (arm64) or a used mini-PC. Docker or bare systemd:
#   stage 1 coordinator: bun build --compile (arm64) on a build host
#   stage 2 core:        cargo build --release -p agentcowork-core (arm64)
# Then install the systemd unit with the two binaries at /usr/local/bin.
# ARM note: use the arm64 builds; the x86_64 image won't run on the Pi.
```

## Verification checklist

1. `AGENTCOWORK_HOME` points at the mounted volume and is writable by the
   service user.
2. `AGENTCOWORK_VAULT_KEY` is set via secrets/env file with `chmod 600`.
3. The sync port (47615) is reachable from the control plane over the mesh,
   and ONLY over the mesh.
4. A scheduled task ticks on the node with the control laptop off: create the
   schedule on the desktop, close the lid, check the node's audit log shows
   the run and the receipt lands back on the desktop when it reconnects.
5. Guard-2-required steps appear as pending on the control surface — never
   auto-approved on the node.
