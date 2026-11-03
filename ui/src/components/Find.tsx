import { useEffect, useRef, useState } from "react";
import { useStore } from "../store";
import { getTerminal } from "../terminals";
import { activeTermKey } from "../shortcuts";
import * as I from "./icons";

const DECOR = {
  matchBackground: "#e3c58e55",
  matchOverviewRuler: "#e3c58e",
  activeMatchBackground: "#e3c58e",
  activeMatchColorOverviewRuler: "#ffb454",
};

/** Find in the focused terminal's scrollback (Ctrl+Shift+F). */
export function FindBar() {
  const [q, setQ] = useState("");
  const [caseSensitive, setCase] = useState(false);
  const [count, setCount] = useState<{ index: number; total: number } | null>(null);
  const input = useRef<HTMLInputElement>(null);
  // The pane we search is fixed when the bar opens.
  const [key] = useState(() => activeTermKey());
  const close = () => {
    if (key) {
      getTerminal(key).search.clearDecorations();
      getTerminal(key).term.focus();
    }
    useStore.setState({ findOpen: false });
  };

  useEffect(() => {
    input.current?.focus();
    if (!key) return;
    const sub = getTerminal(key).search.onDidChangeResults((r) => setCount(r.resultCount ? { index: r.resultIndex, total: r.resultCount } : { index: -1, total: 0 }));
    return () => {
      sub.dispose();
      getTerminal(key).search.clearDecorations();
    };
  }, [key]);

  const find = (back = false, incremental = false) => {
    if (!key) return;
    const s = getTerminal(key).search;
    if (!q) {
      s.clearDecorations();
      setCount(null);
      return;
    }
    const opts = { caseSensitive, incremental, decorations: DECOR };
    if (back) s.findPrevious(q, opts);
    else s.findNext(q, opts);
  };
  useEffect(() => find(false, true), [q, caseSensitive]); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <div className="find-bar" role="search" data-testid="find-bar">
      <I.Search width={13} height={13} />
      <input
        ref={input}
        value={q}
        placeholder="Find in this pane"
        spellCheck={false}
        onChange={(e) => setQ(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") close();
          else if (e.key === "Enter") find(e.shiftKey);
          else return;
          e.preventDefault();
        }}
        data-testid="find-input"
      />
      <span className="find-count" data-testid="find-count">
        {q && count ? (count.total ? `${count.index + 1} of ${count.total}` : "No matches") : ""}
      </span>
      <button className={`icon-btn find-case ${caseSensitive ? "pressed" : ""}`} title="Match case" onClick={() => setCase(!caseSensitive)} data-testid="find-case">
        Aa
      </button>
      <button className="icon-btn" title="Previous (Shift+Enter)" onClick={() => find(true)} data-testid="find-prev">
        <I.Chevron width={13} height={13} style={{ transform: "rotate(-90deg)" }} />
      </button>
      <button className="icon-btn" title="Next (Enter)" onClick={() => find(false)} data-testid="find-next">
        <I.Chevron width={13} height={13} style={{ transform: "rotate(90deg)" }} />
      </button>
      <button className="icon-btn" title="Close (Esc)" onClick={close}>
        <I.Close width={13} height={13} />
      </button>
    </div>
  );
}
