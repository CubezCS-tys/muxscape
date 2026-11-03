import { useEffect, useRef, useState } from "react";
import * as A from "../actions";
import type { WindowState } from "../types";
import { useStore } from "../store";
import { leaves } from "../layout";
import * as I from "./icons";

function Tab({ w, onRename }: { w: WindowState; onRename: (w: WindowState) => void }) {
  const ref = useRef<HTMLDivElement>(null);
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

export function TabBar({ windows }: { windows: WindowState[] }) {
  const current = windows.find((w) => w.active);
  // Pane actions (zoom / close) only make sense in a split window.
  const split = !!current && leaves(current.layout).length > 1;
  const zoomed = !!current?.zoomed;
  const [renaming, setRenaming] = useState<string | null>(null);
  const strip = useRef<HTMLDivElement>(null);
  return (
    <div className="tabbar">
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
            <Tab key={w.id} w={w} onRename={(x) => setRenaming(x.id)} />
          ),
        )}
        <button className="tab-new" title="New window (Ctrl+Shift+T)" onClick={() => A.newWindow()} data-testid="new-window">
          <I.Plus />
        </button>
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
        <button
          className="icon-btn with-count"
          title="Go to window… (Ctrl+Shift+P)"
          onClick={() => useStore.setState({ switcherOpen: true })}
          data-testid="open-switcher"
        >
          <I.Search />
          <span>{windows.length}</span>
        </button>
      </div>
    </div>
  );
}
