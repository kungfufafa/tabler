import type { RefreshMode } from "./model";

/** Why an object runs: the board opened, its timer, an input / button event, ⌘R or Refresh. */
export type RunTrigger = "open" | "interval" | "event" | "manual";

/** What the scheduler needs to know of an object. */
export interface Scheduled {
  id: string;
  refresh: RefreshMode;
  /** It has a query to run. */
  runs: boolean;
  /** Objects it refreshes when it fires (inputs, dropdowns, buttons). */
  targets: readonly string[];
}

export type Runner = (id: string, trigger: RunTrigger) => Promise<unknown>;

interface Timer {
  handle: ReturnType<typeof setTimeout>;
  seconds: number;
}

/**
 * When a board's objects run (spec §8.4): every object once when the board opens; "After N
 * seconds" again N seconds after its previous run finished (runs never overlap); "Refresh on
 * event" when an input or dropdown wired to it changes or a wired button is clicked; "Do not
 * refresh" only on a manual refresh. A trigger that arrives while the object runs makes it run
 * once more afterwards.
 */
export class RefreshScheduler {
  private objects = new Map<string, Scheduled>();
  private timers = new Map<string, Timer>();
  private running = new Set<string>();
  private again = new Map<string, RunTrigger>();
  private active = false;

  constructor(private readonly runner: Runner) {}

  /**
   * Starts the board: runs every object that has a query (those `initial` accepts, by default
   * all), then keeps their timers.
   */
  start(objects: readonly Scheduled[], initial: (id: string) => boolean = () => true): void {
    this.active = true;
    this.update(objects);
    for (const o of this.objects.values())
      if (o.runs && initial(o.id)) this.run(o.id, "open");
      else this.arm(o.id);
  }

  /** Takes the board's current objects (edited, added, removed): timers follow their modes. */
  update(objects: readonly Scheduled[]): void {
    this.objects = new Map(objects.map((o) => [o.id, o]));
    for (const [id, timer] of this.timers) {
      const o = this.objects.get(id);
      if (!o || !o.runs || o.refresh.mode !== "interval" || o.refresh.seconds !== timer.seconds)
        this.clear(id);
    }
    if (!this.active) return;
    for (const o of this.objects.values())
      if (!this.timers.has(o.id) && !this.running.has(o.id)) this.arm(o.id);
  }

  /** Stops every timer; runs still in flight finish without re-arming. */
  stop(): void {
    this.active = false;
    for (const id of [...this.timers.keys()]) this.clear(id);
    this.again.clear();
  }

  /**
   * An input / dropdown changed or a button was clicked: its "Refresh on event" targets run. A
   * source that fires on its own (a dropdown that took its first option when the board opened)
   * says so with `trigger`, so that the runs it causes count as automatic ones.
   */
  fire(sourceId: string, trigger: Extract<RunTrigger, "event" | "open"> = "event"): void {
    const source = this.objects.get(sourceId);
    if (!source) return;
    for (const id of source.targets) {
      const target = this.objects.get(id);
      if (target?.runs && target.refresh.mode === "event") this.run(id, trigger);
    }
  }

  /** Runs one object now (Refresh), whatever its mode. */
  refresh(id: string): void {
    if (this.objects.get(id)?.runs) this.run(id, "manual");
  }

  /** Runs every object that has a query (⌘R). */
  refreshAll(): void {
    for (const o of this.objects.values()) if (o.runs) this.run(o.id, "manual");
  }

  isRunning(id: string): boolean {
    return this.running.has(id);
  }

  private run(id: string, trigger: RunTrigger): void {
    if (!this.active) return;
    if (this.running.has(id)) {
      // A timer never queues; an event or a manual refresh runs once more afterwards.
      if (trigger !== "interval" && !this.again.has(id)) this.again.set(id, trigger);
      return;
    }
    this.clear(id);
    this.running.add(id);
    let done: Promise<unknown>;
    try {
      done = this.runner(id, trigger);
    } catch (err) {
      done = Promise.reject(err);
    }
    void done
      .catch(() => {})
      .finally(() => {
        this.running.delete(id);
        if (!this.active) return;
        const next = this.again.get(id);
        if (next) {
          this.again.delete(id);
          this.run(id, next);
        } else this.arm(id);
      });
  }

  private arm(id: string): void {
    const o = this.objects.get(id);
    if (!o?.runs || o.refresh.mode !== "interval" || this.timers.has(id)) return;
    const seconds = o.refresh.seconds;
    const handle = setTimeout(
      () => {
        this.timers.delete(id);
        this.run(id, "interval");
      },
      Math.max(1, seconds) * 1000,
    );
    this.timers.set(id, { handle, seconds });
  }

  private clear(id: string): void {
    const timer = this.timers.get(id);
    if (!timer) return;
    clearTimeout(timer.handle);
    this.timers.delete(id);
  }
}
