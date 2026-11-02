// Port forwarding UI: "server just started" chips, and the Ports side panel
// (what's listening on a server, what it is, where it was started; forward,
// open, pin).

import { useEffect, useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import * as A from "../actions";
import { dismissDetected, probeKey, useStore } from "../store";
import type { HostId, ListeningPort, PortProbe, TunnelInfo } from "../types";
import * as I from "./icons";

/** Offers shown when a pane prints e.g. `http://localhost:5173/`. */
export function PortChips() {
  const detected = useStore((s) => s.detected);
  const hosts = useStore((s) => s.hosts);
  return (
    <>
      {detected.map((d) => {
        const label = hosts.find((h) => h.id === d.hostId)?.label ?? d.hostId;
        return (
          <div key={`${d.hostId}:${d.port}`} className="toast port-chip" data-testid={`port-chip-${d.port}`}>
            <I.Plug className="port-chip-icon" />
            <div className="port-chip-text">
              <div><b>Port {d.port}</b> is up on {label}</div>
              {d.path && <div className="muted small ellipsis">{d.path}</div>}
            </div>
            <button className="primary" onClick={() => A.forwardPort(d.hostId, d.port, { path: d.path })} data-testid={`port-open-${d.port}`}>
              Open
            </button>
            <button className="ghost" title="Forward without opening a browser" onClick={() => A.forwardPort(d.hostId, d.port, { path: d.path, open: false })}>
              Forward
            </button>
            <button className="icon-btn" title="Dismiss" onClick={() => dismissDetected(d.hostId, d.port)}>
              <I.Close width={13} height={13} />
            </button>
          </div>
        );
      })}
    </>
  );
}

/** "5173" or "8080:5173" (local:remote, like ssh -L). */
function parseSpec(s: string): { remote: number; local: number | null } | null {
  const m = /^\s*(?:(\d{1,5})\s*:\s*)?(\d{1,5})\s*$/.exec(s);
  if (!m) return null;
  const remote = Number(m[2]);
  const local = m[1] ? Number(m[1]) : null;
  const ok = (p: number) => p >= 1 && p <= 65535;
  return ok(remote) && (local === null || ok(local)) ? { remote, local } : null;
}

interface Row {
  port: number;
  lp?: ListeningPort;
  probe?: PortProbe;
  tunnel?: TunnelInfo;
}

/** The one line that says what a port is. */
function headline(r: Row): { text: string; dim: boolean } {
  const p = r.probe;
  if (p?.title) return { text: p.title, dim: false };
  if (p?.label) return { text: p.label, dim: false };
  if (r.lp?.process) return { text: r.lp.process, dim: false };
  if (r.lp?.container) return { text: r.lp.container, dim: false };
  if (!p) return { text: "…", dim: true };
  if (p.error) return { text: p.error, dim: true };
  if (p.status) return { text: `HTTP ${p.status}`, dim: true };
  return { text: "Unknown service", dim: true };
}

function PortRow({ hostId, r }: { hostId: HostId; r: Row }) {
  const { port, lp, probe, tunnel } = r;
  const web = probe ? probe.web : true; // unknown yet: assume a web page
  const head = headline(r);
  // A second, smaller name when the headline is a title: "Vite", "FastAPI".
  const tag = probe?.title && probe.label ? probe.label : null;
  const where = [lp?.process && lp.process !== head.text ? lp.process : null, lp?.cwd].filter(Boolean).join(" · ");
  const act = () => {
    if (tunnel) return web ? A.openTunnel(tunnel) : A.copyAddress(tunnel);
    return A.forwardPort(hostId, port, { open: web });
  };
  return (
    <div
      className={`pp-row ${tunnel ? "fwd" : ""} ${tunnel?.error ? "has-error" : ""}`}
      onClick={act}
      title={lp?.command ?? undefined}
      data-testid={`pp-row-${port}`}
    >
      <div className="pp-line1">
        <span className={`pp-dot ${tunnel ? (tunnel.error ? "warn" : "on") : ""}`} />
        <span className="pp-port">{port}</span>
        <span className={`pp-title ${head.dim ? "dim" : ""}`}>{head.text}</span>
        {tag && <span className="pp-tag">{tag}</span>}
        {probe?.tls && <span className="pp-tag">https</span>}
        {lp?.public && <span className="pp-tag warn" title={`Listening on ${lp.address}: reachable from other machines unless a firewall blocks it`}>public</span>}
        <div className="pp-actions" onClick={(e) => e.stopPropagation()}>
          {tunnel ? (
            <>
              {web && (
                <button className="icon-btn" title={`Open localhost:${tunnel.localPort}`} onClick={() => A.openTunnel(tunnel)} data-testid={`pp-open-${port}`}>
                  <I.External width={14} height={14} />
                </button>
              )}
              <button className="icon-btn" title={web ? "Copy URL" : `Copy localhost:${tunnel.localPort}`} onClick={() => (web ? A.copyTunnelUrl(tunnel) : A.copyAddress(tunnel))}>
                <I.Copy width={14} height={14} />
              </button>
              <button
                className={`icon-btn pin ${tunnel.pinned ? "on" : ""}`}
                title={tunnel.pinned ? "Pinned: re-opens whenever this host connects (click to unpin)" : "Pin: re-open whenever this host connects"}
                onClick={() => A.pinTunnel(tunnel, !tunnel.pinned)}
                data-testid={`pp-pin-${port}`}
              >
                <I.Pin width={14} height={14} />
              </button>
              <button className="icon-btn" title="Stop forwarding" onClick={() => A.closeTunnel(tunnel)} data-testid={`pp-stop-${port}`}>
                <I.Close width={14} height={14} />
              </button>
            </>
          ) : web ? (
            <button className="pp-btn" onClick={() => A.forwardPort(hostId, port)} data-testid={`pp-forward-${port}`}>
              <I.External width={13} height={13} /> Open
            </button>
          ) : (
            <button className="pp-btn" title="Forward (not a web page: connect with its own client)" onClick={() => A.forwardPort(hostId, port, { open: false })} data-testid={`pp-forward-${port}`}>
              <I.Plug width={13} height={13} /> Forward
            </button>
          )}
        </div>
      </div>
      {(where || lp?.pane || lp?.container || tunnel || probe?.redirect) && (
        <div className="pp-line2">
          {tunnel && tunnel.localPort !== port && <span className="pp-local">on localhost:{tunnel.localPort}</span>}
          {where && <span className="pp-where">{where}</span>}
          {lp?.pane && (
            <button
              className="pp-pane"
              title="Show the tmux window this runs in"
              onClick={(e) => {
                e.stopPropagation();
                A.jumpToPane(hostId, lp.pane!);
              }}
              data-testid={`pp-jump-${port}`}
            >
              <I.Terminal width={12} height={12} />
              {lp.pane.sessionName} › {lp.pane.windowIndex}:{lp.pane.windowName}
            </button>
          )}
          {lp?.container && head.text !== lp.container && <span className="pp-where">container {lp.container}</span>}
          {probe?.redirect && <span className="pp-where">→ {probe.redirect}</span>}
        </div>
      )}
      {tunnel?.error && <div className="pp-error">{tunnel.error}</div>}
    </div>
  );
}

function matches(r: Row, q: string) {
  if (!q) return true;
  const hay = [
    r.port, r.probe?.title, r.probe?.label, r.probe?.server, r.lp?.process, r.lp?.command, r.lp?.cwd,
    r.lp?.container, r.lp?.pane?.sessionName, r.lp?.pane?.windowName,
  ].filter(Boolean).join(" ").toLowerCase();
  return q.toLowerCase().split(/\s+/).every((w) => hay.includes(w));
}

/** Right-hand panel: everything listening on a host, forwarded first. */
export function PortsPanel() {
  const hosts = useStore((s) => s.hosts);
  const conn = useStore((s) => s.conn);
  const chosen = useStore((s) => s.portsHost);
  const activeHost = useStore(() => A.activeHostId());
  const hostId = chosen ?? activeHost ?? hosts[0]?.id ?? null;
  const host = hosts.find((h) => h.id === hostId);
  const state = hostId ? conn[hostId]?.state ?? "disconnected" : "disconnected";
  const info = useStore((s) => (hostId ? s.portInfo[hostId] : undefined));
  const tunnels = useStore(useShallow((s) => s.tunnels.filter((t) => t.hostId === hostId)));
  const [spec, setSpec] = useState("");
  const [filter, setFilter] = useState("");
  const [showOther, setShowOther] = useState(false);
  const parsed = parseSpec(spec);

  // Scan on open / host change, then keep it fresh while visible.
  useEffect(() => {
    if (!hostId || state !== "connected") return;
    A.refreshPorts(hostId);
    const t = setInterval(() => A.refreshPorts(hostId), 15000);
    return () => clearInterval(t);
  }, [hostId, state]);
  // A new forward: probe it if the scan can't see it.
  useEffect(() => {
    if (hostId && state === "connected" && tunnels.some((t) => !info?.ports.some((p) => p.port === t.remotePort))) A.refreshPorts(hostId);
  }, [tunnels.length]); // eslint-disable-line react-hooks/exhaustive-deps

  const { forwarded, yours, other } = useMemo(() => {
    const probes = info?.probes ?? {};
    const rows: Row[] = [];
    for (const t of tunnels) {
      const lp = info?.ports.find((p) => p.port === t.remotePort);
      rows.push({ port: t.remotePort, lp, tunnel: t, probe: probes[probeKey(t.remotePort, lp?.pid)] });
    }
    for (const lp of info?.ports ?? []) {
      if (!tunnels.some((t) => t.remotePort === lp.port)) rows.push({ port: lp.port, lp, probe: probes[probeKey(lp.port, lp.pid)] });
    }
    const vis = rows.filter((r) => matches(r, filter.trim()));
    return {
      forwarded: vis.filter((r) => r.tunnel),
      yours: vis.filter((r) => !r.tunnel && r.lp?.mine),
      other: vis.filter((r) => !r.tunnel && !r.lp?.mine),
    };
  }, [info, tunnels, filter]);

  const add = async (open: boolean) => {
    if (!parsed || !hostId) return;
    const t = await A.forwardPort(hostId, parsed.remote, { localPort: parsed.local, open });
    if (t) setSpec("");
  };
  const otherOpen = showOther || !!filter.trim() || yours.length === 0;
  const total = (info?.ports.length ?? 0) + tunnels.filter((t) => !info?.ports.some((p) => p.port === t.remotePort)).length;

  return (
    <aside className="ports-panel" data-testid="ports-panel">
      <div className="pp-head">
        <I.Plug className="row-icon" />
        <span className="pp-head-title">Ports</span>
        <select
          className="pp-host"
          value={hostId ?? ""}
          onChange={(e) => useStore.setState({ portsHost: e.target.value })}
          title="Server"
          data-testid="pp-host"
        >
          {hosts.map((h) => (
            <option key={h.id} value={h.id}>
              {h.label}{conn[h.id]?.state === "connected" ? "" : " (offline)"}
            </option>
          ))}
        </select>
        <button className="icon-btn" title="Rescan" disabled={state !== "connected"} onClick={() => hostId && A.refreshPorts(hostId)} data-testid="pp-rescan">
          <I.Refresh width={13} height={13} className={info?.scanning ? "spin" : ""} />
        </button>
        <button className="icon-btn" title="Close (Ctrl+Shift+O)" onClick={() => useStore.setState({ portsOpen: false })}>
          <I.Close width={14} height={14} />
        </button>
      </div>

      {state !== "connected" ? (
        <div className="pp-empty">
          <p>{host ? `${host.label} isn't connected.` : "No host selected."}</p>
          {host && <button className="ghost" onClick={() => A.connect(host.id)}>Connect</button>}
        </div>
      ) : (
        <>
          <form
            className="pp-add"
            onSubmit={(e) => {
              e.preventDefault();
              add(true);
            }}
          >
            <input
              placeholder="Port, e.g. 5173 or 8080:5173"
              value={spec}
              onChange={(e) => setSpec(e.target.value)}
              spellCheck={false}
              data-testid="pp-input"
            />
            <button type="submit" className="pp-btn" disabled={!parsed} title="Forward and open in the browser" data-testid="pp-add">Open</button>
            <button type="button" className="pp-btn" disabled={!parsed} title="Forward only" onClick={() => add(false)} data-testid="pp-add-only">Forward</button>
          </form>
          {total > 6 && (
            <div className="pp-filter">
              <I.Search width={13} height={13} />
              <input placeholder="Filter: port, name, folder, session…" value={filter} onChange={(e) => setFilter(e.target.value)} spellCheck={false} data-testid="pp-filter" />
            </div>
          )}
          <div className="pp-list">
            {forwarded.length > 0 && (
              <>
                <div className="pp-section">Forwarded</div>
                {forwarded.map((r) => <PortRow key={`f${r.port}`} hostId={hostId!} r={r} />)}
              </>
            )}
            {yours.length > 0 && (
              <>
                <div className="pp-section">Your services</div>
                {yours.map((r) => <PortRow key={`y${r.port}`} hostId={hostId!} r={r} />)}
              </>
            )}
            {other.length > 0 && (
              <>
                <button className="pp-section toggle" onClick={() => setShowOther((v) => !v)} data-testid="pp-other-toggle">
                  <I.Chevron width={12} height={12} className={`chev ${otherOpen ? "open" : ""}`} />
                  Other services <span className="pp-count">{other.length}</span>
                </button>
                {otherOpen && other.map((r) => <PortRow key={`o${r.port}`} hostId={hostId!} r={r} />)}
              </>
            )}
            {info && !info.scanning && total === 0 && <div className="pp-empty"><p>Nothing is listening on this server (besides SSH).</p></div>}
            {!info && <div className="pp-empty"><p>Scanning…</p></div>}
            {filter && forwarded.length + yours.length + other.length === 0 && <div className="pp-empty"><p>No ports match “{filter}”.</p></div>}
          </div>
          <div className="pp-foot">Forwards listen on 127.0.0.1 only · {info?.scannedAt ? `scanned ${new Date(info.scannedAt).toLocaleTimeString()}` : ""}</div>
        </>
      )}
    </aside>
  );
}
