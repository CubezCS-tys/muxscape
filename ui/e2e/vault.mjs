// Headless browser test of the password vault in the real UI, over real SSH.
//
// Prereqs: same as smoke.mjs; start the dev server fresh (in-memory vault).
// Run:     node ui/e2e/vault.mjs [screenshot-dir]
//
// The test server accepts dev/devpass. Every check is a real login.

import { chromium } from "playwright";
import path from "node:path";
import fs from "node:fs";

const url = process.env.MUXSCAPE_URL ?? "http://127.0.0.1:7878/?token=devtoken";
const shots = process.argv[2];
if (shots) fs.mkdirSync(shots, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let step = 0;
function check(cond, msg) {
  if (!cond) throw new Error(`FAILED: ${msg}`);
  console.log(`  ✓ ${msg}`);
}
async function snap(page, name) {
  if (shots) await page.screenshot({ path: path.join(shots, `${String(++step).padStart(2, "0")}-${name}.png`) });
}
async function until(fn, msg, ms = 15000) {
  const end = Date.now() + ms;
  for (;;) {
    try {
      if (await fn()) return check(true, msg);
    } catch {}
    if (Date.now() > end) check(false, msg);
    await sleep(150);
  }
}

const browser = await chromium.launch();
const page = await (await browser.newContext({ viewport: { width: 1400, height: 860 } })).newPage();
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
const HOST = '[data-testid="host-pw-test"]';
const connected = () => page.$(`${HOST} .dot-connected`).then(Boolean);
const trustIfAsked = async () => {
  const t = await page.waitForSelector('[data-testid="trust-host"]', { timeout: 2500 }).catch(() => null);
  if (t) await t.click();
};
async function connect() {
  await page.click(HOST, { button: "right" });
  await page.click(".ctx-item > span:text-is('Connect')");
}
async function disconnect() {
  await page.click(HOST, { button: "right" });
  await page.click("text=Disconnect");
  await until(async () => !(await connected()), "disconnected");
}

try {
  await page.goto(url);
  console.log("add a password-only host");
  await page.click('[data-testid="add-host"]');
  await page.fill('[data-testid="quick-fill"]', "dev@127.0.0.1:2222");
  await page.fill('.host-form label:has-text("Label") input', "pw-test");
  await page.check('.host-form label:has-text("Only use these identity files") input');
  await page.uncheck('.host-form label:has-text("Open tmux workspace") input');
  await page.click('[data-testid="host-save"]');
  await page.waitForSelector(HOST);
  check((await page.getAttribute('[data-testid="vault-button"]', "title")).includes("not set up"), "vault starts not set up");

  console.log("first sign-in: remember → create vault");
  await page.click(HOST);
  await trustIfAsked();
  await page.waitForSelector('[data-testid="secret-prompt"]');
  check(await page.isChecked('[data-testid="secret-save"]'), "'Remember this password' is on by default");
  check((await page.textContent('[data-testid="secret-prompt"]')).includes("sets up an encrypted vault"), "says it will set up a vault");
  await page.fill('[data-testid="secret-input"]', "devpass");
  await snap(page, "password-prompt");
  await page.click('[data-testid="secret-ok"]');
  await page.waitForSelector('[data-testid="vault-new"]');
  await page.fill('[data-testid="vault-new"]', "short");
  await page.fill('[data-testid="vault-new-again"]', "short");
  await page.click('[data-testid="vault-create"]');
  await until(async () => (await page.textContent(".form-error"))?.includes("At least 8"), "too-short master password refused");
  await page.fill('[data-testid="vault-new"]', "correct horse battery");
  await page.fill('[data-testid="vault-new-again"]', "correct horse battery");
  await snap(page, "create-vault");
  await page.click('[data-testid="vault-create"]');
  await until(connected, "signed in");
  await until(async () => (await page.getAttribute('[data-testid="vault-button"]', "title")).includes("unlocked"), "vault is unlocked");

  console.log("vault manager lists it (no secret shown)");
  await page.click('[data-testid="vault-button"]');
  await page.waitForSelector('[data-testid="vault-entry-dev@127.0.0.1:2222"]');
  check(!(await page.textContent('[data-testid="vault-dialog"]')).includes("devpass"), "the password itself is never displayed");
  await snap(page, "vault-manager");
  await page.keyboard.press("Escape");

  console.log("reconnect: no prompt");
  await disconnect();
  await connect();
  await until(connected, "signed in again");
  check(!(await page.$('[data-testid="secret-prompt"]')), "no password prompt");

  console.log("locked vault: asks for the master password");
  await page.click('[data-testid="vault-button"]');
  await page.click('[data-testid="vault-lock"]');
  await until(async () => (await page.getAttribute('[data-testid="vault-button"]', "title")).includes("locked"), "locked");
  await page.keyboard.press("Escape");
  await disconnect();
  await connect();
  await page.waitForSelector('[data-testid="vault-unlock-prompt"]');
  await page.fill('[data-testid="vault-master"]', "wrong master pw");
  await page.click('[data-testid="vault-unlock"]');
  await until(async () => (await page.textContent('[data-testid="vault-unlock-prompt"]'))?.includes("Wrong master password"), "wrong master password: try again");
  await snap(page, "unlock-retry");
  await page.fill('[data-testid="vault-master"]', "correct horse battery");
  await page.click('[data-testid="vault-unlock"]');
  await until(connected, "signed in with the saved password after unlocking");
  check(!(await page.$('[data-testid="secret-prompt"]')), "never asked for the server password");

  console.log("forget");
  await page.click('[data-testid="vault-button"]');
  await page.click('[data-testid="vault-forget-dev@127.0.0.1:2222"]');
  await until(async () => !(await page.$('[data-testid="vault-entry-dev@127.0.0.1:2222"]')), "forgotten");
  await page.keyboard.press("Escape");
  await disconnect();
  await connect();
  await page.waitForSelector('[data-testid="secret-prompt"]');
  check(true, "asked again after forgetting");
  await page.click("text=Cancel");

  // Cleanup
  await page.click(HOST, { button: "right" });
  await page.click("text=Delete host");
  check(errors.length === 0, `no page errors (${errors.join("; ")})`);
  console.log("\nVAULT E2E PASSED");
} finally {
  await browser.close();
}
