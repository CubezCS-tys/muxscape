import { useEffect, useMemo, useState } from "react";
import * as A from "../actions";
import { pop, useDialogs, type DialogSpec } from "../dialogs";
import { fuzzyScore } from "../layout";
import { useStore, visibleWindows, type Prompt } from "../store";
import type { Host } from "../types";
import { PortChips } from "./Ports";
import { SecretPrompt, VaultDialog, VaultUnlockPrompt } from "./Vault";
import { AppearanceDialog } from "./Appearance";
import { TILE_COLORS, initials, tileColor } from "../hostColors";

function Modal({ children, onClose, wide }: { children: React.ReactNode; onClose?: () => void; wide?: boolean }) {
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && onClose?.();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [onClose]);
  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose?.()}>
      <div className={`modal ${wide ? "wide" : ""}`} role="dialog">{children}</div>
    </div>
  );
}

// ------------------------------------------------------------ prompts

function PromptDialog({ p }: { p: Prompt }) {
  const host = useStore((s) => s.hosts.find((h) => h.id === p.hostId));
  const [values, setValues] = useState<string[]>(() => (p.request.kind === "keyboardInteractive" ? p.request.prompts.map(() => "") : []));
  const r = p.request;
  const cancel = () =>
    A.answerPrompt(
      p.prompt,
      r.kind === "hostKey" ? { kind: "hostKey", accept: false, save: false } : r.kind === "keyboardInteractive" ? { kind: "responses", values: null } : { kind: "secret", value: null },
    );

  if (r.kind === "hostKey") {
    return (
      <Modal onClose={cancel}>
        <h2>Unknown host key</h2>
        <p>
          The authenticity of <b>{r.info.host}:{r.info.port}</b>{host ? ` (${host.label})` : ""} can't be established yet.
          Verify this fingerprint with the server's administrator before trusting it.
        </p>
        <div className="fingerprint">
          <span>{r.info.algorithm}</span>
          <code data-testid="fingerprint">{r.info.fingerprint}</code>
        </div>
        <div className="modal-actions">
          <button onClick={cancel}>Cancel</button>
          <button onClick={() => A.answerPrompt(p.prompt, { kind: "hostKey", accept: true, save: false })}>Trust once</button>
          <button className="primary" data-testid="trust-host" onClick={() => A.answerPrompt(p.prompt, { kind: "hostKey", accept: true, save: true })}>
            Trust &amp; remember
          </button>
        </div>
      </Modal>
    );
  }
  if (r.kind === "keyboardInteractive") {
    return (
      <Modal onClose={cancel}>
        <h2>Authentication for {r.target}</h2>
        {r.instructions && <p>{r.instructions}</p>}
        <form onSubmit={(e) => { e.preventDefault(); A.answerPrompt(p.prompt, { kind: "responses", values }); }}>
          {r.prompts.map(([label, echo], i) => (
            <label key={i} className="field">
              <span>{label}</span>
              <input autoFocus={i === 0} type={echo ? "text" : "password"} value={values[i]} onChange={(e) => setValues(values.map((v, j) => (j === i ? e.target.value : v)))} />
            </label>
          ))}
          <div className="modal-actions">
            <button type="button" onClick={cancel}>Cancel</button>
            <button className="primary" type="submit">Continue</button>
          </div>
        </form>
      </Modal>
    );
  }
  // Passwords / passphrases (with "remember") and vault unlock.
  return r.kind === "vaultUnlock" ? <VaultUnlockPrompt p={p} /> : <SecretPrompt p={p} />;
}

// ------------------------------------------------------------ dialogs

function GenericDialog({ d }: { d: DialogSpec }) {
  const [v, setV] = useState(d.kind === "text" ? d.initial : "");
  const close = (result: boolean) => {
    pop(d);
    if (d.kind === "confirm") d.resolve(result);
    else d.resolve(result ? v : null);
  };
  return (
    <Modal onClose={() => close(false)}>
      <h2>{d.title}</h2>
      {d.message && <p>{d.message}</p>}
      <form onSubmit={(e) => { e.preventDefault(); close(true); }}>
        {d.kind === "text" && (
          <input className="wide-input" autoFocus value={v} onChange={(e) => setV(e.target.value)} onFocus={(e) => e.target.select()} data-testid="dialog-input" />
        )}
        <div className="modal-actions">
          <button type="button" onClick={() => close(false)}>Cancel</button>
          <button className={d.kind === "confirm" && d.danger ? "danger" : "primary"} type="submit" autoFocus={d.kind === "confirm"} data-testid="dialog-ok">
            {d.ok ?? "OK"}
          </button>
        </div>
      </form>
    </Modal>
  );
}

// --------------------------------------------------------- host editor

const emptyHost: Host = {
  id: "", label: "", folder: "", hostname: "", port: 22, user: "", identityFiles: [], identitiesOnly: false, jump: "", autoTmux: true,
  source: { kind: "saved" },
};

function HostDialog({ initial }: { initial: Host | "new" }) {
  const folders = useStore((s) => s.folders);
  const hosts = useStore((s) => s.hosts);
  const [h, setH] = useState<Host>(initial === "new" ? emptyHost : { ...initial, jump: initial.jump ?? "", user: initial.user ?? "" });
  const [quick, setQuick] = useState("");
  const upd = (patch: Partial<Host>) => setH((x) => ({ ...x, ...patch }));
  const close = () => useStore.setState({ hostDialog: null });
  const applyQuick = (s: string) => {
    setQuick(s);
    const m = s.match(/^(?:([^@\s]+)@)?([^:\s]+)(?::(\d+))?$/);
    if (m) upd({ user: m[1] ?? h.user, hostname: m[2], port: m[3] ? Number(m[3]) : 22, label: h.label || m[2] });
  };
  return (
    <Modal onClose={close} wide>
      <h2>{initial === "new" ? "Add host" : h.source.kind === "sshConfig" ? `Save "${h.label}" as editable host` : `Edit ${h.label}`}</h2>
      <form
        className="host-form"
        onSubmit={(e) => {
          e.preventDefault();
          A.saveHost({ ...h, user: h.user || null, jump: h.jump || null });
        }}
      >
        {initial === "new" && (
          <label className="field span2">
            <span>Quick fill</span>
            <input placeholder="user@host:port" value={quick} onChange={(e) => applyQuick(e.target.value)} autoFocus data-testid="quick-fill" />
          </label>
        )}
        <label className="field"><span>Label</span><input value={h.label} onChange={(e) => upd({ label: e.target.value })} /></label>
        <label className="field">
          <span>Folder</span>
          <input list="folders" placeholder="e.g. Work/Production" value={h.folder} onChange={(e) => upd({ folder: e.target.value })} />
          <datalist id="folders">{folders.map((f) => <option key={f} value={f} />)}</datalist>
        </label>
        <label className="field"><span>Hostname</span><input required value={h.hostname} onChange={(e) => upd({ hostname: e.target.value })} data-testid="host-hostname" /></label>
        <label className="field"><span>Port</span><input type="number" min={1} max={65535} value={h.port} onChange={(e) => upd({ port: Number(e.target.value) || 22 })} /></label>
        <label className="field"><span>User</span><input placeholder="(local user)" value={h.user ?? ""} onChange={(e) => upd({ user: e.target.value })} data-testid="host-user" /></label>
        <label className="field">
          <span>Jump host</span>
          <input list="jumphosts" placeholder="none, a saved host, or user@bastion:22" value={h.jump ?? ""} onChange={(e) => upd({ jump: e.target.value })} />
          <datalist id="jumphosts">{hosts.filter((x) => x.id !== h.id).map((x) => <option key={x.id} value={x.id}>{x.label}</option>)}</datalist>
        </label>
        <label className="field span2">
          <span>Identity files</span>
          <input
            placeholder="~/.ssh/id_ed25519 (comma separated; agent and default keys are tried too)"
            value={h.identityFiles.join(", ")}
            onChange={(e) => upd({ identityFiles: e.target.value.split(",").map((x) => x.trim()).filter(Boolean) })}
            data-testid="host-identity"
          />
        </label>
        <label className="check"><input type="checkbox" checked={h.identitiesOnly} onChange={(e) => upd({ identitiesOnly: e.target.checked })} /> Only use these identity files</label>
        <label className="check"><input type="checkbox" checked={h.autoTmux} onChange={(e) => upd({ autoTmux: e.target.checked })} /> Open tmux workspace on connect</label>
        <div className="field span2">
          <span>Colour</span>
          <div className="host-colors">
            <span className="host-tile big" style={{ "--c": tileColor(h.label || h.hostname || "?", h.color) } as React.CSSProperties}>
              {initials(h.label || h.hostname || "?")}
            </span>
            <button type="button" className={`ap-swatch auto ${!h.color ? "on" : ""}`} onClick={() => upd({ color: null })} title="From the name" style={{ "--sw": tileColor(h.label || h.hostname || "?") } as React.CSSProperties}>
              <span>Auto</span>
            </button>
            {TILE_COLORS.map((c) => (
              <button type="button" key={c} className={`ap-swatch ${h.color === c ? "on" : ""}`} onClick={() => upd({ color: c })} style={{ "--sw": c } as React.CSSProperties} aria-label={`Colour ${c}`} data-testid={`host-color-${c}`} />
            ))}
          </div>
        </div>
        <div className="modal-actions span2">
          <button type="button" onClick={close}>Cancel</button>
          <button className="primary" type="submit" data-testid="host-save">Save</button>
        </div>
      </form>
    </Modal>
  );
}

// ------------------------------------------------------ quick switcher

interface Item {
  label: string;
  detail: string;
  run: () => void;
}

function QuickSwitcher() {
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  const att = useStore((s) => (s.active?.kind === "att" ? s.attachments[s.active.id] : undefined));
  const servers = useStore((s) => s.servers);
  const hosts = useStore((s) => s.hosts);
  const tunnels = useStore((s) => s.tunnels);
  const close = () => useStore.setState({ switcherOpen: false });

  const items = useMemo(() => {
    const all: Item[] = [];
    for (const w of att?.state ? visibleWindows(att.id, att.state.windows) : []) {
      all.push({ label: `${w.index}: ${w.name}`, detail: "window", run: () => A.selectWindow(w.id) });
    }
    for (const t of tunnels) {
      const h = hosts.find((x) => x.id === t.hostId);
      all.push({ label: `localhost:${t.localPort}`, detail: `open — port ${t.remotePort} on ${h?.label ?? t.hostId}`, run: () => A.openTunnel(t) });
    }
    for (const h of hosts) {
      for (const s of (servers[h.id]?.sessions ?? []).filter((x) => !x.name.includes("_mx_"))) {
        all.push({ label: s.name, detail: `session on ${h.label}`, run: () => A.attach(h.id, { kind: "existing", session: s.id }) });
      }
      all.push({ label: h.label, detail: "host — open workspace", run: () => A.quickOpen(h.id) });
    }
    return all
      .map((it) => ({ it, score: fuzzyScore(q, it.label) }))
      .filter((x) => x.score !== null)
      .sort((a, b) => (q ? b.score! - a.score! : 0))
      .map((x) => x.it)
      .slice(0, 50);
  }, [q, att, servers, hosts, tunnels]);

  useEffect(() => setSel(0), [q]);
  const pick = (it?: Item) => {
    if (!it) return;
    close();
    it.run();
  };
  return (
    <Modal onClose={close}>
      <input
        className="wide-input switcher-input"
        autoFocus
        placeholder="Go to window, session or host…"
        value={q}
        onChange={(e) => setQ(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown") { e.preventDefault(); setSel((s) => Math.min(s + 1, items.length - 1)); }
          else if (e.key === "ArrowUp") { e.preventDefault(); setSel((s) => Math.max(s - 1, 0)); }
          else if (e.key === "Enter") { e.preventDefault(); pick(items[sel]); }
        }}
        data-testid="switcher-input"
      />
      <div className="switcher-list">
        {items.map((it, i) => (
          <div key={`${it.detail}-${it.label}-${i}`} className={`switcher-item ${i === sel ? "sel" : ""}`} onMouseEnter={() => setSel(i)} onClick={() => pick(it)}>
            <span>{it.label}</span>
            <span className="muted small">{it.detail}</span>
          </div>
        ))}
        {items.length === 0 && <div className="muted switcher-item">No matches</div>}
      </div>
    </Modal>
  );
}

export function Overlays() {
  const prompts = useStore((s) => s.prompts);
  const dialogs = useDialogs((s) => s.stack);
  const hostDialog = useStore((s) => s.hostDialog);
  const switcher = useStore((s) => s.switcherOpen);
  const toasts = useStore((s) => s.toasts);
  const vaultDialog = useStore((s) => s.vaultDialog);
  const appearanceOpen = useStore((s) => s.appearanceOpen);
  return (
    <>
      {hostDialog && <HostDialog key={typeof hostDialog === "string" ? "new" : hostDialog.id} initial={hostDialog} />}
      {switcher && <QuickSwitcher />}
      {vaultDialog && <VaultDialog />}
      {appearanceOpen && <AppearanceDialog />}
      {dialogs.map((d, i) => <GenericDialog key={i} d={d} />)}
      {prompts.slice(0, 1).map((p) => <PromptDialog key={p.prompt} p={p} />)}
      <div className="toasts">
        <PortChips />
        {toasts.map((t) => <div key={t.id} className={`toast ${t.level}`}>{t.message}</div>)}
      </div>
    </>
  );
}
