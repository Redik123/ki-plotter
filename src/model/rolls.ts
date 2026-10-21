// Rolls of vinyl: which one is in the cutter, what was cut on each, and
// roughly how much is left. Shared by every project (saved by the app, not
// in the .kplot file).
//
// Along a roll, x = 0 is its very beginning. On the canvas, x = 0 is the
// start of the free part: the roll's history sits at negative x.

import { backend } from "../backend";
import type { JobResult } from "./types";

export interface RollCut {
  id: string;
  /** Unix time, ms. */
  date: number;
  name: string;
  /** Where it starts along the roll and how much it used, mm. */
  x: number;
  length: number;
  /** What the blade cut, in roll coordinates (mm), simplified. */
  lines: [number, number][][];
}

export interface Roll {
  id: string;
  name: string;
  /** Vinyl colour. */
  color: string;
  /** Width across the roll, mm. */
  width: number;
  /** Length when new, mm. */
  length: number;
  /** Length used from the start, mm. */
  used: number;
  cuts: RollCut[];
  created: number;
}

export interface RollsData {
  version: 1;
  current: string | null;
  /** Margin left after each job along the roll, mm. */
  spacing: number;
  rolls: Roll[];
}

const MAX_POINTS_PER_CUT = 40_000;

function emptyData(): RollsData {
  return { version: 1, current: null, spacing: 10, rolls: [] };
}

function newId(): string {
  return `r${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
}

/** Keeps a point every `step` mm (and the last one), rounded to 0.1 mm. */
function simplify(lines: [number, number][][], step: number): [number, number][][] {
  const r = (v: number) => Math.round(v * 10) / 10;
  return lines
    .map((pts) => {
      const out: [number, number][] = [];
      let last: [number, number] | null = null;
      pts.forEach(([x, y], i) => {
        const keep = !last || i === pts.length - 1 || Math.hypot(x - last[0], y - last[1]) >= step;
        if (keep) {
          last = [x, y];
          out.push([r(x), r(y)]);
        }
      });
      return out;
    })
    .filter((l) => l.length >= 2);
}

function count(lines: [number, number][][]): number {
  return lines.reduce((n, l) => n + l.length, 0);
}

export class Rolls {
  data: RollsData = emptyData();
  private listeners: (() => void)[] = [];
  private onError: (msg: string) => void = () => {};

  on(fn: () => void): void {
    this.listeners.push(fn);
  }

  setErrorHandler(fn: (msg: string) => void): void {
    this.onError = fn;
  }

  async load(): Promise<void> {
    try {
      const text = await backend.loadRolls();
      if (!text) return;
      const raw = JSON.parse(text) as Partial<RollsData>;
      if (raw.version !== 1 || !Array.isArray(raw.rolls)) throw new Error("format inconnu");
      this.data = { ...emptyData(), ...raw, rolls: raw.rolls };
      if (!this.data.rolls.some((r) => r.id === this.data.current)) this.data.current = null;
    } catch (e) {
      this.onError(`Rouleaux illisibles : ${e instanceof Error ? e.message : String(e)}`);
    }
    this.emit();
  }

  private emit(): void {
    for (const fn of this.listeners) fn();
  }

  /** After every change: redraw, then save. */
  private changed(): void {
    this.emit();
    backend.saveRolls(JSON.stringify(this.data)).catch((e: unknown) => this.onError(String(e)));
  }

  get(id: string): Roll | undefined {
    return this.data.rolls.find((r) => r.id === id);
  }

  current(): Roll | null {
    return (this.data.current && this.get(this.data.current)) || null;
  }

  setCurrent(id: string | null): void {
    this.data.current = id && this.get(id) ? id : null;
    this.changed();
  }

  /** Next free default name: "Feuille 1", "Feuille 2"… */
  nextName(): string {
    let n = this.data.rolls.length + 1;
    while (this.data.rolls.some((r) => r.name === `Feuille ${n}`)) n++;
    return `Feuille ${n}`;
  }

  create(fields: Pick<Roll, "name" | "color" | "width" | "length">): Roll {
    const roll: Roll = { id: newId(), ...fields, used: 0, cuts: [], created: Date.now() };
    this.data.rolls.push(roll);
    this.data.current = roll.id;
    this.changed();
    return roll;
  }

  update(id: string, patch: Partial<Pick<Roll, "name" | "color" | "width" | "length">>): void {
    const r = this.get(id);
    if (!r) return;
    Object.assign(r, patch);
    this.changed();
  }

  remove(id: string): void {
    this.data.rolls = this.data.rolls.filter((r) => r.id !== id);
    if (this.data.current === id) this.data.current = null;
    this.changed();
  }

  remaining(r: Roll): number {
    return Math.max(0, r.length - r.used);
  }

  /** The user measured what is left: trust that over our estimate. */
  setRemaining(id: string, mm: number): void {
    const r = this.get(id);
    if (!r || !Number.isFinite(mm)) return;
    r.used = Math.min(r.length, Math.max(0, r.length - mm));
    this.changed();
  }

  /** Takes back the last recorded cut (sent by mistake, deleted on the Pi…). */
  undoLast(id: string): RollCut | null {
    const r = this.get(id);
    const cut = r?.cuts.pop();
    if (!r || !cut) return null;
    r.used = Math.max(0, r.used - cut.length);
    this.changed();
    return cut;
  }

  /** A fresh roll of the same kind: history cleared, full length again. */
  renew(id: string): void {
    const r = this.get(id);
    if (!r) return;
    r.cuts = [];
    r.used = 0;
    r.created = Date.now();
    this.changed();
  }

  /** Length a job takes along the roll (its extent plus the margin). */
  footprint(job: JobResult, fromOrigin: boolean): number {
    const b = job.stats.bounds;
    const extent = fromOrigin ? b.w : Math.max(0, b.x) + b.w;
    return extent + this.data.spacing;
  }

  /**
   * Records a job just sent to the cutter on the current roll. With
   * `fromOrigin` the job starts at the blade (the start of the free part);
   * otherwise document x = 0 is that start.
   */
  record(job: JobResult, name: string, fromOrigin: boolean): RollCut | null {
    const r = this.current();
    if (!r) return null;
    const b = job.stats.bounds;
    const shift = r.used - (fromOrigin ? b.x : 0);
    const raw = job.moves
      .filter((m) => m.down)
      .map((m) => m.pts.map(([x, y]): [number, number] => [x + shift, y]));
    let step = 0.4;
    let lines = simplify(raw, step);
    while (count(lines) > MAX_POINTS_PER_CUT && step < 50) {
      step *= 2;
      lines = simplify(raw, step);
    }
    const tenth = (v: number) => Math.round(v * 10) / 10;
    const cut: RollCut = {
      id: newId(),
      date: Date.now(),
      name,
      x: tenth(r.used),
      length: tenth(this.footprint(job, fromOrigin)),
      lines,
    };
    r.cuts.push(cut);
    r.used = tenth(r.used + cut.length);
    this.changed();
    return cut;
  }
}

export const rolls = new Rolls();

export function fmtMeters(mm: number): string {
  return `${(mm / 1000).toLocaleString("fr-FR", { maximumFractionDigits: mm < 1000 ? 2 : 1 })} m`;
}
