import { getCurrentWindow } from "@tauri-apps/api/window";
import { create } from "zustand";

export type ThemePreference = "system" | "light" | "dark";
export type ResolvedTheme = "light" | "dark";

const STORAGE_KEY = "idedb.theme";
const darkQuery = window.matchMedia("(prefers-color-scheme: dark)");

function readPreference(): ThemePreference {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === "light" || stored === "dark" || stored === "system") return stored;
  } catch {
    // Storage unavailable: fall back to following the system.
  }
  return "system";
}

function resolve(preference: ThemePreference): ResolvedTheme {
  if (preference !== "system") return preference;
  return darkQuery.matches ? "dark" : "light";
}

interface ThemeState {
  preference: ThemePreference;
  resolved: ResolvedTheme;
  setPreference: (preference: ThemePreference) => void;
}

export const useTheme = create<ThemeState>((set) => {
  const preference = readPreference();
  return {
    preference,
    resolved: resolve(preference),
    setPreference: (preference) => {
      try {
        localStorage.setItem(STORAGE_KEY, preference);
      } catch {
        // Not persisted; still applies for this session.
      }
      set({ preference, resolved: resolve(preference) });
    },
  };
});

function apply({ preference, resolved }: ThemeState) {
  document.documentElement.dataset.theme = resolved;
  // Keeps the native title bar and traffic lights in step. `null` follows the system.
  void getCurrentWindow()
    .setTheme(preference === "system" ? null : preference)
    .catch(() => {});
}

apply(useTheme.getState());
useTheme.subscribe(apply);
darkQuery.addEventListener("change", () => {
  const { preference } = useTheme.getState();
  if (preference === "system") useTheme.setState({ resolved: resolve(preference) });
});

/** Reads a design token's current value, for consumers that cannot use CSS (the canvas grid). */
export function token(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(`--${name}`).trim();
}
