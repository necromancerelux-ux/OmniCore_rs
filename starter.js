'use strict';

/**
 * OmniCore (Rust) — panel startup file
 *
 * Use this when your host only lets you set a startup file / command and
 * does not give you an interactive console.
 *
 * Panel examples:
 *   Startup command:  node starter.js
 *   JS FILE:          starter.js
 *   Start script:     node starter.js
 *
 * What it does:
 *   1. Loads .env into the process environment (token, OWNER_ID, etc.)
 *   2. Finds the omnicore binary (see BINARY PATHS below)
 *   3. Runs it as a child process
 *   4. If the bot exits with code 42 (Owner → Restart), starts it again
 *   5. On any other exit code, exits so the panel can mark the server stopped
 *
 * Layout expected next to this file:
 *   starter.js
 *   .env                 ← DISCORD_BOT_TOKEN + OWNER_ID
 *   omnicore             ← Linux x86_64 binary (chmod +x)
 *   assets/              ← fonts + emoji-icons (required)
 *   data/                ← created automatically for SQLite
 *
 * BINARY PATHS (first match wins):
 *   OMNICORE_BIN env var
 *   ./omnicore
 *   ./omnicore-linux-x86_64
 *   ./bin/omnicore
 *   ./target/release/omnicore
 */

const path = require('path');
const fs = require('fs');
const { spawn } = require('child_process');

const ROOT = __dirname;
const RESTART_CODE = 42;
const RETRY_MS = 1000;
const CRASH_RETRY_MS = 5000;

// ── load .env (stdlib only — no dotenv package required) ──────────────
function loadEnvFile(filePath) {
  if (!fs.existsSync(filePath)) return;
  const text = fs.readFileSync(filePath, 'utf8');
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith('#')) continue;
    const eq = trimmed.indexOf('=');
    if (eq <= 0) continue;
    const key = trimmed.slice(0, eq).trim();
    let val = trimmed.slice(eq + 1).trim();
    if (
      (val.startsWith('"') && val.endsWith('"')) ||
      (val.startsWith("'") && val.endsWith("'"))
    ) {
      val = val.slice(1, -1);
    }
    if (process.env[key] === undefined) process.env[key] = val;
  }
}

loadEnvFile(path.join(ROOT, '.env'));

// Sensible defaults for panel hosts
if (!process.env.DATABASE_PATH) {
  process.env.DATABASE_PATH = path.join(ROOT, 'data', 'bot.sqlite');
}
if (!process.env.RUST_LOG) {
  process.env.RUST_LOG = 'info';
}

// Ensure data dir exists for SQLite
try {
  fs.mkdirSync(path.dirname(process.env.DATABASE_PATH), { recursive: true });
} catch {
  /* ignore */
}

// ── resolve binary ────────────────────────────────────────────────────
function resolveBinary() {
  const candidates = [
    process.env.OMNICORE_BIN,
    path.join(ROOT, 'omnicore'),
    path.join(ROOT, 'omnicore-linux-x86_64'),
    path.join(ROOT, 'bin', 'omnicore'),
    path.join(ROOT, 'target', 'release', 'omnicore'),
  ].filter(Boolean);

  for (const candidate of candidates) {
    try {
      if (fs.existsSync(candidate) && fs.statSync(candidate).isFile()) {
        // Ensure executable bit on Unix
        try {
          fs.accessSync(candidate, fs.constants.X_OK);
        } catch {
          try {
            fs.chmodSync(candidate, 0o755);
          } catch {
            /* panel may not allow chmod — try running anyway */
          }
        }
        return candidate;
      }
    } catch {
      /* next */
    }
  }
  return null;
}

const binary = resolveBinary();
if (!binary) {
  console.error('[starter] No omnicore binary found.');
  console.error('[starter] Place a Linux x86_64 binary next to starter.js as:');
  console.error('[starter]   ./omnicore');
  console.error('[starter] or set OMNICORE_BIN to the full path.');
  console.error('[starter] Build with: cargo build --release --no-default-features');
  console.error('[starter] Or download the GitHub Actions artifact omnicore-linux-x86_64');
  process.exit(1);
}

if (!process.env.DISCORD_BOT_TOKEN && !process.env.TOKEN && !process.env.BOT_TOKEN) {
  console.error('[starter] Missing DISCORD_BOT_TOKEN in .env');
  process.exit(1);
}

console.log(`[starter] binary: ${binary}`);
console.log(`[starter] database: ${process.env.DATABASE_PATH}`);
console.log(`[starter] cwd: ${ROOT}`);

// ── supervisor ────────────────────────────────────────────────────────
let child = null;
let stopping = false;

function start() {
  console.log(`[starter] launching at ${new Date().toISOString()}`);
  child = spawn(binary, [], {
    cwd: ROOT,
    stdio: 'inherit',
    env: process.env,
  });

  child.on('error', (err) => {
    console.error('[starter] failed to spawn binary:', err.message);
    if (err.code === 'ENOENT') {
      console.error('[starter] binary path not found or not executable');
    }
    if (err.code === 'EACCES') {
      console.error('[starter] permission denied — chmod +x the binary');
    }
    process.exit(1);
  });

  child.on('exit', (code, signal) => {
    child = null;
    if (stopping) {
      process.exit(code ?? (signal ? 1 : 0));
      return;
    }
    if (code === RESTART_CODE) {
      console.log(`[starter] restart requested (exit ${RESTART_CODE}) — relaunching in ${RETRY_MS}ms`);
      setTimeout(start, RETRY_MS);
      return;
    }
    // Unexpected crash: retry a few times so a transient failure does not
    // leave the panel "online" with a dead process forever. After that, exit
    // so the panel can mark the server stopped / auto-restart if configured.
    const delay = CRASH_RETRY_MS;
    console.error(
      `[starter] process exited code=${code} signal=${signal} — retry in ${delay}ms`
    );
    setTimeout(start, delay);
  });
}

for (const sig of ['SIGINT', 'SIGTERM']) {
  process.on(sig, () => {
    stopping = true;
    console.log(`[starter] received ${sig}`);
    if (child) {
      try {
        child.kill(sig);
      } catch {
        process.exit(0);
      }
    } else {
      process.exit(0);
    }
  });
}

start();
