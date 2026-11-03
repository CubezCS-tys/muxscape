// Headless test: paste a screenshot / drop files on a remote terminal →
// uploaded to the server, path typed. Prereqs: same as smoke.mjs.
import { chromium } from "playwright";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
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
async function until(fn, msg, ms = 12000) {
  const end = Date.now() + ms;
  for (;;) {
    try { if (await fn()) return check(true, msg); } catch {}
    if (Date.now() > end) check(false, msg);
    await sleep(150);
  }
}
// A real 1x1 PNG.
const PNG = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");
const sha = (b) => createHash("sha256").update(b).digest("hex");

ssh("rm -rf ~/.cache/muxscape/uploads; tmux kill-session -t upl 2>/dev/null; tmux new-session -d -s upl -x 140 -y 36");
const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1400, height: 860 } });
await ctx.grantPermissions(["clipboard-read", "clipboard-write"], { origin: "http://127.0.0.1:7878" });
const page = await ctx.newPage();
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
const screen = () => page.textContent(".workspace");
try {
  await page.goto(url);
  await page.click('[data-testid="host-ubuntu-tmux34"]');
  const t = await page.waitForSelector('[data-testid="trust-host"]', { timeout: 2500 }).catch(() => null);
  if (t) await t.click();
  await page.click('[data-testid="session-upl"]');
  await page.waitForSelector(".xterm-rows");
  await page.click(".xterm-screen");
  await page.keyboard.type("ls -l ");

  console.log("paste a screenshot (Ctrl+V with an image on the clipboard)");
  await page.evaluate((b64) => {
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const dt = new DataTransfer();
    dt.items.add(new File([bytes], "image.png", { type: "image/png" }));
    document.querySelector(".xterm-helper-textarea").dispatchEvent(new ClipboardEvent("paste", { clipboardData: dt, bubbles: true, cancelable: true }));
  }, PNG.toString("base64"));
  await until(async () => /\.cache\/muxscape\/uploads\/image-\d{8}-\d{6}-[0-9a-f]{4}\.png/.test(await screen()), "path typed at the prompt");
  const p1 = (await screen()).match(/\/home\/dev\/\.cache\/muxscape\/uploads\/image-[\w-]+\.png/)[0];
  check(ssh(`sha256sum '${p1}' | cut -d' ' -f1`) === sha(PNG), "the server has the exact image");
  await page.keyboard.press("Enter");
  await until(async () => (await screen()).includes("-rw-------"), "…readable only by you (ls -l ran on it)");

  console.log("drop files on the pane");
  const drop = (type, files) => page.evaluate(([type, files]) => {
    const dt = new DataTransfer();
    for (const [name, text] of files) dt.items.add(new File([text], name, { type: "text/plain" }));
    document.querySelector('[data-testid^="pane-%"]').dispatchEvent(new DragEvent(type, { dataTransfer: dt, bubbles: true, cancelable: true }));
  }, [type, files]);
  await page.keyboard.type("clear; cat ");
  await drop("dragover", [["notes.txt", "x"]]);
  await until(async () => (await page.textContent(".pane-drop").catch(() => "")) === "Drop to upload to the server", "drop hint over the pane");
  if (shots) await page.screenshot({ path: path.join(shots, "drop.png") });
  await drop("drop", [["notes.txt", "hello-from-notes"], ["config.yaml", "key: value-42"]]);
  await until(async () => (await screen()).includes("config-"), "both paths typed");
  check(!(await page.$(".pane-drop")), "hint gone");
  await page.keyboard.press("Enter");
  await until(async () => (await screen()).includes("hello-from-noteskey: value-42"), "cat read both uploaded files on the server");

  console.log("menu Paste with an image on the clipboard");
  await page.evaluate(async (b64) => {
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    await navigator.clipboard.write([new ClipboardItem({ "image/png": new Blob([bytes], { type: "image/png" }) })]);
  }, PNG.toString("base64"));
  await page.keyboard.type("clear; echo got ");
  await page.click('[data-testid^="pane-%"]', { button: "right" });
  await page.click('[data-testid="pane-paste"]');
  await until(async () => /got \/home\/dev\/\.cache\/muxscape\/uploads\/image-[\w-]+\.png/.test(await screen()), "menu Paste uploaded the clipboard image");
  check(ssh("ls ~/.cache/muxscape/uploads | wc -l") === "4", "4 files uploaded in total");

  check(errors.length === 0, `no page errors (${errors.join("; ")})`);
  console.log("\nUPLOADS E2E PASSED");
} finally {
  ssh("tmux kill-session -t upl 2>/dev/null; rm -rf ~/.cache/muxscape/uploads; true");
  await browser.close();
}
