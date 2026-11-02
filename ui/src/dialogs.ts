// In-app replacements for window.confirm/prompt (not available in every
// platform webview).

import { create } from "zustand";

export type DialogSpec =
  | { kind: "confirm"; title: string; message: string; danger?: boolean; ok?: string; resolve: (v: boolean) => void }
  | { kind: "text"; title: string; message?: string; initial: string; ok?: string; resolve: (v: string | null) => void };

export const useDialogs = create<{ stack: DialogSpec[] }>(() => ({ stack: [] }));

function push(d: DialogSpec) {
  useDialogs.setState((s) => ({ stack: [...s.stack, d] }));
}

export function pop(d: DialogSpec) {
  useDialogs.setState((s) => ({ stack: s.stack.filter((x) => x !== d) }));
}

export function askConfirm(title: string, message: string, opts: { danger?: boolean; ok?: string } = {}): Promise<boolean> {
  return new Promise((resolve) => push({ kind: "confirm", title, message, ...opts, resolve }));
}

export function askText(title: string, initial = "", opts: { message?: string; ok?: string } = {}): Promise<string | null> {
  return new Promise((resolve) => push({ kind: "text", title, initial, ...opts, resolve }));
}
