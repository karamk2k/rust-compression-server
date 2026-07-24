# Live streaming

The first release intentionally uses one 1080p rendition. OBS must encode H.264
video and AAC audio; the server does not transcode.

## OBS settings

- Service: Custom
- Server: `rtmp://YOUR_HOST/live`
- Stream key: copy the one-time value from `/admin/streams`
- Output resolution: 1920x1080
- Frame rate: 30 FPS
- Rate control: CBR
- Video bitrate: 4500 Kbps
- Keyframe interval: 2 seconds
- Audio: AAC, 160 Kbps

## Deployment

1. Build the frontend with `cd frontend && npm ci && npm run build`.
2. Build the Rust service with `cargo build --release`.
3. Ensure `/var/www/hls` exists and is writable by nginx.
4. Merge `docs/nginx-live.conf` into the nginx configuration.
5. Serve the public site over HTTPS.
6. Run `nginx -t`, reload nginx, and restart the Rust service.
7. Open `/admin/streams`, create a stream, and copy the OBS key.
8. Create a viewer link and open it in a private browser window.

The `live_viewer` cookie contains a viewing capability, so HTTPS is mandatory in
production. Viewer and publisher secrets are stored only as SHA-256 hashes.
Rotating an OBS key invalidates the old key immediately. Revoking a viewer key
blocks its playlist and segment requests immediately.

For local development, Rust serves protected HLS files from `/var/www/hls`
directly on port 8080. Override that directory with `LIVE_HLS_DIR` if needed.
Production should let nginx serve the files using `docs/nginx-live.conf`.
