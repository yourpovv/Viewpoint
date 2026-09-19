

<div align="center">

# Viewpoint

**Self-hosted desktop stream**

Stream your full screen anyone, anywhere with a link. your PC to the viewer, the server only trades the handshake.

> Note: Works on localhost as-is. Hosting on a domain needs HTTPS (note that browsers refuse WebRTC on HTTP)

![License](https://img.shields.io/badge/license-MIT-blue.svg)
![Platform](https://img.shields.io/badge/platform-Windows-0078D6?logo=windows&logoColor=white)
![Go](https://img.shields.io/badge/Go-00ADD8?logo=go&logoColor=white)
![Rust](https://img.shields.io/badge/Rust-CE422B?logo=rust&logoColor=white)

</div>

https://github.com/user-attachments/assets/e70980f0-9cca-4771-968d-c76ff414839e

## Usage

1. Copy the config and set the secret: `Copy-Item viewpoint.example.yaml viewpoint.yaml`, then run `$env:VIEWPOINT_JWT_SECRET = "replace-me-a-long-string"`
2. Start the server: `go run ./server/cmd/viewpoint-server`
3. Start the client: `cargo run --release --manifest-path client/Cargo.toml`
4. Mint a link and open the `/v/...` URL it returns: `curl.exe -X POST -H "Authorization: Bearer $env:VIEWPOINT_JWT_SECRET" http://localhost:8080/api/links`

## Warning

Private links show everything on your screen. Only share links with people you trust.

## Notes

- The stream link can handle multiple viewers. Each tab gets its own session, while the host sends the same encoded frames to everyone.
- Leave `turn_url` empty if you want pure P2P. If you set `turn_url` and `VIEWPOINT_TURN_SECRET`, it’ll also work behind symmetric NATs
- Your upload bandwidth gets used once per viewer, so this is fine for a few private viewers, but probably not a alot
- Always use the release client. Debug builds only encode at around 1 FPS, while release runs at full frame rate. If things are still laggy, try lowering `width`, `height`, `fps`, or `bitrate_kbps` in `viewpoint.yaml`, then restart the client
- For a public domain, put HTTPS in front of it with something like Caddy, set `public_url` to your domain, and leave `control_url` local

## Download

Download from [Releases](https://github.com/yourpovv/Viewpoint/releases/tag/V1.0.0). once downloaded just configure **`viewpoint.yaml`** and run `viewport-server.exe` and `viewport-client.exe`

## Build

One command (builds both, packs `dist/` + the zip):

```powershell
powershell -ExecutionPolicy Bypass -File build.ps1
```

Output: `dist/viewpoint-server.exe` + `dist/viewpoint-client.exe` + `viewpoint-windows-amd64.zip`. Run both with `powershell -ExecutionPolicy Bypass -File run.ps1`

Dev builds need [Go](https://go.dev/) 1.22+ and [stable Rust](https://www.rust-lang.org/):

```bash
go build -o viewpoint-server ./server/cmd/viewpoint-server
cargo build --release --manifest-path client/Cargo.toml --features audio
```

## License

[MIT](LICENSE) c [YourPOVV](https://github.com/yourpovv)
