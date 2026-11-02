// Headless browser smoke test of the real UI against real tmux.
//
// Prereqs:  dev/test-server/up.sh   (disposable docker servers)
//           (cd ui && npm run build) && dev/run-devserver.sh
//           npx playwright install chromium   (once)
// Run:      node ui/e2e/smoke.mjs [screenshot-dir]
//
// Every assertion checks state on the tmux server itself (via plain ssh),
// not just what the UI shows.

import { chromium } from "playwright";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";
import fs from "node:fs";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const key = path.join(repo, "dev/keys/test_ed25519");
const url = process.env.MUXSCAPE_URL ?? "http://127.0.0.1:7878/?token=devtoken";
const shots = process.argv[2];
if (shots) fs.mkdirSync(shots, { recursive: true });

const ssh = (cmd, port = 2222) =>
  execFileSync("ssh", ["-i", key, "-p", String(port), "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null",
    "-o", "LogLevel=ERROR", "dev@127.0.0.1", cmd]).toString().trim();
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let step = 0;
function check(cond, msg) {
  if (!cond) throw new Error(`FAILED: ${msg}`);
  console.log(`  ✓ ${msg}`);
}
async function snap(page, name) {
  if (shots) await page.screenshot({ path: path.join(shots, `${String(++step).padStart(2, "0")}-${name}.png`) });
}
async function until(fn, msg, ms = 10000) {
  const end = Date.now() + ms;
  for (;;) {
    try {
      if (await fn()) return check(true, msg);
    } catch {}
    if (Date.now() > end) check(false, msg);
    await sleep(150);
  }
}

// Fixture: a session with 20 windows, created by an ordinary tmux client.
ssh("tmux kill-session -t e2e 2>/dev/null; tmux new-session -d -s e2e -x 150 -y 40 -n w0; " +
  "for i in $(seq 1 19); do tmux new-window -d -t e2e: -n w$i; done; " +
  "tmux send-keys -t e2e:17 'echo pre-existing-output-17' Enter");

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1400, height: 860 } });
const page = await ctx.newPage();
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
try {
  await page.goto(url);
  console.log("connect & discover");
  await page.click('[data-testid="host-ubuntu-tmux34"]');
  const trust = await page.waitForSelector('[data-testid="trust-host"]', { timeout: 4000 }).catch(() => null);
  if (trust) await trust.click();
  await page.waitForSelector('[data-testid="session-e2e"]');
  await page.click('[data-testid="session-e2e"]');
  await page.waitForSelector('[data-testid="tab-19"]');
  check((await page.$$('.tab[role="tab"]')).length === 20, "20 tmux windows appear as 20 tabs");
  await snap(page, "attached");

  console.log("click window 17 directly");
  await page.click('[data-testid="tab-17"]');
  await until(() => ssh("tmux display -p -t e2e: '#{window_index}'") === "17", "tmux selected window 17");
  await until(async () => (await page.textContent(".workspace")).includes("pre-existing-output-17"), "existing output restored");

  console.log("interactive work");
  await page.keyboard.type("echo typed-$((6*7))\n");
  await until(async () => (await page.textContent(".workspace")).includes("typed-42"), "typed command ran remotely");

  console.log("split & drag");
  await page.click('[data-testid="split-right"]');
  await until(() => ssh("tmux list-panes -t e2e:17 | wc -l") === "2", "split created a real tmux pane");
  const before = Number(ssh("tmux display -p -t e2e:17.0 '#{pane_width}'"));
  const div = await page.waitForSelector('[data-testid="divider--0"]');
  const b = await div.boundingBox();
  await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2);
  await page.mouse.down();
  await page.mouse.move(b.x - 150, b.y + b.height / 2, { steps: 8 });
  await page.mouse.up();
  await until(() => Number(ssh("tmux display -p -t e2e:17.0 '#{pane_width}'")) < before - 10, "divider drag resized the tmux pane");
  await snap(page, "split");

  console.log("full-screen app");
  await page.keyboard.type("vi -u NONE\n");
  await until(() => ssh("tmux display -p -t e2e:17 '#{alternate_on}'") === "1", "vim running on the alternate screen");
  await page.keyboard.type("ihello from vim\x1b");
  await until(async () => (await page.textContent(".workspace")).includes("hello from vim"), "vim renders typed text");
  await snap(page, "vim");

  console.log("rename / external changes / close");
  await page.dblclick('[data-testid="tab-3"]');
  await page.fill('[data-testid="tab-rename"]', "renamed in gui");
  await page.keyboard.press("Enter");
  await until(() => ssh("tmux display -p -t e2e:3 '#{window_name}'") === "renamed in gui", "rename reached tmux");
  ssh("tmux new-window -d -t e2e: -n from-other-client");
  await until(async () => (await page.textContent('[data-testid="tab-20"]'))?.includes("from-other-client"), "window from another client shows up");
  await page.hover('[data-testid="tab-20"]');
  await page.click('[data-testid="tab-20"] .tab-close');
  await page.click('[data-testid="dialog-ok"]');
  await until(() => ssh("tmux list-windows -t e2e | wc -l") === "20", "closing a tab killed the tmux window");

  console.log("persistence: connection loss and reload");
  ssh("tmux send-keys -t e2e:5 'while true; do date +%s > /tmp/e2e-tick; sleep 1; done' Enter");
  execFileSync("docker", ["exec", "muxscape-t34", "pkill", "-f", "sshd: dev"]);
  await until(async () => !!(await page.$(".chip-connected")) && !(await page.$(".offline-banner")), "reconnected automatically", 25000);
  await page.reload();
  await page.waitForSelector('[data-testid="tab-19"]', { timeout: 15000 });
  check((await page.textContent('[data-testid="session-name"]')) === "e2e", "workspace restored after reload");
  const t1 = ssh("cat /tmp/e2e-tick");
  await sleep(2200);
  check(ssh("cat /tmp/e2e-tick") !== t1, "long-running process survived");
  await snap(page, "restored");

  check(errors.length === 0, `no page errors (${errors.join("; ")})`);
  console.log("\nall smoke checks passed");
} finally {
  await browser.close();
  try { ssh("tmux kill-session -t e2e"); } catch {}
}
