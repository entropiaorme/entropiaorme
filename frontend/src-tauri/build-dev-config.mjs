// Writes a per-checkout Tauri config overlay carrying the dev devUrl bound
// to ENTROPIAORME_FRONTEND_PORT (falling back to Vite's 5173 default when
// unset). `tauri dev` is invoked with `--config tauri.dev.conf.json
// --config tauri.dev.local.json` so the two overlays merge over the base
// via Tauri's JSON-merge-patch config-extension mechanism.
//
// The indirection exists because Tauri 2 does not support `${env:VAR}`
// interpolation inside tauri.conf.json field values, and the only
// dev-URL-related environment variable it reads (TAURI_DEV_HOST) targets
// mobile public-network development rather than overriding devUrl. The
// generated overlay is the smallest portable shim that keeps Vite's
// env-driven port honoured by Tauri's webview-loading side without
// hardcoding the port in committed config.
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const port = process.env.ENTROPIAORME_FRONTEND_PORT ?? "5173";
const overlay = { build: { devUrl: `http://localhost:${port}` } };
const out = join(dirname(fileURLToPath(import.meta.url)), "tauri.dev.local.json");
writeFileSync(out, JSON.stringify(overlay, null, 2) + "\n");
