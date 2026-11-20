# OmniCore deploy

## Panel startup (HidenCloud / Node-only start command)

1. Upload into the same folder:
   - `omnicore` (Linux x86_64 binary, chmod +x)
   - `starter.js`
   - `.env` (from `.env.example`)
   - `assets/` (fonts + emoji-icons)

2. Set the host **startup file / command** to:

```text
node starter.js
```

`starter.js` loads `.env`, runs the binary, and auto-restarts on exit code 42 (Owner → Restart).

## Required .env

```env
DISCORD_BOT_TOKEN=...
OWNER_ID=...
```

## Build the binary (on any PC with Rust 1.85+)

```bash
cargo build --release --no-default-features
# → target/release/omnicore
```

Or use GitHub Actions workflow `build-release`.

## Docker

```bash
docker compose up -d --build
```
