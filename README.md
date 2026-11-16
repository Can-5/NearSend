# NearSend

Send files directly between devices on the same local network. No cloud, no accounts, no size
limits enforced by anyone but you.

## How it works

1. NearSend broadcasts a tiny **presence beacon** over UDP `44123` every 2 seconds so other
   NearSend instances on the LAN can see this device.
2. Devices appear in **Nearby devices**. Pick one, drop files, press **Send**.
3. The other device shows an **approval card** with every filename and size. Nothing is
   written to disk until they press **Accept**.
4. Files stream over TCP `44124` with a per-file **SHA-256 checksum verified on arrival**.
   Corrupted or interrupted transfers are deleted automatically.
5. If auto-discovery doesn't work (guest networks, VLANs), paste the other device's
   **invite code** (`nearsend://192.168.1.20:44124`) — it's shown with a QR code for convenience.

## Security & privacy

- Files never leave your local network; nothing is uploaded anywhere.
- **Receiver approval is mandatory** — unknown senders cannot make your device write files.
- Filenames are sanitized to **basename only**; path separators, `..`, control characters and
  trailing dots are rejected, so a malicious sender can't escape your save folder.
- Every received file is verified against the sender's SHA-256; mismatches are deleted.
- Incoming headers are capped (64 KB header, 50 files, 2 GiB per file, 4 GiB per transfer).
- Partial files from cancelled/failed transfers are removed.
- No telemetry, no analytics, no account. Transfer history stays on each device.

NearSend is a convenience tool, not a hardened perimeter. Both peers authenticate by
"network adjacency + explicit approval", not cryptography — don't use it on untrusted Wi-Fi.

## Install

```bash
pnpm install
pnpm tauri build
```

Linux build deps: `libwebkit2gtk-4.1-dev`, `libappindicator3-dev`, `librsvg2-dev`, `patchelf`,
plus Rust ≥ 1.77 and Node ≥ 20.

## Development

```bash
pnpm dev        # vite dev server
pnpm tauri dev  # app with hot reload
pnpm test       # vitest (formatting helpers)
cargo test      # in src-tauri — protocol/store unit tests (no network)
pnpm tauri build# native bundles
```

## Ports

| Port | Proto | Purpose |
|---|---|---|
| 44123 | UDP | presence beacon (broadcast) |
| 44124 | TCP | transfer listener |

Both must be reachable on your LAN; allow NearSend through the OS firewall if prompted.

## Platform notes

- **Linux**: broadcast discovery works on most networks; some Wi-Fi access points block client
  isolation — use "Add by code" there.
- **Windows / macOS**: firewall may ask to allow NearSend on private networks — you want yes.
- Discovery ignores this device's own beacon; devices disappear from the list ~30 s after
  they stop broadcasting.

## License

MIT. See `LICENSE`.
