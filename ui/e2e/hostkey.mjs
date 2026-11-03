// Headless test: a known host presenting a key of a new type gets the
// "identity changed" warning, and "Don't connect" refuses. Needs a dev server
// whose known_hosts has a different-type key for 127.0.0.1:2222.
import { chromium } from "playwright";
const url = process.env.MUXSCAPE_URL ?? "http://127.0.0.1:7878/?token=devtoken";
const shots = process.argv[2];
function check(cond, msg) { if (!cond) throw new Error(`FAILED: ${msg}`); console.log(`  ✓ ${msg}`); }
const browser = await chromium.launch();
const page = await (await browser.newContext({ viewport: { width: 1400, height: 860 } })).newPage();
try {
  await page.goto(url);
  await page.click('[data-testid="host-ubuntu-tmux34"]');
  await page.waitForSelector('[data-testid="reject-changed-host"]', { timeout: 15000 });
  const text = await page.textContent(".modal");
  check(text.includes("This server's identity changed"), "distinct warning, not the routine prompt");
  check(text.includes("ecdsa-sha2-nistp521 SHA256:"), "lists the key known for this server");
  check(!(await page.$('[data-testid="trust-host"]')), "no routine Trust & remember button");
  check(await page.evaluate(() => document.activeElement?.dataset?.testid === "reject-changed-host"), "Don't connect has focus (Enter refuses)");
  if (shots) await page.screenshot({ path: `${shots}/hostkey-warning.png` });
  await page.keyboard.press("Enter");
  await page.waitForSelector(".toast", { timeout: 10000 });
  check(!(await page.$('[data-testid="session-name"]')), "not connected");
  console.log("\nHOSTKEY E2E PASSED");
} finally { await browser.close(); }
