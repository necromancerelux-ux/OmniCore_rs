# OmniCore deploy

## 1. Get the right binary (match the host CPU!)

HidenCloud is **ARM64 (aarch64)**. An x86_64 binary will not start there — `starter.js`
now detects that and tells you.

* GitHub → **Actions → build-release → Run workflow** (or push a tag like `v2.0.1`).
* Download the artifact for your CPU: `omnicore-linux-aarch64` (ARM64) or `omnicore-linux-x86_64`.
  GitHub gives it as a `.zip` containing `omnicore-linux-<cpu>.gz`.
* Upload the **`.gz`** as-is (starter.js unpacks it) or the unpacked binary.

Build it yourself instead (Rust **1.94+**, because sqlx 0.9 needs it):

```bash
cargo build --release          # → target/release/omnicore (for the CPU you build on)
```

## 2. Panel layout (no console needed)

Upload into one folder:

```text
starter.js
.env                              (copy of .env.example, filled in)
omnicore-linux-aarch64.gz         (or the unpacked binary / omnicore-linux-x86_64)
assets/                           (fonts + emoji-icons)
```

Set the host **startup file / command** to:

```text
node starter.js
```

`starter.js` loads `.env`, picks the binary for this CPU, restarts on exit code 42
(Owner → Restart) and backs off on repeated crashes (gives up after 8 quick crashes
so a wrong token can't hammer Discord).

## 3. Required `.env`

```env
DISCORD_BOT_TOKEN=...
OWNER_ID=...
```

Everything else is optional — see `.env.example`.

## 4. Oracle token renewal (browser)

Releases are built **with** the browser feature. Chromium is started only when Oracle
needs a token and is closed right after.

* **Easiest:** owner uses **Paste cURL** (no browser needed on the host).
* **Automatic renewal:** the host needs a Chromium:
  * set `CHROME_PATH=/path/to/chrome`, **or**
  * set `CHROMIUM_AUTO_INSTALL=1` — `starter.js` downloads Chromium once via Playwright
    into `./.browsers` (needs Node + npm and ~200 MB of disk; delete the folder to remove it).
* Chromium also needs a few system libraries (nss, etc.). If it fails to start on the panel,
  the bot keeps working and falls back to Paste cURL. Log in manually (first login +
  the Authorize click) with a headed browser, then let the bot renew headlessly.
* `ORACLE_BROWSER=0` turns the browser off entirely.

## 5. Docker

```bash
docker compose up -d --build
# with Chromium inside the image:
docker build --build-arg WITH_CHROMIUM=1 -t omnicore .
```

The image builds for whatever CPU it is built on (x86_64 or aarch64).
