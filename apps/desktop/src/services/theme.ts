export type ThemePreference = "system" | "light" | "dark";
export type Theme = "light" | "dark";

const STORAGE_KEY = "uc.theme";

export function resolveTheme(preference: ThemePreference, systemDark: boolean): Theme {
  if (preference === "system") return systemDark ? "dark" : "light";
  return preference;
}

/** Per-device convenience setting; moves to the settings table in Milestone 2. */
export function loadThemePreference(): ThemePreference {
  try {
    const value = localStorage.getItem(STORAGE_KEY);
    if (value === "light" || value === "dark" || value === "system") return value;
  } catch {
    // Storage unavailable: fall back to the system theme.
  }
  return "system";
}

export function saveThemePreference(preference: ThemePreference): void {
  try {
    localStorage.setItem(STORAGE_KEY, preference);
  } catch {
    // Not critical.
  }
}

/** Calls `onChange` when the OS colour scheme changes. Returns a cleanup function. */
export function watchSystemDark(onChange: (dark: boolean) => void): () => void {
  if (typeof matchMedia !== "function") return () => {};
  const query = matchMedia("(prefers-color-scheme: dark)");
  onChange(query.matches);
  const listener = (e: MediaQueryListEvent) => onChange(e.matches);
  query.addEventListener("change", listener);
  return () => query.removeEventListener("change", listener);
}
