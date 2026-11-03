// Headless test of Find in pane (Ctrl+Shift+F).
import { chromium } from "playwright";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";
import fs from "node:fs";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const key = path.join(repo, "dev/keys/test_ed25519");
const ssh = (cmd) =>
  execFileSync("ssh", ["-i", key, "-p", "2222", "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null",
    "-o", "LogLevel=ERROR", "dev@127.0.0.1", cmd]).toString().trim();
const url = process.env.MUXSCAPE_URL ?? "http://127.0.0.1:7878/?token=devtoken";
const shots = process.argv[2];
if (shots) fs.mkdirSync(shots, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
function check(cond, msg) {
  if (!cond) throw new Error(`FAILED: ${msg}`);
  console.log(`  ✓ ${msg}`);
}
async function until(fn, msg, ms = 15000) {
  const end = Date.now() + ms;
  for (;;) {
    try { if (await fn()) return check(true, msg); } catch {}
    if (Date.now() > end) check(false, msg);
    await sleep(150);
  }
}

ssh("tmux kill-session -t findtest 2>/dev/null; tmux new-session -d -s findtest");
ssh(`tmux send-keys -t findtest 'clear; for i in $(seq 1 60); do echo "line $i"; done; echo "ERROR timeout in worker"; echo "error: retrying"; echo "Error again"' Enter`);
const browser = await chromium.launch();
const page = await (await browser.newContext({ viewport: { width: 1400, height: 860 } })).newPage();
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
const count = () => page.textContent('[data-testid="find-count"]');
try {
  await page.goto(url);
  await page.click('[data-testid="host-ubuntu-tmux34"]');
  const t = await page.waitForSelector('[data-testid="trust-host"]', { timeout: 2500 }).catch(() => null);
  if (t) await t.click();
  await page.click('[data-testid="session-findtest"]');
  await until(async () => (await page.textContent(".workspace")).includes("Error again"), "output on screen");
  await page.click(".workspace .xterm");
  await page.keyboard.press("Control+Shift+F");
  await page.waitForSelector('[data-testid="find-bar"]');
  check(true, "Ctrl+Shift+F opens the find bar");
  await page.keyboard.type("error");
  await until(async () => /of 3$/.test(await count()), "three matches, any case");
  await page.click('[data-testid="find-case"]');
  await until(async () => /of 1$/.test(await count()), "match case: one");
  await page.click('[data-testid="find-case"]');
  await page.fill('[data-testid="find-input"]', "line 4");
  await until(async () => /of 11$/.test(await count()), "'line 4' matches line 4 and lines 40–49");
  const before = await count();
  await page.keyboard.press("Enter");
  await sleep(200);
  check((await count()) !== before, `Enter moves to the next match (${before} → ${await count()})`);
  await page.fill('[data-testid="find-input"]', "zzz-nothing");
  await until(async () => (await count()) === "No matches", "says when nothing matches");
  await page.fill('[data-testid="find-input"]', "ERROR timeout");
  await until(async () => /1 of 1$/.test(await count()), "found");
  if (shots) await page.screenshot({ path: path.join(shots, "find.png") });
  await page.keyboard.press("Escape");
  await until(async () => !(await page.$('[data-testid="find-bar"]')), "Esc closes it");
  // Typing goes back to the terminal.
  await page.keyboard.type("echo back-in-terminal\n");
  await until(() => ssh("tmux capture-pane -p -t findtest").includes("back-in-terminal"), "focus returns to the terminal");
  check(errors.length === 0, `no page errors (${errors.join("; ")})`);
  console.log("\nFIND E2E PASSED");
} finally {
  ssh("tmux kill-session -t findtest 2>/dev/null; true");
  await browser.close();
}
