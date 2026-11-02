// Password vault UI. Secrets typed here go straight to the core; nothing
// secret ever comes back (the store only holds VaultStatus).

import { useEffect, useState } from "react";
import * as A from "../actions";
import { useStore, type Prompt } from "../store";
import * as I from "./icons";

const MIN = 8;

function errText(e: unknown) {
  const s = String(e instanceof Error ? e.message : e);
  return s.charAt(0).toUpperCase() + s.slice(1);
}

function RememberBox({ value, onChange }: { value: boolean; onChange: (v: boolean) => void }) {
  const supported = useStore((s) => s.vault?.rememberSupported ?? false);
  if (!supported) return null;
  return (
    <label className="check vault-remember">
      <input type="checkbox" checked={value} onChange={(e) => onChange(e.target.checked)} data-testid="vault-remember" />
      <span>
        Unlock automatically on this computer
        <span className="muted small block">
          Keeps the vault key in Windows Credential Manager. Convenient, but any program running as you could read it.
        </span>
      </span>
    </label>
  );
}

/** Choose a master password (first-time setup). */
function CreateForm({ onDone, onBack, ok = "Create vault" }: { onDone: () => void; onBack?: () => void; ok?: string }) {
  const [pw, setPw] = useState("");
  const [again, setAgain] = useState("");
  const [remember, setRemember] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const problem = pw.length < MIN ? `At least ${MIN} characters` : pw !== again ? "The two passwords don't match" : null;
  return (
    <form
      onSubmit={async (e) => {
        e.preventDefault();
        if (problem) return setError(problem);
        setBusy(true);
        try {
          await A.vaultCreate(pw, remember);
          onDone();
        } catch (err) {
          setError(errText(err));
        } finally {
          setBusy(false);
        }
      }}
    >
      <label className="field">
        <span>Master password</span>
        <input type="password" autoFocus value={pw} onChange={(e) => { setPw(e.target.value); setError(null); }} data-testid="vault-new" />
      </label>
      <label className="field">
        <span>Repeat it</span>
        <input type="password" value={again} onChange={(e) => { setAgain(e.target.value); setError(null); }} data-testid="vault-new-again" />
      </label>
      <p className="muted small vault-warn">
        <I.Key width={12} height={12} /> Muxscape can't recover a forgotten master password. Use one you'll remember, or keep it in a
        password manager.
      </p>
      <RememberBox value={remember} onChange={setRemember} />
      {error && <div className="form-error">{error}</div>}
      <div className="modal-actions">
        {onBack && <button type="button" onClick={onBack}>Back</button>}
        <button className="primary" type="submit" disabled={busy || !pw} data-testid="vault-create">{busy ? "Creating…" : ok}</button>
      </div>
    </form>
  );
}

/** Unlock with the master password. */
function UnlockForm({ onDone, onBack, ok = "Unlock" }: { onDone: () => void; onBack?: () => void; ok?: string }) {
  const [pw, setPw] = useState("");
  const [remember, setRemember] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <form
      onSubmit={async (e) => {
        e.preventDefault();
        setBusy(true);
        try {
          await A.vaultUnlock(pw, remember);
          onDone();
        } catch (err) {
          setError(errText(err));
          setPw("");
        } finally {
          setBusy(false);
        }
      }}
    >
      <input className="wide-input" type="password" autoFocus placeholder="Master password" value={pw} onChange={(e) => setPw(e.target.value)} data-testid="vault-master" />
      <RememberBox value={remember} onChange={setRemember} />
      {error && <div className="form-error">{error}</div>}
      <div className="modal-actions">
        {onBack && <button type="button" onClick={onBack}>Back</button>}
        <button className="primary" type="submit" disabled={busy || !pw} data-testid="vault-unlock">{busy ? "Unlocking…" : ok}</button>
      </div>
    </form>
  );
}

/**
 * Server password / key passphrase prompt, with "remember". Saving needs an
 * unlocked vault, so the same dialog walks through setup or unlock first.
 * The core stores the secret only after the login succeeds.
 */
export function SecretPrompt({ p }: { p: Prompt }) {
  const r = p.request as Extract<Prompt["request"], { kind: "password" | "passphrase" }>;
  const vault = useStore((s) => s.vault);
  const host = useStore((s) => s.hosts.find((h) => h.id === p.hostId));
  const [value, setValue] = useState("");
  const [save, setSave] = useState(!vault?.error);
  const [step, setStep] = useState<"enter" | "setup" | "unlock">("enter");
  const cancel = () => A.answerPrompt(p.prompt, { kind: "secret", value: null });
  const finish = () => A.answerPrompt(p.prompt, { kind: "secret", value, save });
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && cancel();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const what = r.kind === "password" ? "password" : "passphrase";
  return (
    <div className="modal-backdrop">
      <div className="modal" role="dialog" data-testid="secret-prompt">
        {step === "enter" && (
          <>
            <h2>{r.kind === "password" ? `Password for ${r.target}` : "Key passphrase"}</h2>
            {r.kind === "passphrase" && <p>Enter the passphrase for <code>{r.keyPath}</code>.</p>}
            {r.kind === "password" && host && <p>{host.label}</p>}
            {r.savedFailed && (
              <div className="form-warn" data-testid="saved-failed">
                The saved {what} didn't work. It may have changed. Enter the current one.
              </div>
            )}
            <form
              onSubmit={(e) => {
                e.preventDefault();
                if (!save || vault?.unlocked) return finish();
                setStep(vault?.exists ? "unlock" : "setup");
              }}
            >
              <input className="wide-input" autoFocus type="password" value={value} onChange={(e) => setValue(e.target.value)} data-testid="secret-input" />
              {!vault?.error && (
                <label className="check vault-save">
                  <input type="checkbox" checked={save} onChange={(e) => setSave(e.target.checked)} data-testid="secret-save" />
                  {r.savedFailed ? `Update the saved ${what}` : `Remember this ${what}`}
                  <span className="muted small">
                    {vault?.unlocked ? "(encrypted in your vault)" : vault?.exists ? "(asks for your master password)" : "(sets up an encrypted vault)"}
                  </span>
                </label>
              )}
              <div className="modal-actions">
                <button type="button" onClick={cancel}>Cancel</button>
                <button className="primary" type="submit" disabled={!value} data-testid="secret-ok">
                  {r.kind === "password" ? "Sign in" : "Unlock key"}
                </button>
              </div>
            </form>
          </>
        )}
        {step === "setup" && (
          <>
            <h2>Create your password vault</h2>
            <p>
              Saved passwords are encrypted on this computer with a master password only you know. Muxscape never stores the
              master password itself.
            </p>
            <CreateForm onDone={finish} onBack={() => setStep("enter")} ok="Create vault & sign in" />
          </>
        )}
        {step === "unlock" && (
          <>
            <h2>Unlock your vault</h2>
            <p>Enter your master password to save this {what}.</p>
            <UnlockForm onDone={finish} onBack={() => setStep("enter")} ok="Unlock & sign in" />
          </>
        )}
      </div>
    </div>
  );
}

/** The core needs a saved password but the vault is locked. */
export function VaultUnlockPrompt({ p }: { p: Prompt }) {
  const r = p.request as Extract<Prompt["request"], { kind: "vaultUnlock" }>;
  const [pw, setPw] = useState("");
  const [remember, setRemember] = useState(false);
  const instead = () => A.answerPrompt(p.prompt, { kind: "secret", value: null });
  return (
    <div className="modal-backdrop">
      <div className="modal" role="dialog" data-testid="vault-unlock-prompt">
        <h2><I.Lock width={15} height={15} /> Unlock your vault</h2>
        <p>Your saved password for <b>{r.target}</b> is in the vault.</p>
        {r.retry && <div className="form-error">Wrong master password. Try again.</div>}
        <form
          onSubmit={(e) => {
            e.preventDefault();
            A.answerPrompt(p.prompt, { kind: "secret", value: pw, save: remember });
          }}
        >
          <input className="wide-input" type="password" autoFocus placeholder="Master password" value={pw} onChange={(e) => setPw(e.target.value)} data-testid="vault-master" />
          <RememberBox value={remember} onChange={setRemember} />
          <div className="modal-actions">
            <button type="button" onClick={instead} data-testid="vault-instead">Type the server password instead</button>
            <button className="primary" type="submit" disabled={!pw} data-testid="vault-unlock">Unlock</button>
          </div>
        </form>
      </div>
    </div>
  );
}

function ChangePassword({ onDone }: { onDone: () => void }) {
  const [old, setOld] = useState("");
  const [pw, setPw] = useState("");
  const [again, setAgain] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <form
      className="vault-change"
      onSubmit={async (e) => {
        e.preventDefault();
        if (pw.length < MIN) return setError(`At least ${MIN} characters`);
        if (pw !== again) return setError("The two new passwords don't match");
        setBusy(true);
        try {
          await A.vaultChangePassword(old, pw);
          onDone();
        } catch (err) {
          setError(errText(err));
        } finally {
          setBusy(false);
        }
      }}
    >
      <label className="field"><span>Current master password</span><input type="password" autoFocus value={old} onChange={(e) => setOld(e.target.value)} /></label>
      <label className="field"><span>New master password</span><input type="password" value={pw} onChange={(e) => setPw(e.target.value)} /></label>
      <label className="field"><span>Repeat it</span><input type="password" value={again} onChange={(e) => setAgain(e.target.value)} /></label>
      {error && <div className="form-error">{error}</div>}
      <div className="modal-actions">
        <button type="button" onClick={onDone}>Cancel</button>
        <button className="primary" type="submit" disabled={busy || !old || !pw}>{busy ? "Saving…" : "Change password"}</button>
      </div>
    </form>
  );
}

/** Vault manager: set up, unlock, saved logins, settings. */
export function VaultDialog() {
  const v = useStore((s) => s.vault);
  const [changing, setChanging] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const close = () => useStore.setState({ vaultDialog: false });
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && close();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, []);
  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="modal wide vault" role="dialog" data-testid="vault-dialog">
        <div className="vault-head">
          <h2>{v?.unlocked ? <I.Unlock width={16} height={16} /> : <I.Lock width={16} height={16} />} Password vault</h2>
          <button className="icon-btn" title="Close" onClick={close}><I.Close /></button>
        </div>

        {v?.error ? (
          <>
            <div className="form-error">{v.error}</div>
            <p>The vault file couldn't be read, so it was left untouched. Resetting deletes it and starts over.</p>
            <div className="modal-actions"><button className="danger" onClick={() => A.vaultReset()}>Reset vault</button></div>
          </>
        ) : !v?.exists ? (
          <>
            <p>
              Save server passwords and key passphrases so you don't type them every time. They're encrypted on this computer with
              a master password only you know.
            </p>
            <CreateForm onDone={() => {}} />
          </>
        ) : !v.unlocked ? (
          <>
            <p>The vault is locked. Saved passwords are used again once you unlock it.</p>
            <UnlockForm onDone={() => {}} />
            <button className="link vault-forgot" onClick={() => A.vaultReset()}>Forgot your master password? Reset the vault</button>
          </>
        ) : changing ? (
          <ChangePassword onDone={() => setChanging(false)} />
        ) : (
          <>
            <div className="vault-section">Saved logins</div>
            {v.entries.length === 0 && (
              <p className="muted">Nothing saved yet. Tick <b>Remember this password</b> when you sign in to a server.</p>
            )}
            {v.entries.map((slot) => {
              const d = A.describeSlot(slot);
              return (
                <div key={slot} className="vault-row" data-testid={`vault-entry-${d.label}`}>
                  <I.Key width={14} height={14} className="row-icon" />
                  <div className="vault-row-main">
                    <div className="vault-row-label">{d.label}</div>
                    <div className="muted small">{d.kind}</div>
                  </div>
                  <button className="ghost" onClick={() => A.vaultForget(slot)} data-testid={`vault-forget-${d.label}`}>Forget</button>
                </div>
              );
            })}
            <div className="vault-section">On this computer</div>
            {v.rememberSupported ? (
              <label className="check vault-remember">
                <input
                  type="checkbox"
                  checked={v.remember}
                  onChange={async (e) => {
                    try {
                      setError(null);
                      await A.vaultSetRemember(e.target.checked);
                    } catch (err) {
                      setError(errText(err));
                    }
                  }}
                  data-testid="vault-remember-toggle"
                />
                <span>
                  Unlock automatically when Muxscape starts
                  <span className="muted small block">
                    The vault key is kept in Windows Credential Manager. Any program running as you could read it. Off is safer.
                  </span>
                </span>
              </label>
            ) : (
              <p className="muted small">Automatic unlock isn't available on this system yet.</p>
            )}
            {error && <div className="form-error">{error}</div>}
            <div className="modal-actions vault-actions">
              <button className="link danger-link" onClick={() => A.vaultReset()}>Reset vault…</button>
              <span className="spacer" />
              <button onClick={() => setChanging(true)}>Change master password</button>
              <button className="primary" onClick={() => A.vaultLock()} data-testid="vault-lock">Lock now</button>
            </div>
          </>
        )}
        <div className="vault-foot">
          Encrypted on this device with XChaCha20-Poly1305; the key comes from your master password via Argon2id. Locks after 8
          hours without use.
        </div>
      </div>
    </div>
  );
}
