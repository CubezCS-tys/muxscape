// Headless browser test of port forwarding in the real UI, over real SSH.
//
// Prereqs: same as smoke.mjs (test servers, built UI, dev server running).
// Run:     node ui/e2e/ports.mjs [screenshot-dir]
//
// Services run on the disposable test server; every check fetches real
// HTTP through the tunnel (from the browser, or from node).

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

const ssh = (cmd) =>
  execFileSync("ssh", ["-i", key, "-p", "2222", "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null",
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
const get = async (port, p = "/") => (await fetch(`http://127.0.0.1:${port}${p}`)).text();

// Fixture: an HTTP server script that announces itself like a dev server.
ssh(`pkill -f 'perl /tmp/[m]uxscape-srv'; cat > /tmp/muxscape-srv.pl <<'PL'
use IO::Socket::INET;
my $port = shift;
my $s = IO::Socket::INET->new(LocalAddr => '127.0.0.1', LocalPort => $port, Listen => 10, ReuseAddr => 1) or die "listen: $!";
$| = 1;
print "  Local: http://localhost:$port/app?token=xyz\\n";
while (my $c = $s->accept) { my $r = <$c>; print $c "HTTP/1.0 200 OK\\r\\nContent-Type: text/html\\r\\nServer: MuxTest/1.0\\r\\n\\r\\n<title>Muxscape test $port</title>hello-from-$port $r"; close $c; }
PL
tmux kill-session -t portse2e 2>/dev/null; tmux new-session -d -s portse2e -x 140 -y 40`);

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1400, height: 860 } });
const page = await ctx.newPage();
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
try {
  await page.goto(url);
  await page.click('[data-testid="host-ubuntu-tmux34"]');
  const trust = await page.waitForSelector('[data-testid="trust-host"]', { timeout: 4000 }).catch(() => null);
  if (trust) await trust.click();
  await page.waitForSelector('[data-testid="session-portse2e"]');
  // Start clean: stop forwards left by an earlier (failed) run.
  for (let row; (row = await page.$('[data-testid^="side-tunnel-"]')); ) {
    await row.click({ button: "right" });
    await page.click("text=Stop forwarding");
    await sleep(300);
  }
  await page.click('[data-testid="session-portse2e"]');
  await page.waitForSelector(".xterm-helper-textarea");
  await sleep(500);

  console.log("a server starts in a pane → one-click open");
  await page.keyboard.type("perl /tmp/muxscape-srv.pl 18574\n");
  await page.waitForSelector('[data-testid="port-chip-18574"]', { timeout: 10000 });
  check((await page.textContent('[data-testid="port-chip-18574"]')).includes("/app?token=xyz"), "chip offers port 18574 with its token path");
  await snap(page, "chip");
  const [tab] = await Promise.all([ctx.waitForEvent("page"), page.click('[data-testid="port-open-18574"]')]);
  await until(async () => (await tab.textContent("body")).includes("hello-from-18574 GET /app?token=xyz"), "browser tab loaded the page through the tunnel");
  check(tab.url().startsWith("http://localhost:18574/app?token=xyz"), `tab URL is the local forward (${tab.url()})`);
  await tab.close();
  check(!(await page.$('[data-testid="port-chip-18574"]')), "chip dismissed once forwarded");
  await page.waitForSelector('[data-testid="side-tunnel-18574"]');
  check((await page.textContent('[data-testid="header-ports"]')).includes("1"), "header shows 1 forwarded port");

  console.log("Ports panel: what each port is and where it runs");
  ssh("cd /tmp; nohup perl /tmp/muxscape-srv.pl 18575 >/dev/null 2>&1 &");
  ssh("tmux new-window -t portse2e: -n other"); // the server's window is no longer current
  await page.click('[data-testid="header-ports"]');
  await page.waitForSelector('[data-testid="ports-panel"]');
  const row = (p) => page.textContent(`[data-testid="pp-row-${p}"]`).catch(() => "");
  await until(async () => (await row(18574)).includes("Muxscape test 18574"), "forwarded row shows the page title (probe)");
  const r74 = await row(18574);
  check(r74.includes("muxscape-srv.pl") && r74.includes("~"), `…the process and its folder (${r74})`);
  check(r74.includes("portse2e › 0:"), "…and the tmux window it runs in");
  check(!!(await page.$('[data-testid="pp-row-18574"].fwd')), "listed as forwarded");
  await until(async () => (await row(18575)).includes("Muxscape test 18575"), "a service started outside tmux is listed with its title");
  check((await row(18575)).includes("/tmp"), "…and its folder");
  const sections = await page.$$eval(".pp-section", (els) => els.map((e) => e.textContent));
  check(sections[0] === "Forwarded" && sections[1] === "Your services", `grouped: ${sections.join(" / ")}`);
  await until(async () => (await page.textContent('[data-testid="side-tunnel-18574"]')).includes("Muxscape test 18574"), "sidebar forward shows what it is");
  await page.click('[data-testid="pp-jump-18574"]');
  await until(() => ssh("tmux display -p -t portse2e: '#{window_index}'") === "0", "jump selected the window running the server");
  await page.click('[data-testid="pp-pin-18574"]');
  await until(async () => !!(await page.$('[data-testid="pp-pin-18574"].on')), "pinned");
  await snap(page, "panel");

  console.log("Ports panel: forward by number, stop");
  await page.fill('[data-testid="pp-input"]', "18575");
  await page.click('[data-testid="pp-add-only"]'); // forward without opening
  await page.waitForSelector('[data-testid="pp-row-18575"].fwd');
  check((await get(18575, "/x")).includes("hello-from-18575 GET /x"), "port added by number works");
  await page.click('[data-testid="pp-stop-18575"]');
  await until(async () => !(await page.$('[data-testid="pp-row-18575"].fwd')), "stopped from the panel (row stays, unforwarded)");
  await until(() => get(18575).then(() => false, () => true), "local port 18575 released");
  await page.hover('[data-testid="pp-row-18575"]'); // actions appear on hover
  const [t3] = await Promise.all([ctx.waitForEvent("page"), page.click('[data-testid="pp-forward-18575"]')]);
  await until(async () => (await t3.textContent("body")).includes("hello-from-18575 GET /"), "row's Open forwards and opens it");
  await t3.close();
  await page.waitForSelector('[data-testid="pp-row-18575"].fwd');
  await page.click('[data-testid="pp-stop-18575"]');
  await page.click('[data-testid="header-ports"]');
  await until(async () => !(await page.$('[data-testid="ports-panel"]')), "header button closes the panel");

  console.log("Ctrl+click a bare localhost link (as agents print them) forwards and opens it");
  ssh("nohup perl /tmp/muxscape-srv.pl 18576 >/dev/null 2>&1 &");
  await page.click(".xterm-screen");
  await page.keyboard.press("Control+c");
  await page.keyboard.type("clear; echo see localhost:18576/clicked for the plan\n");
  // Exact screen position of "localhost" inside the visible echoed line.
  let pt = null;
  await until(async () => {
    pt = await page.evaluate(() => {
      for (const r of document.querySelectorAll(".workspace .xterm-rows > div")) {
        if (!r.getBoundingClientRect().height || !r.textContent.trimStart().startsWith("see localhost:18576/clicked")) continue;
        const walker = document.createTreeWalker(r, NodeFilter.SHOW_TEXT);
        for (let n; (n = walker.nextNode()); ) {
          const i = n.textContent.indexOf("localhost");
          if (i < 0) continue;
          const range = document.createRange();
          range.setStart(n, i + 2);
          range.setEnd(n, i + 3);
          const b = range.getBoundingClientRect();
          return { x: b.x + b.width / 2, y: b.y + b.height / 2 };
        }
      }
      return null;
    });
    return !!pt;
  }, "link line rendered");
  const { x, y } = pt;
  await page.mouse.move(x, y);
  await until(async () => (await page.evaluate(([x, y]) => getComputedStyle(document.elementFromPoint(x, y)).cursor, [x, y])) === "pointer", "link is hoverable");
  await until(async () => (await page.textContent(".link-tip"))?.includes("forwards port 18576"), "hover hint: Ctrl+click, forwards the port");
  // A plain click (e.g. while selecting) opens nothing.
  const stray = ctx.waitForEvent("page", { timeout: 1500 }).then(() => true, () => false);
  await page.mouse.click(x, y);
  check(!(await stray), "plain click does not open anything");
  check(!(await page.$('[data-testid="side-tunnel-18576"]')), "…nor forward the port");
  await page.mouse.move(x, y + 1);
  const [tab2] = await Promise.all([
    ctx.waitForEvent("page", { timeout: 8000 }),
    (async () => {
      await page.keyboard.down("Control");
      await page.mouse.click(x, y);
      await page.keyboard.up("Control");
    })(),
  ]);
  await until(async () => (await tab2.textContent("body")).includes("hello-from-18576 GET /clicked"), "link opened through a new forward");
  await tab2.close();
  await page.waitForSelector('[data-testid="side-tunnel-18576"]');

  console.log("tunnels outlive a page reload");
  await page.reload();
  await page.waitForSelector('[data-testid="side-tunnel-18574"]', { timeout: 10000 });
  check(!!(await page.$('[data-testid="side-tunnel-18576"]')), "both forwards still listed after reload");
  check((await get(18576, "/after-reload")).includes("GET /after-reload"), "still forwarding after reload");
  // The 18574 server was stopped (Ctrl+C above): the forward says so.
  await get(18574).catch(() => null);
  await until(async () => !!(await page.$('[data-testid="side-tunnel-18574"].has-error')), "dead service is flagged on its forward");
  check((await page.getAttribute('[data-testid="side-tunnel-18574"]', "title")).includes("nothing is listening"), "with a clear reason");
  await snap(page, "sidebar");

  // Cleanup (also unpins).
  for (const p of [18574, 18576]) {
    await page.click(`[data-testid="side-tunnel-${p}"]`, { button: "right" });
    await page.click("text=Stop forwarding");
    await until(async () => !(await page.$(`[data-testid="side-tunnel-${p}"]`)), `stopped ${p} from the sidebar`);
  }
  check(errors.length === 0, `no page errors (${errors.join("; ")})`);
  console.log("\nPORTS E2E PASSED");
} finally {
  ssh("pkill -f 'perl /tmp/[m]uxscape-srv'; tmux kill-session -t portse2e 2>/dev/null; true");
  await browser.close();
}
