import { beforeEach, describe, expect, it, vi } from "vitest";

const saved: string[] = [];
vi.mock("../backend", () => ({
  backend: {
    loadRolls: async () => saved.at(-1) ?? null,
    saveRolls: async (s: string) => {
      saved.push(s);
    },
  },
}));

const { Rolls } = await import("./rolls");
import type { JobResult } from "./types";

/** A job whose knife path is a rectangle at (x, y), w × h. */
function job(x: number, y: number, w: number, h: number, points = 5): JobResult {
  const pts: [number, number][] = [];
  for (let i = 0; i < points; i++) pts.push([x + (w * i) / Math.max(1, points - 1), y]);
  pts.push([x + w, y + h], [x, y + h], [x, y]);
  return {
    data: "IN;",
    moves: [
      { down: false, pts: [[0, 0], [x, y]] },
      { down: true, pts },
    ],
    stats: { pathCount: 1, cutLength: 0, travelLength: 0, estimatedSeconds: 1, bounds: { x, y, w, h } },
  };
}

describe("rolls", () => {
  let rolls: InstanceType<typeof Rolls>;
  beforeEach(() => {
    saved.length = 0;
    rolls = new Rolls();
  });

  it("records cuts one after another and tracks what is left", () => {
    const r = rolls.create({ name: "Feuille 1", color: "#ffffff", width: 700, length: 10_000 });
    expect(rolls.current()?.id).toBe(r.id);

    // From the origin: the job's left edge goes where the free part starts.
    const a = rolls.record(job(40, 20, 300, 100), "Vitrine", true)!;
    expect(a.x).toBe(0);
    expect(a.length).toBe(300 + rolls.data.spacing);
    expect(a.lines[0][0]).toEqual([0, 20]);

    // Not from the origin: document x = 0 is the free start, so x counts.
    const b = rolls.record(job(50, 0, 100, 50), "Plaque", false)!;
    expect(b.x).toBe(310);
    expect(b.length).toBe(150 + rolls.data.spacing);
    expect(b.lines[0][0]).toEqual([360, 0]);

    expect(rolls.remaining(r)).toBe(10_000 - 310 - 160);
    expect(saved.length).toBeGreaterThan(0);
  });

  it("undoes the last cut and accepts a measured remainder", () => {
    const r = rolls.create({ name: "Noir 1", color: "#2b2f36", width: 610, length: 5000 });
    rolls.record(job(0, 0, 1000, 200), "A", true);
    rolls.record(job(0, 0, 500, 200), "B", true);
    expect(rolls.undoLast(r.id)?.name).toBe("B");
    expect(rolls.remaining(r)).toBe(5000 - 1010);
    rolls.setRemaining(r.id, 3500);
    expect(rolls.remaining(r)).toBe(3500);
    rolls.renew(r.id);
    expect(rolls.remaining(r)).toBe(5000);
    expect(r.cuts).toHaveLength(0);
  });

  it("keeps huge traced jobs small", () => {
    rolls.create({ name: "F", color: "#fff", width: 700, length: 10_000 });
    const cut = rolls.record(job(0, 0, 600, 300, 200_000), "Logo tracé", true)!;
    const points = cut.lines.reduce((n, l) => n + l.length, 0);
    expect(points).toBeLessThanOrEqual(40_000);
    expect(cut.lines[0].at(-1)).toEqual([0, 0]);
  });

  it("round-trips through the saved file", async () => {
    rolls.create({ name: "Feuille 1", color: "#fff", width: 700, length: 10_000 });
    rolls.record(job(0, 0, 100, 100), "A", true);
    const again = new Rolls();
    await again.load();
    expect(again.current()?.name).toBe("Feuille 1");
    expect(again.current()?.cuts).toHaveLength(1);
    expect(again.nextName()).toBe("Feuille 2");
  });
});
