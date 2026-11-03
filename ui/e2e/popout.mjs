// Headless test of popping a tab out into its own window, over real tmux.
// Prereqs: same as smoke.mjs.  Run: node ui/e2e/popout.mjs
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
async function until(fn, msg, ms = 12000) {
  const end = Date.now() + ms;
  for (;;) {
    try { if (await fn()) return check(true, msg); } catch {}
    if (Date.now() > end) check(false, msg);
    await sleep(150);
  }
}
const companions = () => ssh("tmux ls -F '#{session_name}' 2>/dev/null").split("\n").filter((l) => l.startsWith("_mx_"));

ssh("tmux kill-session -t popt 2>/dev/null; for s in $(tmux ls -F '#{session_name}' 2>/dev/null | grep '^_mx_'); do tmux kill-session -t $s; done; " +
  "tmux new-session -d -s popt -x 140 -y 36 -n main; tmux new-window -d -t popt: -n logs; tmux new-window -d -t popt: -n agent; " +
  "tmux send-keys -t popt:2 'clear; echo popped-out-window-2' Enter; tmux send-keys -t popt:1 'clear; echo this-is-logs' Enter");
const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1400, height: 860 } });
const page = await ctx.newPage();
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
try {
  await page.goto(url);
  await page.click('[data-testid="host-ubuntu-tmux34"]');
  const t = await page.waitForSelector('[data-testid="trust-host"]', { timeout: 2500 }).catch(() => null);
  if (t) await t.click();
  await page.click('[data-testid="session-popt"]');
  await page.waitForSelector('[data-testid="tab-2"]');

  console.log("right-click a tab → Open in new window: the tab moves there");
  await page.click('[data-testid="tab-2"]', { button: "right" });
  const [pop] = await Promise.all([ctx.waitForEvent("page"), page.click('[data-testid="tab-popout"]')]);
  pop.on("pageerror", (e) => errors.push(`popout: ${e.message}`));
  await pop.waitForSelector(".xterm-rows");
  await until(async () => (await pop.textContent(".workspace")).includes("popped-out-window-2"), "new window shows that tab");
  const popTabs = await pop.$$eval(".tab", (els) => els.map((e) => e.textContent));
  check(popTabs.length === 1 && popTabs[0].includes("agent"), `pop-out holds just that tab (${popTabs.join(" | ")})`);
  check(!(await pop.$('[data-testid="new-window"]')) && !(await pop.$('[data-testid="open-switcher"]')), "no new-tab or tab-switcher in a pop-out");
  check((await pop.textContent('[data-testid="session-name"]')) === "popt", "titled with the original session name");
  check(!(await pop.$(".sidebar")) && !(await pop.$("text=Detach")), "no sidebar, no Detach");
  check(companions().length === 1, "one hidden companion session on the server");
  await until(async () => (await page.$$(".tab")).length === 2, "the main window no longer lists it");
  check(!(await page.$('[data-testid="tab-2"]')), "…tab 2 is gone from the main window");
  check(ssh("tmux display -p -t popt: '#{window_index}'") === "0", "main window stays on its own tab");
  check(!(await page.$('[data-testid^="session-_mx_"]')), "companion isn't listed in the sidebar");

  console.log("each window keeps to its own tabs");
  await page.click('[data-testid="tab-1"]');
  await until(() => ssh("tmux display -p -t popt: '#{window_index}'") === "1", "main switched to tab 1");
  await page.keyboard.press("Control+Tab");
  await until(() => ssh("tmux display -p -t popt: '#{window_index}'") === "0", "Ctrl+Tab in main skips the popped-out tab");
  check(ssh(`tmux display -p -t '${companions()[0]}' '#{window_index}'`) === "2", "pop-out still on its tab");
  await pop.click(".xterm-screen");
  await pop.keyboard.press("Control+Tab");
  await sleep(600);
  check(ssh(`tmux display -p -t '${companions()[0]}' '#{window_index}'`) === "2", "Ctrl+Tab does nothing in a pop-out");
  await page.click('[data-testid="tab-1"]', { button: "right" });
  await page.click('[data-testid="tab-popout"]');
  const second = await ctx.waitForEvent("page", { timeout: 6000 });
  await second.waitForSelector(".xterm-rows");
  await until(async () => (await page.$$(".tab")).length === 1, "second tab popped out too");
  await page.click('[data-testid="tab-0"]', { button: "right" });
  await page.click('[data-testid="tab-popout"]');
  await until(async () => (await page.textContent(".toasts")).includes("only tab here"), "the last tab can't be popped out");

  console.log("closing: the tab comes back; closing the tab closes its window");
  ssh("tmux kill-window -t popt:1");
  await until(() => second.isClosed(), "pop-out closes itself when its tab is closed");
  await pop.close({ runBeforeUnload: true });
  await until(() => companions().length === 0, "companion sessions removed");
  await until(async () => (await page.$$(".tab")).length === 2, "tab 2 is back in the main window");
  check(ssh("tmux list-windows -t popt | wc -l") === "2", "windows intact (only the one we closed is gone)");
  await page.click('[data-testid="tab-2"]');
  await until(async () => (await page.textContent(".workspace")).includes("popped-out-window-2"), "and it works there");

  console.log("dragging shows the tab (preview card)");
  const ghost = await page.evaluate(() => {
    const tab = document.querySelector('[data-testid="tab-2"]');
    const dt = new DataTransfer();
    tab.dispatchEvent(new DragEvent("dragstart", { bubbles: true, dataTransfer: dt }));
    const g = document.querySelector(".drag-ghost");
    const text = g ? g.innerText : null;
    tab.dispatchEvent(new DragEvent("dragend", { bubbles: true, dataTransfer: dt }));
    return text;
  });
  check(ghost?.includes("agent") && ghost.includes("popped-out-window-2"), `drag image shows the tab and its terminal (${JSON.stringify(ghost?.slice(0, 60))})`);

  console.log("moving a tab back: button");
  await page.click('[data-testid="tab-2"]', { button: "right" });
  const [p2] = await Promise.all([ctx.waitForEvent("page"), page.click('[data-testid="tab-popout"]')]);
  await p2.waitForSelector('[data-testid="move-back"]');
  await until(async () => (await page.$$(".tab")).length === 1, "tab 2 moved out again");
  await p2.click('[data-testid="move-back"]');
  await until(() => p2.isClosed(), "Move back closes the pop-out");
  await until(async () => (await page.$$(".tab")).length === 2, "…and the tab is back in the main window");
  check(companions().length === 0, "companion removed");

  console.log("moving a tab back: drop it on the main tab bar");
  await page.click('[data-testid="tab-2"]', { button: "right" });
  const [p3] = await Promise.all([ctx.waitForEvent("page"), page.click('[data-testid="tab-popout"]')]);
  await p3.waitForSelector(".xterm-rows");
  await until(async () => (await page.$$(".tab")).length === 1, "tab 2 moved out a third time");
  const popAtt = new URL(p3.url()).searchParams.get("popout");
  // What the main window receives when the pop-out's tab is dragged onto it.
  const hint = await page.evaluate((id) => {
    const bar = document.querySelector('[data-testid="tabbar"]');
    const dt = new DataTransfer();
    dt.setData("application/x-muxscape-tab", id);
    bar.dispatchEvent(new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer: dt }));
    return new Promise((r) => setTimeout(() => r(document.querySelector(".drop-hint")?.textContent ?? null), 50));
  }, popAtt);
  check(hint === "Drop to move it back here", "hovering a popped-out tab over the tab bar shows where it goes");
  await page.evaluate((id) => {
    const bar = document.querySelector('[data-testid="tabbar"]');
    const dt = new DataTransfer();
    dt.setData("application/x-muxscape-tab", id);
    bar.dispatchEvent(new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer: dt }));
  }, popAtt);
  await until(() => p3.isClosed(), "dropping it on the main window closes the pop-out");
  await until(async () => (await page.$$(".tab")).length === 2, "…the tab is back");
  await until(() => ssh("tmux display -p -t popt: '#{window_index}'") === "2", "…and shown");
  check(!(await page.$(".drop-hint")), "drop hint gone");
  check(companions().length === 0, "companion removed");

  check(errors.length === 0, `no page errors (${errors.join("; ")})`);
  console.log("\nPOPOUT E2E PASSED");
} finally {
  ssh("tmux kill-session -t popt 2>/dev/null; true");
  await browser.close();
}
