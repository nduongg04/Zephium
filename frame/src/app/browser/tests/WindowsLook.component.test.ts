import { afterEach, expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import { favicons } from "$domain/favicons";
import { surface } from "$domain/surface";
import { tabs } from "$domain/tabs";
import { preferences } from "$domain/preferences";
import Shell from "../Shell.svelte";
import { paint, settle, shot, wallpaper } from "./look-bench";

const native = vi.hoisted(() => ({ settings: {} as Record<string, string> }));
vi.mock("$shared/platform", () => ({ IS_MAC: false, IS_WINDOWS: true }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    isMaximized: async () => false,
    onResized: async () => () => {},
    onFocusChanged: async () => () => {},
    minimize: async () => {},
    toggleMaximize: async () => {},
    close: async () => {},
  }),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    pagePermissionCardHost: async () => {},
    tabsBootstrap: async () => {},
    sidebarSetWidth: async () => {},
    sidebarResize: async () => false,
    sidebarResizeGuide: async () => true,
    settingGet: async (key: string) => native.settings[key] ?? null,
    settingSet: async () => ({ accepted: true, operation_id: null }),
  });
});

afterEach(() => {
  favicons.dispose();
  surface.dispose();
  tabs.dispose();
  preferences.dispose();
  document.getElementById("wallpaper")?.remove();
});

// Windows acrylic without our layer is the system's own milky frost; the
// window's colour is the body wash in Shell.svelte.
const ACRYLIC = { light: "rgb(250 250 250 / 45%)", dark: "rgb(44 44 44 / 45%)" };

for (const theme of ["dark", "light"] as const) {
  test(`windows over acrylic, ${theme}`, async () => {
    await page.viewport(1100, 760);
    native.settings = { appearance: theme };
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.material = "acrylic";
    await paint();
    const screen = await render(Shell);
    wallpaper("violet", theme, theme === "light" ? "#ffffff" : "#0d1117", {
      glass: ACRYLIC[theme],
      inset: "40px 8px 8px 256px",
    });
    await settle();
    await shot(`windows-${theme}`);
    // Windows draws its own caption buttons over the window's corner; the
    // chrome draws none of its own there.
    expect(screen.container.querySelector('[aria-label="Close"]')).toBeNull();
  });
}
