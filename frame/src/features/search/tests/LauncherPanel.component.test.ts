import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import { CheckListIcon, Note01Icon } from "@hugeicons/core-free-icons";
import "$styles/panel.css";
import { IS_MAC, IS_WINDOWS } from "$shared/platform";
const primary = IS_MAC ? "Meta" : "Control";
import { emitNativeEvent } from "$shared/testing/native-events";
import type { PanelLayout, PanelState, SearchResult } from "$shared/ipc/bindings";
import LauncherPanel from "../components/LauncherPanel.svelte";

const native = vi.hoisted(() => ({
  context: {
    window_id: "window",
    profile_id: "profile",
    space_id: "space",
    session_id: "0000000000000001",
    request_id: "",
  },
  search: vi.fn(async (_query: string, _request: string) => true),
  run: vi.fn(async (..._args: unknown[]) => ({ accepted: true, operation_id: null })),
  layout: vi.fn(async (_layout: PanelLayout) => {}),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    launcherSearch: native.search,
    launcherRun: native.run,
    panelLayout: native.layout,
  });
});
afterEach(() => {
  vi.clearAllMocks();
  delete document.documentElement.dataset.theme;
  delete document.documentElement.dataset.material;
  document.querySelector("#glass")?.remove();
});

/** Stands in for the native shapes, which a browser test has no way to draw,
 *  at exactly the rectangles the launcher reported. */
function glass(layout: PanelLayout) {
  document.querySelector("#glass")?.remove();
  const layer = document.createElement("div");
  layer.id = "glass";
  layer.style.cssText = "position:fixed;inset:0;z-index:-1;pointer-events:none";
  for (const [rect, radius] of [
    [layout.field, (layout.field.height ?? 0) / 2],
    [layout.sheet, 24],
  ] as const) {
    if (!rect) continue;
    const shape = document.createElement("div");
    shape.style.cssText = `position:absolute;left:${rect.x}px;top:${rect.y}px;width:${rect.width}px;height:${rect.height}px;border-radius:${radius}px;backdrop-filter:blur(28px) saturate(1.6);background:color-mix(in srgb, var(--color-chrome) 42%, transparent);box-shadow:inset 0 0.5px 0 rgb(255 255 255 / 0.28),inset 0 0 0 0.5px rgb(255 255 255 / 0.14),0 18px 48px rgb(0 0 0 / 0.28)`;
    layer.append(shape);
  }
  document.body.append(layer);
}

const context: PanelState = {
  ...native.context,
  revision: "0000000000000001",
  visible: true,
  profile_name: "Personal",
  error: false,
  corner_radius: 20,
  seed: null,
  seed_tab: null,
};
const destinations = [
  { kind: "tasks" as const, label: "Tasks", icon: CheckListIcon },
  { kind: "notes" as const, label: "Notes", icon: Note01Icon },
];
const search = (title: string): SearchResult => ({
  kind: "search",
  title,
  detail: "DuckDuckGo",
  icon: null,
  action: { type: "OpenUrl", url: `https://duckduckgo.com/?q=${title}` },
});
const visit = (title: string, url: string): SearchResult => ({
  kind: "history",
  title,
  detail: url,
  icon: null,
  action: { type: "OpenUrl", url },
});

async function reply(query: string, results: SearchResult[], completion: string | null = null) {
  await vi.waitFor(() => expect(native.search.mock.calls.at(-1)?.[0]).toBe(query));
  const owner = { ...native.context, request_id: native.search.mock.calls.at(-1)![1] };
  emitNativeEvent("searchChanged", {
    context: owner,
    query,
    results,
    completion,
    pending: false,
  });
  return owner;
}

test("Enter before any answer arrives runs once, against that exact request", async () => {
  const screen = await render(LauncherPanel, { props: { context, destinations } });
  const input = screen.getByRole("combobox");
  await input.fill("rust");
  await userEvent.keyboard("{Enter}");
  const owner = await reply("rust", [search("rust")]);
  await vi.waitFor(() =>
    expect(native.run).toHaveBeenCalledExactlyOnceWith(search("rust").action, owner, false),
  );
  // A later answer for the same query must not replay the action.
  emitNativeEvent("searchChanged", {
    context: owner,
    query: "rust",
    completion: null,
    pending: false,
    results: [search("rust"), search("rust book")],
  });
  expect(native.run).toHaveBeenCalledTimes(1);
});

test("an obsolete answer cannot put its rows back on screen", async () => {
  const screen = await render(LauncherPanel, { props: { context, destinations } });
  await screen.getByRole("combobox").fill("rust");
  const owner = await reply("rust", [search("rust"), search("rust book")]);
  await screen.getByRole("combobox").fill("@notes rust");
  await reply("@notes rust", []);
  emitNativeEvent("searchChanged", {
    context: owner,
    query: "rust",
    completion: null,
    pending: false,
    results: [search("stale")],
  });
  await expect.element(screen.getByRole("option", { name: /stale/u })).not.toBeInTheDocument();
});

test("home offers recent tabs and destinations, growing only as results need space", async () => {
  const onTool = vi.fn();
  const screen = await render(LauncherPanel, { props: { context, destinations, onTool } });
  await reply("", [
    {
      kind: "tab",
      title: "Rust Book",
      detail: "doc.rust-lang.org",
      icon: null,
      action: { type: "ActivateTab", id: "tab-1" },
    },
  ]);
  await expect.element(screen.getByRole("option", { name: /Rust Book/u })).toBeVisible();
  await screen.getByRole("button", { name: "Tasks" }).click();
  expect(onTool).toHaveBeenCalledExactlyOnceWith("tasks");
  await vi.waitFor(() => expect(native.layout).toHaveBeenCalled());
  const home = native.layout.mock.calls.at(-1)![0];

  await screen.getByRole("combobox").fill("rust");
  await reply("rust", [
    search("rust"),
    visit("Rust Book", "https://doc.rust-lang.org/book/"),
    {
      kind: "note",
      title: "Rust notes",
      detail: "Ownership, borrowing and lifetimes",
      icon: null,
      action: { type: "OpenNote", id: "note" },
    },
  ]);
  // Each row names its kind; the list needs no section headings.
  await expect.element(screen.getByText("History", { exact: true })).toBeVisible();
  await expect.element(screen.getByText("Ownership, borrowing and lifetimes")).toBeVisible();
  const row = screen.container.querySelector<HTMLElement>(".row")!;
  expect(row.getBoundingClientRect().height).toBe(44);
  await vi.waitFor(() => {
    const height = native.layout.mock.calls.at(-1)![0].height;
    expect(height).toBeGreaterThan(home.height ?? 0);
    if (IS_WINDOWS) expect(screen.container.querySelector(".launcher.entering")).toBeNull();
  });
  // The action capsule names what Enter will do to the selected row.
  expect(screen.container.querySelector(".primary")?.textContent).toContain("Search");
});

test("a bookmark names itself and keeps the place native gave it ahead of a visit", async () => {
  const screen = await render(LauncherPanel, { props: { context, destinations } });
  await screen.getByRole("combobox").fill("rust");
  await reply("rust", [
    search("rust"),
    { ...visit("Rust Book", "https://doc.rust-lang.org/book/"), kind: "bookmark" },
    visit("Rust Blog", "https://blog.rust-lang.org/"),
  ]);
  await expect.element(screen.getByText("Bookmark", { exact: true })).toBeVisible();
  const titles = [...screen.container.querySelectorAll(".row .title")].map((el) => el.textContent);
  expect(titles).toEqual(["rust", "Rust Book", "Rust Blog"]);
  const star = screen.container.querySelectorAll(".row")[1]!.querySelector(".glyph svg");
  const globe = screen.container.querySelectorAll(".row")[2]!.querySelector(".glyph svg");
  expect(star).not.toBeNull();
  expect(star!.innerHTML).not.toBe(globe!.innerHTML);
});

test("renders as two glass objects where native draws the material", async () => {
  document.documentElement.dataset.material = "liquid_glass";
  await page.viewport(720, 460);
  document.body.style.cssText =
    "margin:0;background:linear-gradient(135deg,#1c2340 0%,#4a4f7a 38%,#23263d 60%,#7a6a8e 100%)";
  const screen = await render(LauncherPanel, { props: { context, destinations } });
  await reply("", [
    {
      kind: "tab",
      title: "Download VPN | Proton VPN",
      detail: "protonvpn.com",
      icon: null,
      action: { type: "ActivateTab", id: "tab-1" },
    },
    {
      kind: "tab",
      title: "Telegram Desktop",
      detail: "desktop.telegram.org",
      icon: null,
      action: { type: "ActivateTab", id: "tab-2" },
    },
    {
      kind: "tab",
      title: "Terax - Terminal-first AI-native dev workspace",
      detail: "terax.app",
      icon: null,
      action: { type: "ActivateTab", id: "tab-3" },
    },
  ]);
  for (const theme of ["dark", "light"]) {
    document.documentElement.dataset.theme = theme;
    await new Promise((done) => setTimeout(done, 320));
    glass(native.layout.mock.calls.at(-1)![0]);
    await page.screenshot({ path: `../../../../../target/search-launcher-home-${theme}.png` });
  }
  document.documentElement.dataset.theme = "dark";
  await screen.getByRole("combobox").fill("rust");
  await reply("rust", [
    search("rust"),
    visit("Rust Book", "https://doc.rust-lang.org/book/"),
    {
      kind: "note",
      title: "Rust notes",
      detail: "Ownership, borrowing and lifetimes",
      icon: null,
      action: { type: "OpenNote", id: "note" },
    },
    {
      kind: "command",
      title: "Reopen Closed Tab",
      detail: "CmdOrCtrl+Shift+T",
      icon: null,
      action: { type: "RunCommand", id: "tab.reopen" },
    },
  ]);
  await new Promise((done) => setTimeout(done, 320));
  glass(native.layout.mock.calls.at(-1)![0]);
  await page.screenshot({ path: "../../../../../target/search-launcher-dark.png" });
  document.body.style.cssText = "";
});

test("a section that recurs in the list does not break it", async () => {
  const onCapture = vi.fn(async () => null);
  const screen = await render(LauncherPanel, {
    props: {
      context,
      onCapture,
      destinations: [{ kind: "history" as const, label: "History", icon: Note01Icon }],
    },
  });
  await screen.getByRole("combobox").fill("h");
  await reply("h", [
    search("h"),
    {
      kind: "command",
      title: "Theme: Dark",
      detail: "",
      icon: null,
      action: { type: "RunCommand", id: "theme.dark" },
    },
  ]);
  // Commands, then a destination, then the capture row, which is a command.
  await expect.element(screen.getByRole("option", { name: /Theme: Dark/u })).toBeVisible();
  await expect.element(screen.getByRole("option", { name: /^History/u })).toBeVisible();
  await expect.element(screen.getByRole("option", { name: /Add “h” to Tasks/u })).toBeVisible();
});

test("arithmetic is answered in place, and Enter copies the answer", async () => {
  const written: string[] = [];
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: async (text: string) => void written.push(text) },
  });
  document.documentElement.dataset.material = "liquid_glass";
  document.documentElement.dataset.theme = "dark";
  await page.viewport(760, 420);
  document.body.style.cssText =
    "margin:0;background:linear-gradient(135deg,#1c2340 0%,#4a4f7a 38%,#23263d 60%,#7a6a8e 100%)";
  const screen = await render(LauncherPanel, { props: { context, destinations } });
  await screen.getByRole("combobox").fill("1200 * 1.2 + 15%");
  // The answer does not wait for native.
  const answer = screen.getByRole("option", { name: /Calculator/u });
  await expect.element(answer).toHaveTextContent("1,656");
  await reply("1200 * 1.2 + 15%", [search("1200 * 1.2 + 15%")]);
  await new Promise((done) => setTimeout(done, 320));
  glass(native.layout.mock.calls.at(-1)![0]);
  await page.screenshot({ path: "../../../../../target/search-launcher-calculator.png" });
  await userEvent.keyboard("{Enter}");
  await vi.waitFor(() => expect(written).toEqual(["1656"]));
  expect(native.run).not.toHaveBeenCalled();
  await expect.element(screen.getByText("Answer copied")).toBeVisible();
  document.body.style.cssText = "";
});

test("renders as one card where the platform draws a single material", async () => {
  // Windows: Acrylic behind the whole window, corners and rim from DWM.
  document.documentElement.dataset.material = "acrylic";
  document.documentElement.dataset.theme = "dark";
  await page.viewport(680, 360);
  document.body.style.cssText =
    "margin:0;background:linear-gradient(135deg,#1f2a44 0%,#3b4a6b 45%,#202637 100%)";
  const screen = await render(LauncherPanel, { props: { context, destinations } });
  screen.container.style.cssText = "--panel-radius:8px";
  await screen.getByRole("combobox").fill("rust");
  await reply("rust", [
    search("rust"),
    visit("Rust Book", "https://doc.rust-lang.org/book/"),
    {
      kind: "note",
      title: "Rust notes",
      detail: "Ownership, borrowing and lifetimes",
      icon: null,
      action: { type: "OpenNote", id: "note" },
    },
  ]);
  await expect.element(screen.getByRole("option", { name: /Rust Book/u })).toBeVisible();
  const card = screen.container.querySelector<HTMLElement>(".launcher")!;
  // Windows owns the rounded clip in DWM; CSS fills the renderer behind it.
  expect(getComputedStyle(card).borderTopLeftRadius).toBe(IS_WINDOWS ? "0px" : "8px");
  await new Promise((done) => setTimeout(done, 300));
  await page.screenshot({ path: "../../../../../target/search-launcher-windows.png" });
  document.body.style.cssText = "";
});

test("a destination is handed to the browser", async () => {
  const onTool = vi.fn();
  const screen = await render(LauncherPanel, { props: { context, destinations, onTool } });
  await screen.getByRole("combobox").fill("not");
  await screen.getByRole("option", { name: "Notes" }).click();
  expect(onTool).toHaveBeenCalledExactlyOnceWith("notes");
});

test("The primary modifier plus Enter opens behind the current tab and keeps the launcher", async () => {
  const onDismiss = vi.fn();
  const screen = await render(LauncherPanel, { props: { context, destinations, onDismiss } });
  await screen.getByRole("combobox").fill("rust");
  const owner = await reply("rust", [visit("Rust Book", "https://doc.rust-lang.org/book/")]);
  await userEvent.keyboard(`{ArrowDown}{${primary}>}{Enter}{/${primary}}`);
  await vi.waitFor(() =>
    expect(native.run).toHaveBeenCalledExactlyOnceWith(
      { type: "OpenUrl", url: "https://doc.rust-lang.org/book/" },
      owner,
      true,
    ),
  );
  await expect.element(screen.getByText("Opened in a background tab")).toBeVisible();
  expect(onDismiss).not.toHaveBeenCalled();
});

test("The primary modifier plus K lists what can be done to the selected row", async () => {
  const screen = await render(LauncherPanel, { props: { context, destinations } });
  await screen.getByRole("combobox").fill("rust");
  await reply("rust", [search("rust"), visit("Rust Book", "https://doc.rust-lang.org/book/")]);
  await userEvent.keyboard(`{ArrowDown}{${primary}>}k{/${primary}}`);
  const menu = screen.getByRole("menu");
  await expect.element(menu.getByRole("menuitem", { name: /Open in Background/u })).toBeVisible();
  await expect.element(menu.getByRole("menuitem", { name: /Copy Link/u })).toBeVisible();
  await page.screenshot({ path: "../../../../../target/search-launcher-actions.png" });
  await userEvent.keyboard("{Escape}");
  await expect.element(screen.getByRole("menu")).not.toBeInTheDocument();
  // The field is untouched by closing the sheet.
  await expect.element(screen.getByRole("combobox")).toHaveValue("rust");
});

test("Escape clears what was typed, then puts the launcher away", async () => {
  const onDismiss = vi.fn();
  const screen = await render(LauncherPanel, { props: { context, destinations, onDismiss } });
  await screen.getByRole("combobox").fill("rust");
  await userEvent.keyboard("{Escape}");
  await expect.element(screen.getByRole("combobox")).toHaveValue("");
  expect(onDismiss).not.toHaveBeenCalled();
  await userEvent.keyboard("{Escape}");
  expect(onDismiss).toHaveBeenCalledOnce();
});

test("a launcher reopened a moment later resumes, and a fresh session asks again", async () => {
  const screen = await render(LauncherPanel, { props: { context, destinations } });
  await screen.getByRole("combobox").fill("rust");
  await reply("rust", [search("rust")]);
  await screen.rerender({ context: { ...context, revision: "0000000000000002", visible: false } });
  const calls = native.search.mock.calls.length;
  await screen.rerender({
    context: { ...context, revision: "0000000000000003", session_id: "0000000000000003" },
  });
  await expect.element(screen.getByRole("combobox")).toHaveValue("rust");
  await vi.waitFor(() => expect(native.search.mock.calls.length).toBeGreaterThan(calls));
  expect(native.search.mock.calls.at(-1)![0]).toBe("rust");
});

test("sleeping past the resume deadline clears a query even when its timer never fired", async () => {
  const screen = await render(LauncherPanel, { props: { context, destinations } });
  await screen.getByRole("combobox").fill("private old query");
  await reply("private old query", [search("private old query")]);
  await screen.rerender({ context: { ...context, revision: "0000000000000002", visible: false } });
  // WebView suspension freezes timers, but wall time still advances.
  const now = vi.spyOn(Date, "now").mockReturnValue(Date.now() + 31_000);
  try {
    await screen.rerender({
      context: { ...context, revision: "0000000000000003", session_id: "0000000000000003" },
    });
    await expect.element(screen.getByRole("combobox")).toHaveValue("");
    await vi.waitFor(() => expect(native.search.mock.calls.at(-1)?.[0]).toBe(""));
  } finally {
    now.mockRestore();
  }
});

test("anything typed can be kept as a task, by its row or by Option+Enter", async () => {
  const onCapture = vi.fn(async (text: string) => text.replace(/ tomorrow$/u, ""));
  const screen = await render(LauncherPanel, { props: { context, destinations, onCapture } });
  const input = screen.getByRole("combobox");
  await input.fill("Call the dentist tomorrow");
  await reply("Call the dentist tomorrow", [search("Call the dentist tomorrow")]);
  const row = screen.getByRole("option", { name: /Add “Call the dentist tomorrow” to Tasks/u });
  await expect.element(row).toBeVisible();
  await page.screenshot({ path: "../../../../../target/tasks-qa/launcher-capture.png" });

  await userEvent.keyboard("{Alt>}{Enter}{/Alt}");
  await vi.waitFor(() => expect(onCapture).toHaveBeenCalledWith("Call the dentist tomorrow"));
  // The page search the line most often means stayed where it was, untouched.
  expect(native.run).not.toHaveBeenCalled();
  await expect.element(screen.getByText("Added “Call the dentist” to Tasks")).toBeVisible();
});

test("typing through a selected completion keeps offering it, while Backspace rejects it", async () => {
  const screen = await render(LauncherPanel, { props: { context, destinations } });
  const input = screen.getByRole("combobox");
  const element = input.element() as HTMLInputElement;
  const result = visit("GitHub", "https://github.com/");
  await input.fill("gi");
  await reply("gi", [search("gi"), result], "github.com");
  await vi.waitFor(() => expect(element.value).toBe("github.com"));
  expect([element.selectionStart, element.selectionEnd]).toEqual([2, 10]);
  await userEvent.keyboard("t");
  await reply("git", [search("git"), result], "github.com");
  await vi.waitFor(() => expect(element.value).toBe("github.com"));
  expect([element.selectionStart, element.selectionEnd]).toEqual([3, 10]);
  await userEvent.keyboard("h");
  await reply("gith", [search("gith"), result], "github.com");
  await vi.waitFor(() => expect(element.value).toBe("github.com"));
  expect([element.selectionStart, element.selectionEnd]).toEqual([4, 10]);
  const previous = native.search.mock.calls.at(-1)![1];
  await userEvent.keyboard("{Backspace}");
  await vi.waitFor(() => expect(native.search.mock.calls.at(-1)![1]).not.toBe(previous));
  await reply("gith", [search("gith"), result], "github.com");
  await expect.element(input).toHaveValue("gith");
  expect(native.run).not.toHaveBeenCalled();
});

test("Windows paints one continuous wash while the native clip follows content height", async () => {
  if (!IS_WINDOWS) return;
  document.documentElement.dataset.material = "acrylic";
  await page.viewport(680, 600);
  const screen = await render(LauncherPanel, { props: { context, destinations } });
  await reply("", [visit("GitHub", "https://github.com/")]);
  const launcher = screen.container.querySelector<HTMLElement>(".launcher")!;
  await vi.waitFor(() => expect(native.layout.mock.calls.at(-1)![0].height).toBeGreaterThan(100));
  expect(launcher.getBoundingClientRect().height).toBe(600);
  expect(native.layout.mock.calls.at(-1)![0].height).toBeLessThan(300);
  await screen.getByRole("combobox").fill("rust");
  await reply(
    "rust",
    Array.from({ length: 20 }, (_, i) => visit("Result " + i, "https://example.com/" + i)),
  );
  await vi.waitFor(() => expect(native.layout.mock.calls.at(-1)![0].height).toBe(600));
  const scroll = screen.container.querySelector<HTMLElement>(".scroll")!;
  expect(scroll.scrollHeight).toBeGreaterThan(scroll.clientHeight);
  expect(getComputedStyle(scroll).scrollbarWidth).toBe("none");
  await screen.getByRole("combobox").fill("few");
  await reply("few", [search("few")]);
  await vi.waitFor(() => expect(native.layout.mock.calls.at(-1)![0].height).toBeLessThan(300));
  expect(launcher.getBoundingClientRect().height).toBe(600);
});
