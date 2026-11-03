import { useEffect, useState } from "react";
import * as A from "./actions";
import { Overlays } from "./components/Overlays";
import { PortsPanel } from "./components/Ports";
import { Sidebar } from "./components/Sidebar";
import { TabBar } from "./components/TabBar";
import { Workspace } from "./components/Workspace";
import * as I from "./components/icons";
import { handleShortcut, SHORTCUTS } from "./shortcuts";
import { POPOUT, closeView, useStore } from "./store";

function Header() {
  const active = useStore((s) => s.active);
  const att = useStore((s) => (s.active?.kind === "att" ? s.attachments[s.active.id] : undefined));
  const shell = useStore((s) => (s.active?.kind === "shell" ? s.shells[s.active.id] : undefined));
  const hostId = att?.hostId ?? shell?.hostId;
  const host = useStore((s) => s.hosts.find((h) => h.id === hostId));
  const conn = useStore((s) => (hostId ? s.conn[hostId] : undefined));
  const [help, setHelp] = useState(false);
  const tunnelCount = useStore((s) => s.tunnels.filter((t) => t.hostId === hostId).length);
  const portsOpen = useStore((s) => s.portsOpen);
  if (!active) return <div className="header empty" />;
  const state = conn?.state ?? "disconnected";
  return (
    <div className="header">
      <div className="crumbs">
        <I.Server className="row-icon" />
        <span className="crumb-host">{host?.label ?? hostId}</span>
        <span className="crumb-sep">›</span>
        {att ? (
          POPOUT ? (
            // A pop-out shows the session it came from (its own hidden
            // companion session has an internal name).
            <span className="crumb-session static" data-testid="session-name">{POPOUT.name || "…"}</span>
          ) : (
            <button className="crumb-session" title="Rename session" onClick={() => att.state && A.renameSession(att.id, att.state.name)} data-testid="session-name">
              {att.state?.name ?? "…"}
            </button>
          )
        ) : (
          <span>Shell {shell?.id}{shell?.closed ? ` — ${shell.closed}` : ""}</span>
        )}
        <span className={`chip chip-${state}`}>{state}</span>
      </div>
      <div className="header-actions">
        <button className={`icon-btn with-count ${portsOpen ? "pressed" : ""}`} title="Ports & forwarding (Ctrl+Shift+O)" onClick={() => A.togglePorts()} data-testid="header-ports">
          <I.Plug /> Ports{tunnelCount > 0 && <span className="count-pill">{tunnelCount}</span>}
        </button>
        <button className="icon-btn" title="Keyboard shortcuts" onClick={() => setHelp((h) => !h)}>?</button>
        {att && POPOUT && (
          <button className="ghost" title="Put this tab back in the main window" onClick={() => A.moveBackFromPopout()} data-testid="move-back">
            <I.Unplug /> Move back
          </button>
        )}
        {att && !POPOUT && (
          <button className="ghost" title="Detach (the session keeps running on the server)" onClick={() => A.detach(att.id)}>
            <I.Unplug /> Detach
          </button>
        )}
        {shell && (
          <button className="ghost" onClick={() => { A.closeShell(shell.id); closeView({ kind: "shell", id: shell.id }); }}>
            <I.Close /> Close shell
          </button>
        )}
      </div>
      {help && (
        <div className="help-pop" onClick={() => setHelp(false)}>
          {SHORTCUTS.map(([k, v]) => (
            <div key={k} className="help-row"><kbd>{k}</kbd><span>{v}</span></div>
          ))}
        </div>
      )}
    </div>
  );
}

export function App() {
  const att = useStore((s) => (s.active?.kind === "att" ? s.attachments[s.active.id] : undefined));
  const portsOpen = useStore((s) => s.portsOpen);
  useEffect(() => {
    A.init();
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement;
      // Terminals route through xterm's handler; inputs keep their keys.
      if (t?.classList?.contains("xterm-helper-textarea")) return;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA")) return;
      handleShortcut(e);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  return (
    <div className="app">
      {!POPOUT && <Sidebar />}
      <main className="main">
        <Header />
        {att?.state && <TabBar att={att.id} windows={att.state.windows} />}
        <Workspace />
      </main>
      {portsOpen && <PortsPanel />}
      <Overlays />
    </div>
  );
}
