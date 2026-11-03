// Appearance: theme, accent colour, terminal text size. Changes apply
// immediately (live preview) and are remembered on this computer.

import { useEffect } from "react";
import { ACCENTS, DEFAULT_PREFS, FONT_SIZES, THEMES, themeInfo } from "../prefs";
import { setPrefs, useStore } from "../store";
import * as I from "./icons";

export function AppearanceDialog() {
  const prefs = useStore((s) => s.prefs);
  const close = () => useStore.setState({ appearanceOpen: false });
  const light = themeInfo(prefs.theme).light;
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && close();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, []);
  const size = (d: number) => setPrefs({ fontSize: Math.min(FONT_SIZES.max, Math.max(FONT_SIZES.min, prefs.fontSize + d)) });

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="modal wide appearance" role="dialog" data-testid="appearance-dialog">
        <div className="vault-head">
          <h2>Appearance</h2>
          <button className="icon-btn" title="Close" onClick={close}><I.Close /></button>
        </div>

        <div className="ap-label">Theme</div>
        <div className="ap-themes">
          {THEMES.map((t) => (
            <button
              key={t.id}
              className={`ap-theme ${prefs.theme === t.id ? "on" : ""}`}
              onClick={() => setPrefs({ theme: t.id })}
              aria-pressed={prefs.theme === t.id}
              data-testid={`theme-${t.id}`}
            >
              <span className="ap-preview" style={{ background: t.preview[1] }}>
                <span className="ap-preview-side" style={{ background: t.preview[0] }} />
                <span className="ap-preview-tab" style={{ background: t.preview[2] }} />
                <span className="ap-preview-line" style={{ background: t.light ? "#2a2f3a" : "#cdd3de" }} />
                <span className="ap-preview-line short" style={{ background: t.accent }} />
              </span>
              <span className="ap-theme-name">{t.name}</span>
              <span className="ap-theme-blurb">{t.blurb}</span>
            </button>
          ))}
        </div>

        <div className="ap-label">Accent colour</div>
        <div className="ap-swatches">
          <button
            className={`ap-swatch auto ${prefs.accent === null ? "on" : ""}`}
            title={`Theme default`}
            onClick={() => setPrefs({ accent: null })}
            style={{ "--sw": themeInfo(prefs.theme).accent } as React.CSSProperties}
            data-testid="accent-default"
          >
            <span>Auto</span>
          </button>
          {ACCENTS.map((a) => (
            <button
              key={a.id}
              className={`ap-swatch ${prefs.accent === a.id ? "on" : ""}`}
              title={a.name}
              aria-label={a.name}
              onClick={() => setPrefs({ accent: a.id })}
              style={{ "--sw": light ? a.light : a.dark } as React.CSSProperties}
              data-testid={`accent-${a.id}`}
            />
          ))}
        </div>

        <div className="ap-label">Terminal text size</div>
        <div className="ap-size">
          <button className="icon-btn" onClick={() => size(-1)} disabled={prefs.fontSize <= FONT_SIZES.min} title="Smaller" data-testid="font-smaller">A−</button>
          <input
            type="range"
            min={FONT_SIZES.min}
            max={FONT_SIZES.max}
            value={prefs.fontSize}
            onChange={(e) => setPrefs({ fontSize: Number(e.target.value) })}
            aria-label="Terminal text size"
          />
          <button className="icon-btn" onClick={() => size(1)} disabled={prefs.fontSize >= FONT_SIZES.max} title="Larger" data-testid="font-larger">A+</button>
          <span className="ap-size-value" data-testid="font-size">{prefs.fontSize}px</span>
        </div>

        <p className="muted small ap-note">
          Saved on this computer. Host colours are set per host: right-click a host, then <b>Edit host</b>.
        </p>
        <div className="modal-actions">
          <button onClick={() => setPrefs({ ...DEFAULT_PREFS })} data-testid="appearance-reset">Reset to defaults</button>
          <button className="primary" onClick={close}>Done</button>
        </div>
      </div>
    </div>
  );
}
