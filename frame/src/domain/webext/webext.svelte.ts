import {
  commands,
  type WebExtensionAccessRequestView,
  type WebExtensionReview,
  type WebExtensionView,
} from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";

let available = $state<boolean | null>(null);
let installed = $state<WebExtensionView[]>([]);
let pendingReview = $state<WebExtensionReview | null>(null);
let preparing = $state(false);
let confirming = $state(false);
// The extension just added, until it has started.
let settling = $state<string | null>(null);
let failure = $state<string | null>(null);
let refreshTimer: ReturnType<typeof setTimeout> | undefined;
// Extensions ask for access one request at a time; later ones wait.
let accessQueue = $state<WebExtensionAccessRequestView[]>([]);
let answering = $state(false);

export const isAvailable = () => available === true;
export const list = () => installed;
export const review = () => pendingReview;
export const isPreparing = () => preparing;
export const isConfirming = () => confirming;
export const isInstalling = () => preparing || confirming || settling !== null;
export const error = () => failure;
export const accessRequest = () => accessQueue[0] ?? null;
export const isAnswering = () => answering;
export const named = (id: string) => installed.find((extension) => extension.id === id) ?? null;

export function listenForAccess(): Promise<() => void> {
  return events.webExtensionAccessRequested.listen((event) => {
    accessQueue = [...accessQueue, event.payload];
    if (named(event.payload.extension_id) === null) void refresh();
  });
}

export async function answerAccess(allowed: boolean): Promise<void> {
  const current = accessQueue[0];
  if (current === undefined || answering) return;
  answering = true;
  const result = await commands.webExtensionAnswerAccess(current, allowed);
  answering = false;
  accessQueue = accessQueue.slice(1);
  if (result.status !== "ok") failure = result.error;
}

export async function refresh(): Promise<void> {
  const result = await commands.webExtensionList();
  if (result.status === "ok") {
    available = true;
    installed = result.data;
    if (settling !== null && named(settling)?.state !== "starting") settling = null;
    // Extensions start in the background after a change; follow them until
    // every one has settled.
    clearTimeout(refreshTimer);
    if (installed.some((extension) => extension.state === "starting")) {
      refreshTimer = setTimeout(() => void refresh(), 600);
    }
  } else {
    settling = null;
    if (available === null) available = false;
  }
}

type Reviewed =
  { status: "ok"; data: WebExtensionReview | null } | { status: "error"; error: string };

// Every way of installing ends in the same review; one runs at a time.
async function reviewing(request: () => Promise<Reviewed>): Promise<void> {
  if (preparing) return;
  preparing = true;
  failure = null;
  const result = await request();
  preparing = false;
  if (result.status === "ok") {
    pendingReview = result.data;
    failure = null;
  } else failure = result.error;
}

export const prepare = (tabId: string) => reviewing(() => commands.webExtensionPrepare(tabId));
export const chooseFile = (folder: boolean) =>
  reviewing(() => commands.webExtensionChooseFile(folder));
const prepareFile = (path: string) => reviewing(() => commands.webExtensionPrepareFile(path));
export const reviewUpdate = (id: string) => reviewing(() => commands.webExtensionReviewUpdate(id));

export function listenForDrops(accept: () => boolean = () => true): Promise<() => void> {
  return events.webExtensionDropped.listen((event) => {
    if (accept()) void prepareFile(event.payload);
  });
}

export async function confirm(reviewId: string): Promise<void> {
  const current = pendingReview;
  if (current === null || current.review_id !== reviewId || confirming) return;
  confirming = true;
  const result = await commands.webExtensionConfirm(current.id, current.review_id);
  confirming = false;
  if (result.status === "ok") {
    if (pendingReview?.review_id === current.review_id) {
      pendingReview = null;
      failure = null;
    }
    settling = current.id;
    await refresh();
  } else if (pendingReview?.review_id === current.review_id) {
    failure = result.error;
  }
}

export function cancel(reviewId: string): void {
  if (pendingReview?.review_id !== reviewId || confirming) return;
  pendingReview = null;
  failure = null;
  void commands.webExtensionCancel(reviewId);
}

export async function setEnabled(id: string, enabled: boolean): Promise<void> {
  const result = await commands.webExtensionSetEnabled(id, enabled);
  failure = result.status === "ok" ? null : result.error;
  await refresh();
}

export async function setAccess(id: string, mode: string, sites: string[] = []): Promise<void> {
  const result = await commands.webExtensionSetAccess(id, mode, sites);
  failure = result.status === "ok" ? null : result.error;
  await refresh();
}

export async function openOptions(id: string): Promise<void> {
  const result = await commands.webExtensionOpenOptions(id);
  if (result.status !== "ok") failure = result.error;
}

export async function retry(extension: WebExtensionView): Promise<void> {
  const result = await commands.webExtensionRetry(extension.profile_id, extension.install_id);
  failure = result.status === "ok" ? null : result.error;
  await refresh();
}

export async function uninstall(id: string): Promise<void> {
  const result = await commands.webExtensionUninstall(id);
  failure = result.status === "ok" ? null : result.error;
  await refresh();
}
