import { useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import * as A from "../actions";
import { sameView, useStore } from "../store";
import type { Host, SessionInfo } from "../types";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import * as I from "./icons";

function StatusDot({ hostId }: { hostId: string }) {
  const c = useStore((s) => s.conn[hostId]);
  const state = c?.state ?? "disconnected";
  return <span className={`dot dot-${state}`} title={c?.message ? `${state}: ${c.message}` : state} />;
}

function SessionRow({ host, s }: { host: Host; s: SessionInfo }) {
  const att = useStore((st) => Object.values(st.attachments).find((a) => a.hostId === host.id && a.state?.id === s.id));
  const isActive = useStore((st) => !!att && sameView(st.active, { kind: "att", id: att.id }));
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const items: MenuItem[] = [
    { label: att ? "Show" : "Attach", action: () => A.attach(host.id, { kind: "existing", session: s.id }) },
    ...(att ? [{ label: "Rename…", action: () => A.renameSession(att.id, s.name) }, { label: "Detach", action: () => A.detach(att.id) }] : []),
    { label: "Kill session…", danger: true, action: () => A.killSession(host.id, s) },
  ];
  return (
    <div
      className={`tree-row session ${isActive ? "active" : ""} ${att ? "open" : ""}`}
      onClick={() => A.attach(host.id, { kind: "existing", session: s.id })}
      onContextMenu={(e) => {
        e.preventDefault();
        setMenu({ x: e.clientX, y: e.clientY });
      }}
      title={`${s.name} — ${s.windows} windows, ${s.attached} client(s) attached`}
      data-testid={`session-${s.name}`}
    >
      <I.Layers className="row-icon" />
      <span className="row-label">{s.name}</span>
      <span className="row-meta">{s.windows}</span>
      {menu && <ContextMenu {...menu} items={items} onClose={() => setMenu(null)} />}
    </div>
  );
}

function HostNode({ host }: { host: Host }) {
  const expanded = useStore((s) => !!s.expanded[host.id]);
  const server = useStore((s) => s.servers[host.id]);
  const state = useStore((s) => s.conn[host.id]?.state ?? "disconnected");
  const shells = useStore(useShallow((s) => Object.values(s.shells).filter((x) => x.hostId === host.id)));
  const activeView = useStore((s) => s.active);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const connected = state === "connected" || state === "reconnecting";
  const toggle = () => useStore.setState((s) => ({ expanded: { ...s.expanded, [host.id]: !s.expanded[host.id] } }));

  const items: MenuItem[] = [
    connected
      ? { label: "Disconnect", action: () => A.disconnect(host.id) }
      : { label: "Connect", action: () => A.connect(host.id) },
    { label: "Open tmux workspace", action: () => A.quickOpen(host.id) },
    { label: "New tmux session…", action: () => A.newSession(host.id) },
    { label: "Plain shell (no tmux)", action: () => A.openShell(host.id) },
    { label: "Refresh sessions", action: () => A.refreshSessions(host.id) },
    { label: host.source.kind === "sshConfig" ? "Save as editable host…" : "Edit host…", action: () => useStore.setState({ hostDialog: host }) },
    ...(host.source.kind === "saved" ? [{ label: "Delete host", danger: true, action: () => A.deleteHost(host.id) }] : []),
  ];

  return (
    <div className="host-node">
      <div
        className={`tree-row host ${expanded ? "expanded" : ""}`}
        onClick={() => {
          // Expanding always (idempotently) connects and fetches sessions.
          if (expanded) toggle();
          else if (state !== "connecting") A.connect(host.id);
        }}
        onDoubleClick={() => A.quickOpen(host.id)}
        onContextMenu={(e) => {
          e.preventDefault();
          setMenu({ x: e.clientX, y: e.clientY });
        }}
        data-testid={`host-${host.label}`}
      >
        <I.Chevron className={`chev ${expanded ? "open" : ""}`} onClick={(e) => { e.stopPropagation(); toggle(); }} />
        <StatusDot hostId={host.id} />
        <span className="row-label">{host.label}</span>
        <span className="row-meta small">{host.user ? `${host.user}@` : ""}{host.hostname}</span>
        {menu && <ContextMenu {...menu} items={items} onClose={() => setMenu(null)} />}
      </div>
      {expanded && (
        <div className="tree-children">
          {state === "connecting" && <div className="tree-hint">Connecting…</div>}
          {server?.tmux.kind === "notInstalled" && <div className="tree-hint">tmux not installed</div>}
          {server?.sessions.map((s) => <SessionRow key={s.id} host={host} s={s} />)}
          {shells.map((sh) => (
            <div
              key={sh.id}
              className={`tree-row session ${sameView(activeView, { kind: "shell", id: sh.id }) ? "active" : ""}`}
              onClick={() => useStore.setState({ active: { kind: "shell", id: sh.id } })}
            >
              <I.Terminal className="row-icon" />
              <span className="row-label">Shell {sh.id}</span>
              {sh.closed && <span className="row-meta">closed</span>}
            </div>
          ))}
          {connected && (
            <div className="tree-actions">
              {server?.tmux.kind !== "notInstalled" && (
                <button className="link" onClick={() => A.newSession(host.id)}>+ session</button>
              )}
              <button className="link" onClick={() => A.openShell(host.id)}>+ shell</button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export function Sidebar() {
  const hosts = useStore((s) => s.hosts);
  const folders = useStore((s) => s.folders);
  const [filter, setFilter] = useState("");
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});

  const grouped = useMemo(() => {
    const f = filter.toLowerCase();
    const visible = hosts.filter(
      (h) => !f || h.label.toLowerCase().includes(f) || h.hostname.toLowerCase().includes(f) || h.folder.toLowerCase().includes(f),
    );
    const order = ["", ...folders.filter((x) => x)];
    const groups = new Map<string, Host[]>();
    for (const name of order) groups.set(name, []);
    for (const h of visible) groups.set(h.folder, [...(groups.get(h.folder) ?? []), h]);
    return [...groups.entries()].filter(([, hs]) => hs.length > 0);
  }, [hosts, folders, filter]);

  return (
    <aside className="sidebar">
      <div className="sidebar-head">
        <div className="brand">
          <span className="brand-mark" />
          Muxscape
        </div>
        <button className="icon-btn" title="Add host" onClick={() => useStore.setState({ hostDialog: "new" })} data-testid="add-host">
          <I.Plus />
        </button>
      </div>
      <div className="sidebar-search">
        <I.Search />
        <input placeholder="Filter hosts" value={filter} onChange={(e) => setFilter(e.target.value)} spellCheck={false} />
      </div>
      <div className="tree">
        {grouped.length === 0 && (
          <div className="empty-hosts">
            <p>No hosts yet.</p>
            <button className="primary" onClick={() => useStore.setState({ hostDialog: "new" })}>Add a host</button>
            <p className="muted">Hosts from ~/.ssh/config appear here automatically.</p>
          </div>
        )}
        {grouped.map(([folder, hs]) => (
          <div key={folder || "_root"} className="folder">
            {folder && (
              <div className="folder-row" onClick={() => setCollapsed((c) => ({ ...c, [folder]: !c[folder] }))}>
                <I.Chevron className={`chev ${collapsed[folder] ? "" : "open"}`} />
                <I.Folder className="row-icon" />
                <span>{folder}</span>
              </div>
            )}
            {!collapsed[folder] && hs.map((h) => <HostNode key={h.id} host={h} />)}
          </div>
        ))}
      </div>
    </aside>
  );
}
