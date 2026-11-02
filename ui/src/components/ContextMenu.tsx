import { useEffect, useRef } from "react";
import { createPortal } from "react-dom";

export interface MenuItem {
  label: string;
  action: () => void;
  danger?: boolean;
}

export function ContextMenu({ x, y, items, onClose }: { x: number; y: number; items: MenuItem[]; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const close = (e: Event) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", close, true);
    window.addEventListener("keydown", esc, true);
    window.addEventListener("blur", onClose);
    return () => {
      window.removeEventListener("mousedown", close, true);
      window.removeEventListener("keydown", esc, true);
      window.removeEventListener("blur", onClose);
    };
  }, [onClose]);
  // Keep inside the viewport.
  const left = Math.min(x, window.innerWidth - 220);
  const top = Math.min(y, window.innerHeight - items.length * 30 - 12);
  return createPortal(
    <div ref={ref} className="ctx-menu" style={{ left, top }} onClick={(e) => e.stopPropagation()}>
      {items.map((it) => (
        <button
          key={it.label}
          className={`ctx-item ${it.danger ? "danger" : ""}`}
          onClick={() => {
            onClose();
            it.action();
          }}
        >
          {it.label}
        </button>
      ))}
    </div>,
    document.body,
  );
}
