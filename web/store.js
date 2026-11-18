// Saved tanks in IndexedDB (SPEC §3.5, WB3). The bytes are the native
// app's save files (SPEC §2.6, desktank_sim::save), so an exported web save
// opens on the desktop and the other way round.
//
// One object store, "saves", keyed by [tank name, generation]: each save is
// a new generation in one transaction that also deletes all but the newest
// KEEP, so a save is replaced atomically (IndexedDB's transactions stand in
// for the native write-fsync-rename) and the last few are kept, as on
// native. Only the tab that owns the tank (Web Locks, main.js) writes.

const DB_NAME = "desktank";
const DB_VERSION = 1;
const STORE = "saves";
const KEEP = 3;

const done = (req) =>
  new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });

const committed = (tx) =>
  new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error ?? new Error("the save was aborted"));
  });

// Every generation of `tank`.
const range = (tank) => IDBKeyRange.bound([tank, 0], [tank, Infinity]);

// Opens the database, or resolves to null where IndexedDB is unavailable
// (some private modes): the page then runs without saving.
export async function openStore() {
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

// Writes `bytes` as the newest save of `tank` and prunes the older ones.
// Started synchronously, so a `pagehide` handler gets its transaction queued
// before the page goes. Resolves once it is committed.
export function writeSave(db, tank, bytes, tick) {
  const tx = db.transaction(STORE, "readwrite");
  const store = tx.objectStore(STORE);
  const keys = store.getAllKeys(range(tank));
  keys.onsuccess = () => {
    const gens = keys.result.map((k) => k[1]);
    const next = gens.length ? gens[gens.length - 1] + 1 : 1;
    store.put({ bytes, tick, savedAt: Date.now() }, [tank, next]);
    for (const g of gens.slice(0, Math.max(0, gens.length - (KEEP - 1)))) store.delete([tank, g]);
  };
  return committed(tx);
}

// The saves of `tank`, newest first: [{ key, bytes, tick, savedAt }].
export async function readSaves(db, tank) {
  const tx = db.transaction(STORE, "readonly");
  const store = tx.objectStore(STORE);
  const [keys, values] = await Promise.all([done(store.getAllKeys(range(tank))), done(store.getAll(range(tank)))]);
  return keys.map((key, i) => ({ key, ...values[i] })).reverse();
}

// Deletes one save (one that doesn't load).
export function deleteSave(db, key) {
  const tx = db.transaction(STORE, "readwrite");
  tx.objectStore(STORE).delete(key);
  return committed(tx);
}
