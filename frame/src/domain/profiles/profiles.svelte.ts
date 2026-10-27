/**
 * The persistent profiles and the one this window browses as. Rust owns the
 * roster, each profile's isolation and its avatar; this mirrors its last
 * projection. A picked photo is staged natively and named here by a token.
 */
import {
  commands,
  type ProfileAvatarInputV1,
  type ProfileEntryView,
  type ProfilesView,
} from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";
import { forgetDrafts } from "$shared/lib/drafts";
import { observe } from "$shared/lib/observe";
import { settle, type Settlement } from "$domain/operations";

let view = $state.raw<ProfilesView | null>(null);
let users = 0;
let stop: (() => void) | null = null;
let generation = 0;

export const all = (): ProfileEntryView[] => view?.profiles ?? [];
export const current = (): ProfileEntryView | null =>
  view?.profiles.find((profile) => profile.id === view?.current) ?? null;
export const inPrivate = () => view?.private ?? false;
/** The tabs of the private window while one exists, in front or set aside. */
export const privateTabs = (): number | null => view?.private_tabs ?? null;
/** A private window exists and the window is showing the regular tabs. */
export const privateAside = () => privateTabs() !== null && !inPrivate();
/** The profiles a window could switch to from here. */
export const others = (): ProfileEntryView[] =>
  all().filter((profile) => profile.id !== view?.current);

async function read(started: number) {
  const reply = await observe(
    Promise.resolve().then(() => commands.profilesGet()),
    5000,
  );
  if (started !== generation || reply.state !== "received") return;
  if (reply.value?.status === "ok" && reply.value.data) view = reply.value.data;
}

/** Listens while anything shows profiles; the first user reads them. */
export async function retain() {
  users += 1;
  if (users > 1) {
    if (!view) await read(generation);
    return;
  }
  const started = ++generation;
  const unlisten = await events.profilesChanged.listen((event) => {
    if (started === generation) view = event.payload;
  });
  if (started !== generation || users === 0) {
    unlisten();
    return;
  }
  stop = unlisten;
  await read(started);
}

export function release() {
  users = Math.max(0, users - 1);
  if (users > 0) return;
  generation += 1;
  stop?.();
  stop = null;
}

export type AvatarDraft = ProfileAvatarInputV1;

const applied = (result: Settlement) => result.outcome === "applied" || result.outcome === "no_op";

/** With `andSwitch`, the window shows the new profile once it exists. */
export const create = async (name: string, avatar: AvatarDraft, andSwitch = false) =>
  applied(await settle(commands.profilesCreate(name, avatar, andSwitch), andSwitch ? 8000 : 5000));
export const switchTo = async (id: string) =>
  applied(await settle(commands.profilesSwitch(id), 8000));
export const rename = async (id: string, name: string) =>
  applied(await settle(commands.profilesRename(id, name)));
export const setAvatar = async (id: string, avatar: AvatarDraft) =>
  applied(await settle(commands.profilesSetAvatar(id, avatar)));
/** Deletion settles once its native and durable phases both finish. */
export const remove = async (id: string) => {
  const result = await settle(commands.profilesDelete(id), 30_000);
  const removed = applied(result) || result.outcome === "deferred";
  if (removed) forgetDrafts(id);
  return removed;
};

export type PhotoPick = { token: string } | "cancelled" | "unsupported" | "unavailable";

/** The native picker; minutes may pass while the person chooses. */
export async function pickPhoto(): Promise<PhotoPick> {
  const reply = await observe(
    Promise.resolve().then(() => commands.profileAvatarPick()),
    10 * 60_000,
  );
  if (reply.state !== "received" || reply.value?.status !== "ok") return "unavailable";
  const pick = reply.value.data;
  return pick.kind === "picked" ? { token: pick.token } : pick.kind;
}
