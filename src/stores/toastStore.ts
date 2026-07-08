import { create } from "zustand";

export type ToastKind = "error" | "warning" | "info" | "success";

export interface ToastAction {
  label: string;
  onClick: () => void;
}

export interface Toast {
  id: number;
  kind: ToastKind;
  title: string;
  message?: string;
  action?: ToastAction;
  /** Auto-dismiss after this many ms; 0 = sticky (user must dismiss). */
  timeout: number;
}

export interface ToastInput {
  kind: ToastKind;
  title: string;
  message?: string;
  action?: ToastAction;
  /** Override the auto-dismiss delay (defaults: errors sticky, others 6s). */
  timeout?: number;
  /** Dedupe key: a live toast with the same key is replaced, not stacked
   *  (e.g. repeated "extraction missing" reads while panning the board). */
  key?: string;
}

interface ToastState {
  toasts: Toast[];
  /** Show a toast; returns its id. */
  push: (t: ToastInput) => number;
  dismiss: (id: number) => void;
}

let nextId = 1;
const keyed = new Map<string, number>();

export const useToastStore = create<ToastState>((set, get) => ({
  toasts: [],
  push: ({ timeout, key, ...rest }) => {
    if (key && keyed.has(key)) {
      const prev = keyed.get(key)!;
      set((s) => ({ toasts: s.toasts.filter((t) => t.id !== prev) }));
    }
    const id = nextId++;
    if (key) keyed.set(key, id);
    const t: Toast = {
      id,
      ...rest,
      timeout: timeout ?? (rest.kind === "error" ? 0 : 6000),
    };
    set((s) => ({ toasts: [...s.toasts, t] }));
    if (t.timeout > 0) setTimeout(() => get().dismiss(id), t.timeout);
    return id;
  },
  dismiss: (id) => {
    set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
    for (const [k, v] of keyed) if (v === id) keyed.delete(k);
  },
}));
