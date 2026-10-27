import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { userEvent } from "vitest/browser";
import "$styles/global.css";
import "$shared/ui/Menu/surface.css";
import ExtensionsSection from "../components/ExtensionsSection.svelte";

type Action = {
  runtime: { install_id: string; generation: string };
  revision: string;
  label: string;
  badge: string;
  unread_badge: boolean;
  enabled: boolean;
  presents_popup: boolean;
  icon_rgba_base64: string | null;
};

const native = vi.hoisted(() => ({
  available: true,
  actions: [] as unknown[],
  shortcut: null as null | {
    runtime: { install_id: string; generation: string };
    action_revision: string;
    projection_revision: string;
  },
  invoke: vi.fn(async (..._args: unknown[]) => {}),
  consume: vi.fn((_revision: string) => true),
  open: vi.fn(async (_page: string) => {}),
}));
vi.mock("$domain/webext", () => ({ webext: { isAvailable: () => native.available } }));
vi.mock("$domain/surface", () => ({ surface: { open: native.open } }));
vi.mock("$domain/tabs", () => ({
  tabs: { profile: () => ({ id: "profile" }), activeId: () => "tab" },
}));
vi.mock("$domain/extensions", () => ({
  extensions: {
    activeActions: () => native.actions,
    failureReason: () => null,
    isInvoking: () => false,
    actionShortcut: () => native.shortcut,
    consumeActionShortcut: native.consume,
    invoke: native.invoke,
  },
}));

const action = (id: string, over: Partial<Action> = {}): Action => ({
  runtime: { install_id: id, generation: "g" },
  revision: "r1",
  label: `Extension ${id}`,
  badge: "",
  unread_badge: false,
  enabled: true,
  presents_popup: true,
  icon_rgba_base64: null,
  ...over,
});

beforeEach(() => {
  native.available = true;
  native.actions = [];
  native.shortcut = null;
});
afterEach(() => vi.clearAllMocks());

test("without extensions there is nothing to draw, not even a hairline's worth", async () => {
  native.available = false;
  const screen = await render(ExtensionsSection);
  expect(screen.container.querySelector(".extensions")).toBeNull();
  await expect
    .element(screen.getByRole("button", { name: "Manage extensions" }))
    .not.toBeInTheDocument();
});

test("each extension's action is a button named for it, beside the way to manage them", async () => {
  native.actions = [action("a"), action("b", { enabled: false })];
  const screen = await render(ExtensionsSection);
  await expect.element(screen.getByRole("button", { name: "Extension a" })).toBeEnabled();
  await expect.element(screen.getByRole("button", { name: "Extension b" })).toBeDisabled();
  await expect.element(screen.getByRole("button", { name: "Manage extensions" })).toBeVisible();
});

test("with no actions the way to manage them stays", async () => {
  const screen = await render(ExtensionsSection);
  await expect.element(screen.getByRole("button", { name: "Manage extensions" })).toBeVisible();
  await expect.element(screen.getByRole("button", { name: /^Extension / })).not.toBeInTheDocument();
});

test("pressing an action opens it from the page's tab, and leaves the panel", async () => {
  native.actions = [action("a")];
  const screen = await render(ExtensionsSection);
  const tile = screen.getByRole("button", { name: "Extension a" });
  await expect.element(tile).toHaveAttribute("data-dismiss");
  await tile.click();
  expect(native.invoke).toHaveBeenCalledExactlyOnceWith(
    "profile",
    "tab",
    native.actions[0],
    expect.any(DOMRect),
  );
});

test("an extension's key opens its action without the panel ever being opened", async () => {
  native.actions = [action("a")];
  native.shortcut = {
    runtime: { install_id: "a", generation: "g" },
    action_revision: "r1",
    projection_revision: "p1",
  };
  await render(ExtensionsSection);
  await expect.poll(() => native.invoke.mock.calls.length).toBe(1);
  expect(native.consume).toHaveBeenCalledWith("p1");
});

test("Manage extensions opens the extensions page and leaves the panel", async () => {
  const screen = await render(ExtensionsSection);
  const manage = screen.getByRole("button", { name: "Manage extensions" });
  await expect.element(manage).toHaveAttribute("data-dismiss");
  await manage.click();
  expect(native.open).toHaveBeenCalledExactlyOnceWith("extensions");
});

// The panel's width at the default 240px column, less its own padding.
const panelWidth = "228px";

const rect = (element: Element) => element.getBoundingClientRect();

test("one row at the default column: the name and the way on at the left, each extension's button at the right", async () => {
  native.actions = [action("a"), action("b"), action("c")];
  const screen = await render(ExtensionsSection);
  screen.container.style.inlineSize = panelWidth;
  const row = screen.container.querySelector<HTMLElement>(".row")!;
  const manage = screen.getByRole("button", { name: "Manage extensions" });
  await expect.element(manage).toBeVisible();
  await expect.element(manage).toHaveTextContent("Extensions");
  expect(row.offsetHeight).toBe(34);
  expect(row.querySelector(".ui-menu-icon")).toBeNull();

  const tiles = [...row.querySelectorAll<HTMLElement>("button[data-extension-install]")];
  expect(tiles).toHaveLength(3);
  const button = manage.element();
  expect(rect(button).right).toBeLessThanOrEqual(rect(tiles[0]!).left);
  expect(rect(button).left).toBe(rect(row).left);
  // The name's text starts where the icons of the rows around it do.
  expect(Math.round(rect(button.querySelector(".name")!).left - rect(row).left)).toBe(8);
  // The buttons end at the row's end, each on the same line and the same size.
  expect(rect(row).right - rect(tiles[2]!).right).toBeLessThanOrEqual(4.5);
  for (const tile of tiles) {
    expect(Math.abs(rect(tile).top - rect(tiles[0]!).top)).toBeLessThan(1);
    expect(rect(tile).width).toBe(28);
    expect(rect(tile).bottom).toBeLessThanOrEqual(rect(row).bottom);
  }
  expect(rect(button).height).toBe(28);
});

test("the way to manage them is the name and its chevron, not the row", async () => {
  native.actions = [action("a")];
  const screen = await render(ExtensionsSection);
  screen.container.style.inlineSize = panelWidth;
  const manage = screen.getByRole("button", { name: "Manage extensions" });
  await expect.element(manage).toBeVisible();
  const row = screen.container.querySelector<HTMLElement>(".row")!;
  const button = manage.element();
  const tile = row.querySelector<HTMLElement>("button[data-extension-install]")!;

  expect(rect(button).width).toBeLessThan(rect(row).width / 2);
  const gap = [
    (rect(button).right + rect(tile).left) / 2,
    rect(row).top + rect(row).height / 2,
  ] as const;
  expect(rect(tile).left - rect(button).right).toBeGreaterThan(40);
  const under = document.elementFromPoint(...gap);
  expect(button.contains(under)).toBe(false);
  expect(tile.contains(under)).toBe(false);
  await userEvent.click(under!);
  expect(native.open).not.toHaveBeenCalled();

  await expect.element(manage).toBeVisible();
  const centre = (element: Element) =>
    [
      rect(element).left + rect(element).width / 2,
      rect(element).top + rect(element).height / 2,
    ] as const;
  for (const part of [".name", ".chevron"]) {
    expect(
      document.elementFromPoint(...centre(button.querySelector(part)!))!.closest("button"),
    ).toBe(button);
  }
  expect(tile.contains(document.elementFromPoint(...centre(tile)))).toBe(true);
  await manage.click();
  expect(native.open).toHaveBeenCalledExactlyOnceWith("extensions");
});

test("each button has its own hover, and the others keep none", async () => {
  native.actions = [action("a"), action("b")];
  const screen = await render(ExtensionsSection);
  screen.container.style.inlineSize = panelWidth;
  const a = screen.getByRole("button", { name: "Extension a" });
  const b = screen.getByRole("button", { name: "Extension b" });
  const manage = screen.getByRole("button", { name: "Manage extensions" });
  // Whether a button has a plate of its own: the fill is not fully transparent.
  const lit = (locator: typeof a) => {
    const fill = getComputedStyle(locator.element()).backgroundColor;
    return Number(/,\s*([\d.]+)\)$/u.exec(fill)?.[1] ?? 1) > 0;
  };
  expect(lit(a) || lit(b) || lit(manage)).toBe(false);
  await a.hover();
  await expect.poll(() => lit(a)).toBe(true);
  expect(lit(b)).toBe(false);
  expect(lit(manage)).toBe(false);
  await manage.hover();
  await expect.poll(() => lit(manage)).toBe(true);
  await expect.poll(() => lit(a)).toBe(false);
  expect(lit(b)).toBe(false);
});

test("many extensions wrap beside the name instead of leaving the row", async () => {
  native.actions = Array.from({ length: 9 }, (_, index) => action(`x${index}`));
  const screen = await render(ExtensionsSection);
  screen.container.style.inlineSize = panelWidth;
  const row = screen.container.querySelector<HTMLElement>(".row")!;
  await expect.element(screen.getByRole("button", { name: "Extension x8" })).toBeVisible();
  expect(row.offsetHeight).toBeGreaterThan(34);
  expect(row.scrollWidth).toBeLessThanOrEqual(row.clientWidth);
  const box = rect(row);
  for (const tile of row.querySelectorAll("button[data-extension-install]")) {
    expect(rect(tile).left).toBeGreaterThanOrEqual(box.left);
    expect(rect(tile).right).toBeLessThanOrEqual(box.right);
    expect(rect(tile).top).toBeGreaterThanOrEqual(box.top);
    expect(rect(tile).bottom).toBeLessThanOrEqual(box.bottom);
  }
  const name = screen.container.querySelector(".manage .name")!;
  expect(name.scrollWidth).toBeLessThanOrEqual(name.clientWidth);
});

test("the keyboard reaches the manager and then each extension, each with a ring", async () => {
  native.actions = [action("a")];
  const screen = await render(ExtensionsSection);
  const manage = screen.getByRole("button", { name: "Manage extensions" });
  manage.element().focus({ focusVisible: true } as FocusOptions);
  await expect.element(manage).toHaveFocus();
  expect(getComputedStyle(manage.element()).outlineStyle).toBe("solid");
  const tile = screen.getByRole("button", { name: "Extension a" });
  tile.element().focus({ focusVisible: true } as FocusOptions);
  await expect.element(tile).toHaveFocus();
  expect(getComputedStyle(tile.element()).outlineStyle).toBe("solid");
});
