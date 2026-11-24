// Tab ownership of a tank (SPEC §3.5, §2.6; WB3): exactly one tab
// simulates the tank and writes its saves. Ownership is a Web Lock, which
// the browser releases when the tab closes or crashes.
//
// A tab that finds the lock held is a viewer: it queues for the lock (so it
// takes over by itself when the owner closes) and can ask to take over now.
// Take over posts a BroadcastChannel message; the owner saves, releases the
// lock and becomes a viewer, and the queued request is granted. If the
// owner doesn't answer within HANDOVER_MS (a frozen tab), the viewer steals
// the lock; the old owner learns it lost the lock from its rejected request.

export const HANDOVER_MS = 1000;

// own() once this tab holds the lock; view() once it doesn't (at start,
// after handing over, or stolen); handOver() before releasing on request
// (save).
export interface OwnershipCallbacks {
  own(stolen: boolean): void;
  view(): void;
  handOver(): Promise<void>;
}

export class Ownership {
  name: string;
  callbacks: OwnershipCallbacks;
  owner: boolean;
  release: (() => void) | null;
  queued: AbortController | null;
  stealTimer: ReturnType<typeof setTimeout> | undefined;
  channel: BroadcastChannel;

  constructor(name: string, callbacks: OwnershipCallbacks) {
    this.name = name;
    this.callbacks = callbacks;
    this.owner = false;
    this.release = null; // resolves the held lock's callback promise
    this.queued = null; // AbortController of the queued request
    this.stealTimer = undefined;
    this.channel = new BroadcastChannel(name);
    this.channel.onmessage = (e: MessageEvent<{ type?: string } | null>) => {
      if (e.data?.type === "take-over" && this.owner) this.handOver();
    };
  }

  // Resolves once this tab knows whether it owns the tank (and has called
  // own() or view()).
  async start(): Promise<void> {
    const got = await this.request({ ifAvailable: true });
    if (!got) {
      this.callbacks.view();
      this.queue();
    }
  }

  // Requests the lock with `options`. Resolves to true once held, false if
  // `ifAvailable` found it busy; a queued request resolves only when granted.
  request(options: LockOptions): Promise<boolean> {
    return new Promise((resolve) => {
      navigator.locks
        .request(this.name, options, (lock) => {
          if (!lock) {
            resolve(false);
            return null;
          }
          clearTimeout(this.stealTimer);
          this.queued = null;
          this.owner = true;
          const held = new Promise<void>((r) => {
            this.release = r;
          });
          this.callbacks.own(Boolean(options.steal));
          resolve(true);
          return held;
        })
        .catch((err: unknown) => {
          // A queued request we aborted, or the held lock was stolen.
          const aborted = (err as Error | null)?.name === "AbortError";
          if (!aborted) console.warn(`desktank: tank lock: ${err}`);
          if (this.owner && aborted) {
            this.owner = false;
            this.release = null;
            this.callbacks.view();
            this.queue();
          }
          resolve(false);
        });
    });
  }

  queue(): void {
    if (this.queued || this.owner) return;
    this.queued = new AbortController();
    this.request({ signal: this.queued.signal });
  }

  // Viewer: asks the owner to hand over; steals the lock if it doesn't
  // within HANDOVER_MS.
  takeOver(): void {
    if (this.owner) return;
    this.queue();
    this.channel.postMessage({ type: "take-over" });
    clearTimeout(this.stealTimer);
    this.stealTimer = setTimeout(() => {
      if (this.owner) return;
      this.queued?.abort();
      this.queued = null;
      this.request({ steal: true });
    }, HANDOVER_MS);
  }

  // Owner, asked to take over: save, then release and become a viewer.
  async handOver(): Promise<void> {
    try {
      await this.callbacks.handOver();
    } finally {
      if (this.owner) {
        this.owner = false;
        const release = this.release;
        this.release = null;
        this.callbacks.view();
        release?.();
        this.queue();
      }
    }
  }
}
