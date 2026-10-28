import { useEffect, useState } from "react";
export type Theme = "dark" | "light" | "system";
function preference(key: string, fallback: string) {
  try {
    return localStorage.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}
export function usePreferences() {
  const [theme, setTheme] = useState<Theme>(() => {
    const value = preference("kiyora.theme", "dark");
    return value === "light" || value === "system" ? value : "dark";
  });
  const [reducedMotion, setReducedMotion] = useState(
    () => preference("kiyora.motion", "auto") === "reduce",
  );
  useEffect(() => {
    const media = matchMedia("(prefers-color-scheme: light)");
    const update = () => {
      document.documentElement.dataset.theme =
        theme === "system" ? (media.matches ? "light" : "dark") : theme;
    };
    update();
    media.addEventListener("change", update);
    try {
      localStorage.setItem("kiyora.theme", theme);
    } catch {
      /* Appearance can remain session-only. */
    }
    return () => media.removeEventListener("change", update);
  }, [theme]);
  useEffect(() => {
    document.documentElement.dataset.motion = reducedMotion ? "reduce" : "auto";
    try {
      localStorage.setItem("kiyora.motion", reducedMotion ? "reduce" : "auto");
    } catch {
      /* Non-sensitive preference. */
    }
  }, [reducedMotion]);
  return { theme, setTheme, reducedMotion, setReducedMotion };
}
