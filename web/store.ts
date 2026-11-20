// Saved tanks in IndexedDB (SPEC §3.5, WB3). The bytes are the native
// app's save files (SPEC §2.6, desktank_sim::save), so an exported web save
// opens on the desktop and the other way round.
//
// One object store, "saves", keyed by [tank name, generation]: each save is
// a new generation in one transaction that also deletes all but the newest
// KEEP, so a save is replaced atomically (IndexedDB's transactions stand in
// for the native write-fsync-rename) and the last few are kept, as on
// native. Only the tab that owns the tank (Web Locks, main.ts) writes.
//
// Ownership epoch: each new owner bumps a counter stored beside the saves
// ([tank, "owner"], outside the generations' key range), and every write
// checks it in its own transaction, so an owner whose lock was stolen (a
// frozen tab waking up) can't write its stale tank over the new owner's.

const DB_NAME = "desktank";
const DB_VERSION = 1;
const STORE = "saves";
const KEEP = 3;

// A saved generation as stored, and as readSaves returns it (with its key).
export interface SaveRecord {
  bytes: Uint8Array;
  tick: number;
  savedAt: number;
}
export type SaveKey = [tank: string, generation: number];
export interface Save extends SaveRecord {
  key: SaveKey;
}

const done = <T>(req: IDBRequest<T>): Promise<T> =>
  new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });

const committed = (tx: IDBTransaction): Promise<void> =>
  new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error ?? new Error("the save was aborted"));
  });

// Every generation of `tank`.
const range = (tank: string) => IDBKeyRange.bound([tank, 0], [tank, Infinity]);
const epochKey = (tank: string) => [tank, "owner"];

// Claims the tank for a new owner: resolves to its epoch, which its writes
// pass to writeSave.
export async function claim(db: IDBDatabase, tank: string): Promise<number> {
  const tx = db.transaction(STORE, "readwrite");
  const store = tx.objectStore(STORE);
  let epoch = 0;
  const req = store.get(epochKey(tank));
  req.onsuccess = () => {
    epoch = (Number.isInteger(req.result) ? (req.result as number) : 0) + 1;
    store.put(epoch, epochKey(tank));
  };
  await committed(tx);
  return epoch;
}

// Opens the database, or resolves to null where IndexedDB is unavailable
// (some private modes): the page then runs without saving.
export async function openStore(): Promise<IDBDatabase | null> {
  if (typeof indexedDB === "undefined") return null;
  try {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => req.result.createObjectStore(STORE);
    const db = await done(req);
    // Another tab of a newer build wants to upgrade: let it.
    db.onversionchange = () => db.close();
    return db;
  } catch {
    return null;
  }
}

// Writes `bytes` as the newest save of `tank` and prunes the older ones,
// unless another owner has claimed the tank since `epoch` (then it rejects
// and writes nothing). Started synchronously, so a `pagehide` handler gets
// its transaction queued before the page goes. Resolves once committed.
export function writeSave(db: IDBDatabase, tank: string, bytes: Uint8Array, tick: number, epoch: number | null): Promise<void> {
  const tx = db.transaction(STORE, "readwrite");
  const store = tx.objectStore(STORE);
  let lost = false;
  const owner = store.get(epochKey(tank));
  owner.onsuccess = () => {
    if (owner.result !== epoch) {
      lost = true;
      tx.abort();
    }
  };
  const keys = store.getAllKeys(range(tank));
  keys.onsuccess = () => {
    if (lost) return;
    const gens = (keys.result as SaveKey[]).map((k) => k[1]);
    const next = gens.length ? gens[gens.length - 1] + 1 : 1;
    const record: SaveRecord = { bytes, tick, savedAt: Date.now() };
    store.put(record, [tank, next]);
    for (const g of gens.slice(0, Math.max(0, gens.length - (KEEP - 1)))) store.delete([tank, g]);
  };
  return committed(tx).catch((err: unknown) => {
    throw lost ? new Error("another tab owns the tank now") : err;
  });
}

// The saves of `tank`, newest first: [{ key, bytes, tick, savedAt }].
export async function readSaves(db: IDBDatabase, tank: string): Promise<Save[]> {
  const tx = db.transaction(STORE, "readonly");
  const store = tx.objectStore(STORE);
  const [keys, values] = await Promise.all([done(store.getAllKeys(range(tank))), done(store.getAll(range(tank)))]);
  return keys.map((key, i) => ({ key: key as SaveKey, ...(values[i] as SaveRecord) })).reverse();
}
