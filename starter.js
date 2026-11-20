'use strict';

/**
 * OmniCore (Rust) — panel startup file
 *
 * For hosts that only let you pick a startup file (no console), e.g. HidenCloud:
 *   Startup file / command:  node starter.js
 *
 * What it does:
 *   1. Loads .env into the environment (token, OWNER_ID, ...)
 *   2. Finds the omnicore binary for THIS machine's CPU (x86_64 or aarch64/ARM64).
 *      It refuses a binary built for the wrong CPU and says so in plain words.
 *      If you only uploaded the release file (omnicore-linux-<arch>.gz) it unzips it.
 *   3. Optionally finds (or downloads) Chromium for Oracle token renewal
 *   4. Runs the bot; exit code 42 (Owner -> Restart) relaunches it
 *   5. Crash loop protection: quick repeated crashes back off (5s, 10s, 20s ... 5 min)
 *      and stop after 8 in a row, so a bad token cannot hammer Discord forever.
 *
 * Layout expected next to this file:
 *   starter.js
 *   .env                          <- DISCORD_BOT_TOKEN + OWNER_ID
 *   omnicore-linux-aarch64(.gz)   <- ARM64 hosts   (or omnicore-linux-x86_64(.gz))
 *   assets/                       <- fonts + emoji-icons (required)
 *   data/                         <- created automatically for SQLite
 *
 * BINARY PATHS (first usable match wins):
 *   OMNICORE_BIN env var, ./omnicore, ./omnicore-linux-<this cpu>,
 *   ./bin/omnicore, ./target/release/omnicore
 *
 * Optional .env switches:
 *   CHROME_PATH=/path/to/chrome     use this Chromium for Oracle renewal
 *   CHROMIUM_AUTO_INSTALL=1         download Chromium via Playwright into ./.browsers
 *                                   when none is found (needs Node + npm + ~200 MB disk)
 *   ORACLE_BROWSER=0                never use a browser (paste-cURL flow only)
 */

const path = require('path');
const fs = require('fs');
const os = require('os');
const zlib = require('zlib');
const { spawn, spawnSync } = require('child_process');

const ROOT = __dirname;
const RESTART_CODE = 42;
const RETRY_MS = 1000;
const CRASH_RETRY_MS = 5000;
const MAX_CRASH_DELAY_MS = 5 * 60 * 1000;
const MAX_QUICK_CRASHES = 8;
const HEALTHY_RUN_MS = 60 * 1000;
const PLAYWRIGHT_SPEC = 'playwright@1.48.2';

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
try {
  fs.mkdirSync(path.dirname(process.env.DATABASE_PATH), { recursive: true });
} catch {
  /* ignore */
}

// ── CPU architecture ──────────────────────────────────────────────────
const ARCH_NAME = { x64: 'x86_64', arm64: 'aarch64' }[process.arch] || process.arch;
const ELF_MACHINE = { 0x3e: 'x86_64', 0xb7: 'aarch64' };

/** CPU name an ELF binary was built for, or null if it is not a readable ELF file. */
function elfArch(file) {
  try {
    const fd = fs.openSync(file, 'r');
    const buf = Buffer.alloc(20);
    fs.readSync(fd, buf, 0, 20, 0);
    fs.closeSync(fd);
    if (buf[0] !== 0x7f || buf[1] !== 0x45 || buf[2] !== 0x4c || buf[3] !== 0x46) return null;
    const machine = buf[5] === 2 ? buf.readUInt16BE(18) : buf.readUInt16LE(18);
    return ELF_MACHINE[machine] || `unknown(${machine})`;
  } catch {
    return null;
  }
}

function isFile(p) {
  try {
    return fs.statSync(p).isFile();
  } catch {
    return false;
  }
}

/** If only the release .gz was uploaded, unzip it next to itself. */
function unpackReleaseArchives() {
  for (const arch of [ARCH_NAME, 'x86_64', 'aarch64']) {
    const gz = path.join(ROOT, `omnicore-linux-${arch}.gz`);
    const out = path.join(ROOT, `omnicore-linux-${arch}`);
    if (isFile(gz) && !isFile(out)) {
      try {
        console.log(`[starter] unpacking ${path.basename(gz)} ...`);
        fs.writeFileSync(out, zlib.gunzipSync(fs.readFileSync(gz)), { mode: 0o755 });
      } catch (err) {
        console.error(`[starter] could not unpack ${path.basename(gz)}: ${err.message}`);
      }
    }
  }
}

// ── resolve binary ────────────────────────────────────────────────────
function resolveBinary() {
  unpackReleaseArchives();
  const candidates = [
    process.env.OMNICORE_BIN,
    path.join(ROOT, 'omnicore'),
    path.join(ROOT, `omnicore-linux-${ARCH_NAME}`),
    // other-CPU names are listed only so a wrong upload is reported clearly
    path.join(ROOT, 'omnicore-linux-x86_64'),
    path.join(ROOT, 'omnicore-linux-aarch64'),
    path.join(ROOT, 'bin', 'omnicore'),
    path.join(ROOT, 'target', 'release', 'omnicore'),
  ].filter(Boolean);

  const wrongArch = [];
  for (const candidate of candidates) {
    if (!isFile(candidate)) continue;
    const built = elfArch(candidate);
    if (built && ELF_MACHINE && Object.values(ELF_MACHINE).includes(built) && built !== ARCH_NAME) {
      wrongArch.push(`${path.relative(ROOT, candidate) || candidate} (built for ${built})`);
      continue;
    }
    try {
      fs.accessSync(candidate, fs.constants.X_OK);
    } catch {
      try {
        fs.chmodSync(candidate, 0o755);
      } catch {
        /* panel may not allow chmod — try running anyway */
      }
    }
    return { file: candidate, wrongArch };
  }
  return { file: null, wrongArch };
}

const { file: binary, wrongArch } = resolveBinary();
if (!binary) {
  console.error(`[starter] No omnicore binary for this machine (CPU: ${ARCH_NAME}).`);
  if (wrongArch.length) {
    console.error(`[starter] Found, but built for a different CPU: ${wrongArch.join(', ')}`);
    console.error(`[starter] Upload the omnicore-linux-${ARCH_NAME} build instead.`);
  } else {
    console.error(`[starter] Upload omnicore-linux-${ARCH_NAME} (or the .gz from the GitHub release)`);
    console.error('[starter] next to starter.js, or set OMNICORE_BIN to the full path.');
  }
  console.error('[starter] Build: cargo build --release   (or use the GitHub Actions artifact)');
  process.exit(1);
}

if (!process.env.DISCORD_BOT_TOKEN && !process.env.TOKEN && !process.env.BOT_TOKEN) {
  console.error('[starter] Missing DISCORD_BOT_TOKEN in .env');
  process.exit(1);
}

// ── Chromium for Oracle token renewal (optional, only if browser is on) ─
function browserDisabled() {
  return ['0', 'false', 'off', 'no'].includes(String(process.env.ORACLE_BROWSER || '').toLowerCase());
}

function findChromium() {
  const given = process.env.CHROME_PATH || process.env.CHROMIUM_PATH;
  if (given && isFile(given)) return given;

  const roots = [
    process.env.PLAYWRIGHT_BROWSERS_PATH,
    path.join(ROOT, '.browsers'),
    path.join(os.homedir(), '.cache', 'ms-playwright'),
  ].filter(Boolean);
  const relPaths = [
    'chrome-linux/chrome',
    'chrome-linux64/chrome',
    'chrome-linux/headless_shell',
    'chrome-headless-shell-linux64/chrome-headless-shell',
  ];
  for (const root of roots) {
    let dirs = [];
    try {
      dirs = fs.readdirSync(root).filter((d) => /^chromium(_headless_shell)?-/.test(d)).sort().reverse();
    } catch {
      continue;
    }
    for (const d of dirs) {
      for (const rel of relPaths) {
        const p = path.join(root, d, rel);
        if (isFile(p)) return p;
      }
    }
  }
  for (const p of ['/usr/bin/chromium', '/usr/bin/chromium-browser', '/usr/bin/google-chrome', '/usr/bin/google-chrome-stable']) {
    if (isFile(p)) return p;
  }
  return null;
}

function setupChromium() {
  if (browserDisabled()) return;
  let found = findChromium();
  if (!found && process.env.CHROMIUM_AUTO_INSTALL === '1') {
    console.log('[starter] No Chromium found — downloading via Playwright (one time, ~200 MB)...');
    const browsersPath = path.join(ROOT, '.browsers');
    const npx = process.platform === 'win32' ? 'npx.cmd' : 'npx';
    const res = spawnSync(npx, ['--yes', PLAYWRIGHT_SPEC, 'install', 'chromium'], {
      cwd: ROOT,
      stdio: 'inherit',
      env: { ...process.env, PLAYWRIGHT_BROWSERS_PATH: browsersPath },
    });
    if (res.status !== 0) {
      console.error('[starter] Chromium download failed (the bot still runs; use Paste cURL for Oracle).');
    }
    found = findChromium();
  }
  if (found) {
    process.env.CHROME_PATH = found;
    console.log(`[starter] chromium: ${found}`);
  } else {
    console.log('[starter] chromium: not found (Oracle renewal uses Paste cURL; set CHROMIUM_AUTO_INSTALL=1 to download one)');
  }
}

setupChromium();

console.log(`[starter] cpu: ${ARCH_NAME}`);
console.log(`[starter] binary: ${binary}`);
console.log(`[starter] database: ${process.env.DATABASE_PATH}`);
console.log(`[starter] cwd: ${ROOT}`);

// ── supervisor ────────────────────────────────────────────────────────
let child = null;
let stopping = false;
let quickCrashes = 0;
let startedAt = 0;

function start() {
  console.log(`[starter] launching at ${new Date().toISOString()}`);
  startedAt = Date.now();
  child = spawn(binary, [], {
    cwd: ROOT,
    stdio: 'inherit',
    env: process.env,
  });

  child.on('error', (err) => {
    console.error('[starter] failed to spawn binary:', err.message);
    if (err.code === 'ENOENT') console.error('[starter] binary path not found or not executable');
    if (err.code === 'EACCES') console.error('[starter] permission denied — the panel did not allow chmod +x');
    if (err.code === 'ENOEXEC') console.error(`[starter] exec format error — wrong CPU build (this machine is ${ARCH_NAME})`);
    process.exit(1);
  });

  child.on('exit', (code, signal) => {
    child = null;
    if (stopping) {
      process.exit(code ?? (signal ? 1 : 0));
      return;
    }
    if (code === RESTART_CODE) {
      quickCrashes = 0;
      console.log(`[starter] restart requested (exit ${RESTART_CODE}) — relaunching in ${RETRY_MS}ms`);
      setTimeout(start, RETRY_MS);
      return;
    }
    // Crash handling: a run longer than HEALTHY_RUN_MS resets the counter; quick
    // repeated failures back off and eventually stop so the panel shows "offline".
    quickCrashes = Date.now() - startedAt > HEALTHY_RUN_MS ? 1 : quickCrashes + 1;
    if (quickCrashes >= MAX_QUICK_CRASHES) {
      console.error(`[starter] crashed ${quickCrashes} times in a row (last code=${code}) — giving up. Check the log above.`);
      process.exit(code || 1);
      return;
    }
    const delay = Math.min(CRASH_RETRY_MS * 2 ** (quickCrashes - 1), MAX_CRASH_DELAY_MS);
    console.error(
      `[starter] process exited code=${code} signal=${signal} — retry in ${Math.round(delay / 1000)}s (crash ${quickCrashes}/${MAX_QUICK_CRASHES})`
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
