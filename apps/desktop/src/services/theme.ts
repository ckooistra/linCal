export type ThemePreference = "system" | "light" | "dark";
export type Theme = "light" | "dark";

export function resolveTheme(preference: ThemePreference, systemDark: boolean): Theme {
  if (preference === "system") return systemDark ? "dark" : "light";
  return preference;
}

/** Dark unless the user chose otherwise. */
export const DEFAULT_THEME_PREFERENCE: ThemePreference = "dark";

export function parseThemePreference(value: unknown): ThemePreference {
  return value === "light" || value === "dark" || value === "system" ? value : DEFAULT_THEME_PREFERENCE;
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
