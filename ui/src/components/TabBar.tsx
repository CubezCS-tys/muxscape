import { useEffect, useRef, useState } from "react";
import * as A from "../actions";
import type { WindowState } from "../types";
import { POPOUT, useStore, visibleWindows } from "../store";
import { paneKey, previewLines } from "../terminals";
import { leaves } from "../layout";
import { ContextMenu } from "./ContextMenu";
import * as I from "./icons";

/** Drag data marking "a popped-out tab": the main tab bar accepts it. */
const TAB_MIME = "application/x-muxscape-tab";

/** What follows the cursor while dragging a tab: the tab and the last lines
 * of its terminal, so you see what you're carrying (also outside the window). */
function dragGhost(att: number, w: WindowState): HTMLElement {
  const el = document.createElement("div");
  el.className = "drag-ghost";
  const head = document.createElement("div");
  head.className = "drag-ghost-tab";
  head.textContent = `${w.index}  ${w.name}`;
  const body = document.createElement("pre");
  body.className = "drag-ghost-body";
  const pane = w.active_pane ?? leaves(w.visible_layout)[0]?.pane;
  body.textContent = pane ? previewLines(paneKey(att, pane), 9).join("\n") : "";
  el.append(head, body);
  document.body.appendChild(el);
  return el;
}

/** A drag that ended outside this window (dropping a tab on the desktop). */
function outsideWindow(e: React.DragEvent) {
  const x = e.screenX, y = e.screenY;
  if (x === 0 && y === 0) return false; // cancelled drags report 0,0
  return x < window.screenX || y < window.screenY || x > window.screenX + window.outerWidth || y > window.screenY + window.outerHeight;
}

function Tab({ att, w, onRename }: { att: number; w: WindowState; onRename: (w: WindowState) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  useEffect(() => {
    if (w.active) ref.current?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [w.active]);
  return (
    <div
      ref={ref}
      className={`tab ${w.active ? "active" : ""} ${w.bell ? "bell" : ""} ${!w.active && w.activity ? "activity" : ""}`}
      onMouseDown={(e) => {
        if (e.button === 1) {
          e.preventDefault();
          A.killWindow(w.id);
        }
      }}
      onClick={() => !w.active && A.selectWindow(w.id)}
      onDoubleClick={() => onRename(w)}
      onContextMenu={(e) => {
        e.preventDefault();
        setMenu({ x: e.clientX, y: e.clientY });
      }}
      // Drag a tab out of the window to open it in a new one.
      draggable
      onDragStart={(e) => {
        e.dataTransfer.setData("text/plain", w.name);
        // From a pop-out: dropping on the main window's tab bar moves it back.
        if (POPOUT) e.dataTransfer.setData(TAB_MIME, String(POPOUT.attachment));
        e.dataTransfer.effectAllowed = "move";
        const ghost = dragGhost(att, w);
        e.dataTransfer.setDragImage(ghost, 28, 18);
        setTimeout(() => ghost.remove(), 0); // the image is captured now
      }}
      onDragEnd={(e) => {
        // Main window: dropped outside → its own window. (A pop-out's tab
        // dropped on the main window is handled by that window.)
        if (!POPOUT && e.dataTransfer.dropEffect === "none" && outsideWindow(e)) A.popOut(w.id, { x: e.screenX, y: e.screenY });
      }}
      title={`${w.index}: ${w.name}${w.zoomed ? " (zoomed)" : ""}`}
      data-testid={`tab-${w.index}`}
      role="tab"
      aria-selected={w.active}
    >
      <span className="tab-index">{w.index}</span>
      <span className="tab-name">{w.name || "(unnamed)"}</span>
      {w.zoomed && <I.Zoom className="tab-flag" />}
      <button
        className="tab-close"
        title="Close window"
        onClick={(e) => {
          e.stopPropagation();
          A.killWindow(w.id);
        }}
      >
        <I.Close width={12} height={12} />
      </button>
      {menu && (
        <ContextMenu
          {...menu}
          onClose={() => setMenu(null)}
          items={[
            POPOUT
              ? { label: "Move back to main window", hint: "or drag it back", testid: "tab-moveback", action: () => A.moveBackFromPopout() }
              : { label: "Open in new window", hint: "or drag the tab out", testid: "tab-popout", action: () => A.popOut(w.id) },
            { label: "Rename…", action: () => onRename(w) },
            { label: "Close window", danger: true, sep: true, action: () => A.killWindow(w.id) },
          ]}
        />
      )}
    </div>
  );
}

function RenameField({ w, done }: { w: WindowState; done: () => void }) {
  const [v, setV] = useState(w.name);
  return (
    <input
      className="tab-rename"
      autoFocus
      value={v}
      onChange={(e) => setV(e.target.value)}
      onFocus={(e) => e.target.select()}
      onBlur={done}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          if (v.trim() && v !== w.name) A.renameWindow(w.id, v.trim());
          done();
        } else if (e.key === "Escape") done();
      }}
      data-testid="tab-rename"
    />
  );
}

export function TabBar({ att, windows: all }: { att: number; windows: WindowState[] }) {
  // Popped-out tabs live in their own window; a pop-out shows just its tab.
  const popped = useStore((s) => s.popped);
  const windows = visibleWindows(att, all, popped);
  const current = windows.find((w) => w.active);
  useEffect(() => {
    A.keepPopoutOnItsTab();
  }, [all]);
  // Pane actions (zoom / close) only make sense in a split window.
  const split = !!current && leaves(current.layout).length > 1;
  const zoomed = !!current?.zoomed;
  const [renaming, setRenaming] = useState<string | null>(null);
  const [dropHint, setDropHint] = useState(false);
  const strip = useRef<HTMLDivElement>(null);
  // The main window's tab bar takes popped-out tabs back.
  const accepts = (e: React.DragEvent) => !POPOUT && e.dataTransfer.types.includes(TAB_MIME);
  return (
    <div
      className={`tabbar ${dropHint ? "drop-target" : ""}`}
      onDragOver={(e) => {
        if (!accepts(e)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
        setDropHint(true);
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node)) setDropHint(false);
      }}
      onDrop={(e) => {
        if (!accepts(e)) return;
        e.preventDefault();
        setDropHint(false);
        const id = Number(e.dataTransfer.getData(TAB_MIME));
        if (id) A.moveBack(id);
      }}
      data-testid="tabbar"
    >
      {dropHint && <div className="drop-hint">Drop to move it back here</div>}
      <div
        className="tabs"
        ref={strip}
        role="tablist"
        onWheel={(e) => {
          if (strip.current && Math.abs(e.deltaY) > Math.abs(e.deltaX)) strip.current.scrollLeft += e.deltaY;
        }}
      >
        {windows.map((w) =>
          renaming === w.id ? (
            <div key={w.id} className="tab active">
              <span className="tab-index">{w.index}</span>
              <RenameField w={w} done={() => setRenaming(null)} />
            </div>
          ) : (
            <Tab key={w.id} att={att} w={w} onRename={(x) => setRenaming(x.id)} />
          ),
        )}
        {!POPOUT && (
          <button className="tab-new" title="New window (Ctrl+Shift+T)" onClick={() => A.newWindow()} data-testid="new-window">
            <I.Plus />
          </button>
        )}
      </div>
      <div className="tab-tools">
        <button className="icon-btn" title="Split right (Ctrl+Shift+D)" onClick={() => A.split(true)} data-testid="split-right">
          <I.SplitRight />
        </button>
        <button className="icon-btn" title="Split down (Ctrl+Shift+E)" onClick={() => A.split(false)} data-testid="split-down">
          <I.SplitDown />
        </button>
        {split && (
          <>
            <button className={`icon-btn ${zoomed ? "pressed" : ""}`} title="Zoom the focused pane (Ctrl+Shift+Z)" onClick={() => A.zoomPane()} data-testid="zoom-pane">
              <I.Zoom />
            </button>
            <button className="icon-btn" title="Close the focused pane (Ctrl+Shift+W)" onClick={() => A.killPane()} data-testid="close-pane">
              <I.Close />
            </button>
          </>
        )}
        {!POPOUT && <button
          className="icon-btn with-count"
          title="Go to window… (Ctrl+Shift+P)"
          onClick={() => useStore.setState({ switcherOpen: true })}
          data-testid="open-switcher"
        >
          <I.Search />
          <span>{windows.length}</span>
        </button>}
      </div>
    </div>
  );
}
