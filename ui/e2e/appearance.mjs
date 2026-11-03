// Headless test of appearance settings and host colours in the real UI.
// Prereqs: same as smoke.mjs.  Run: node ui/e2e/appearance.mjs
import { chromium } from "playwright";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const key = path.join(repo, "dev/keys/test_ed25519");
const ssh = (cmd) =>
  execFileSync("ssh", ["-i", key, "-p", "2222", "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null",
    "-o", "LogLevel=ERROR", "dev@127.0.0.1", cmd]).toString().trim();
const url = process.env.MUXSCAPE_URL ?? "http://127.0.0.1:7878/?token=devtoken";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
function check(cond, msg) {
  if (!cond) throw new Error(`FAILED: ${msg}`);
  console.log(`  ✓ ${msg}`);
}
async function until(fn, msg, ms = 10000) {
  const end = Date.now() + ms;
  for (;;) {
    try { if (await fn()) return check(true, msg); } catch {}
    if (Date.now() > end) check(false, msg);
    await sleep(150);
  }
}

ssh("tmux kill-session -t aptest 2>/dev/null; tmux new-session -d -s aptest -x 120 -y 30; tmux split-window -h -t aptest:0");
const browser = await chromium.launch();
const page = await (await browser.newContext({ viewport: { width: 1400, height: 860 } })).newPage();
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
const css = (name) => page.evaluate((n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim(), name);
const termFont = () => page.evaluate(() => getComputedStyle(document.querySelector(".xterm-rows")).fontSize);
try {
  await page.goto(url);
  await page.evaluate(() => localStorage.removeItem("muxscape.prefs.v1"));
  await page.reload();
  await page.click('[data-testid="host-ubuntu-tmux34"]');
  const t = await page.waitForSelector('[data-testid="trust-host"]', { timeout: 2500 }).catch(() => null);
  if (t) await t.click();
  await page.click('[data-testid="session-aptest"]');
  await page.waitForSelector(".xterm-rows");
  check((await page.getAttribute("html", "data-theme")) === "dusk", "default theme is Dusk");

  console.log("right-click a pane: a menu that acts on that pane");
  const panes = () => ssh("tmux list-panes -t aptest:0 -F '#{pane_id}'").split("\n");
  const [left, right] = panes();
  const rightPane = `[data-testid="pane-${right}"]`;
  await page.click(rightPane, { button: "right" });
  await page.waitForSelector('[data-testid="pane-close"]');
  const labels = await page.$$eval(".ctx-menu .ctx-item", (els) => els.map((e) => e.textContent));
  check(labels.some((l) => l.startsWith("Paste")) && labels.some((l) => l.startsWith("Split right")) && labels.some((l) => l.startsWith("Close pane")), `menu: ${labels.join(" | ")}`);
  check(labels.some((l) => l.includes("Ctrl+Shift+W")), "items show their shortcuts");
  await until(() => ssh("tmux display -p -t aptest:0 '#{pane_id}'") === right, "right-click focused that pane");
  await page.click('[data-testid="pane-zoom"]');
  await until(() => ssh("tmux display -p -t aptest:0 '#{window_zoomed_flag}'") === "1", "Zoom pane zoomed it");
  await page.click(rightPane, { button: "right" });
  await until(async () => (await page.textContent('[data-testid="pane-zoom"]')) === "Unzoom paneCtrl+Shift+Z", "menu offers Unzoom");
  await page.click('[data-testid="pane-zoom"]');
  await until(() => ssh("tmux display -p -t aptest:0 '#{window_zoomed_flag}'") === "0", "unzoomed");
  await page.click(rightPane, { button: "right" });
  await page.click('[data-testid="pane-close"]');
  await page.click('[data-testid="dialog-ok"]');
  await until(() => ssh("tmux list-panes -t aptest:0 -F '#{pane_id}'") === left, "Close pane closed exactly the right-clicked pane");
  ssh("tmux split-window -h -t aptest:0");

  console.log("pane actions live in the tab bar (nothing floats over text)");
  check(!(await page.$(".pane-tools")), "no floating pane toolbar");
  await page.waitForSelector('[data-testid="close-pane"]');
  await page.click('[data-testid="close-pane"]');
  await page.click('[data-testid="dialog-ok"]');
  await until(() => ssh("tmux list-panes -t aptest:0 | wc -l") === "1", "tab-bar close button closed the tmux pane");
  await until(async () => !(await page.$('[data-testid="close-pane"]')), "pane buttons hidden once the window isn't split");

  console.log("red menu items are readable (not red-on-red) in every theme");
  for (const theme of ["dusk", "midnight", "mocha", "daylight"]) {
    await page.evaluate((t) => (document.documentElement.dataset.theme = t), theme);
    await page.click('[data-testid="session-aptest"]', { button: "right" });
    const item = page.locator(".ctx-item.danger", { hasText: "Kill session" });
    await item.waitFor();
    const [bg, fg] = await item.evaluate((el) => [getComputedStyle(el).backgroundColor, getComputedStyle(el).color]);
    check(bg === "rgba(0, 0, 0, 0)" && fg !== bg, `${theme}: "Kill session…" is red text on the menu (${fg})`);
    await page.keyboard.press("Escape");
    await page.mouse.click(700, 500);
  }
  await page.evaluate(() => (document.documentElement.dataset.theme = "dusk"));

  console.log("appearance: theme, accent, text size (live, remembered)");
  await page.click('[data-testid="appearance-button"]');
  await page.click('[data-testid="theme-midnight"]');
  check((await page.getAttribute("html", "data-theme")) === "midnight", "Midnight applied immediately");
  check((await css("--bg")) === "#0e1116", "Midnight colours");
  await page.click('[data-testid="accent-rose"]');
  check((await css("--accent")) === "#ec8fb0", "rose accent applied");
  const before = await termFont();
  await page.click('[data-testid="font-larger"]');
  await page.click('[data-testid="font-larger"]');
  await until(async () => (await page.textContent('[data-testid="font-size"]')) === "15px", "text size 15px");
  await until(async () => (await termFont()) !== before, `terminal text resized (${before} → ${await termFont()})`);
  await page.click('[data-testid="theme-daylight"]');
  check((await css("--accent")) === "#c2457a", "accent switches to its light-theme shade");
  await page.reload();
  await page.waitForSelector('[data-testid="host-ubuntu-tmux34"]');
  check((await page.getAttribute("html", "data-theme")) === "daylight", "theme remembered after reload");
  check((await css("--accent")) === "#c2457a", "accent remembered");
  await page.click('[data-testid="appearance-button"]');
  await page.click('[data-testid="appearance-reset"]');
  check((await page.getAttribute("html", "data-theme")) === "dusk" && (await page.textContent('[data-testid="font-size"]')) === "13px", "reset to defaults");
  await page.keyboard.press("Escape");

  console.log("host colour");
  await page.click('[data-testid="host-ubuntu-tmux34"]', { button: "right" });
  await page.click("text=Edit host…");
  await page.click('[data-testid="host-color-#e8907f"]');
  await page.click('[data-testid="host-save"]');
  await until(async () => (await page.getAttribute('[data-testid="host-ubuntu-tmux34"] .host-tile', "style"))?.includes("#e8907f"), "tile uses the chosen colour");
  await page.reload();
  await until(async () => (await page.getAttribute('[data-testid="host-ubuntu-tmux34"] .host-tile', "style"))?.includes("#e8907f"), "colour saved with the host");

  check(errors.length === 0, `no page errors (${errors.join("; ")})`);
  console.log("\nAPPEARANCE E2E PASSED");
} finally {
  ssh("tmux kill-session -t aptest 2>/dev/null; true");
  await browser.close();
}
