import { describe, expect, it } from "vitest";
import {
  angleOf,
  apply,
  ellipsePath,
  invert,
  mul,
  pathBounds,
  rectPath,
  rotateAt,
  scaleAt,
  splitIntoPieces,
  subpaths,
  translate,
} from "./geom";
import type { PathData } from "./types";

const close = (a: number, b: number) => expect(a).toBeCloseTo(b, 6);

describe("matrices", () => {
  it("composes right to left", () => {
    const m = mul(translate(10, 0), scaleAt(2, 2, 0, 0));
    const [x, y] = apply(m, 1, 1);
    close(x, 12);
    close(y, 2);
  });

  it("inverts", () => {
    const m = mul(rotateAt(0.7, 5, 3), scaleAt(2, 0.5, 1, 1));
    const [x, y] = apply(mul(invert(m), m), 3.5, -2);
    close(x, 3.5);
    close(y, -2);
  });

  it("reports counter-clockwise angles", () => {
    close(angleOf(rotateAt((-30 * Math.PI) / 180, 0, 0)), 30);
  });
});

describe("pathBounds", () => {
  it("is exact for a rectangle under a transform", () => {
    const b = pathBounds(rectPath(0, 0, 10, 5), translate(3, 4))!;
    expect(b).toEqual({ x: 3, y: 4, w: 10, h: 5 });
  });

  it("uses curve extrema, not control points", () => {
    const b = pathBounds(ellipsePath(0, 0, 20, 10))!;
    close(b.x, 0);
    close(b.y, 0);
    close(b.w, 20);
    close(b.h, 10);
  });

  it("is null for an empty path", () => {
    expect(pathBounds([])).toBeNull();
  });
});

describe("splitIntoPieces", () => {
  it("keeps a hole with its outer contour", () => {
    // An "O" (outer + hole) next to a separate square.
    const path: PathData = [...rectPath(0, 0, 10, 10), ...rectPath(3, 3, 4, 4), ...rectPath(20, 0, 5, 5)];
    const pieces = splitIntoPieces(path);
    expect(pieces).toHaveLength(2);
    expect(subpaths(pieces[0])).toHaveLength(2);
    expect(subpaths(pieces[1])).toHaveLength(1);
  });

  it("treats an island inside a hole as its own piece", () => {
    const path: PathData = [...rectPath(0, 0, 30, 30), ...rectPath(5, 5, 20, 20), ...rectPath(10, 10, 10, 10)];
    expect(splitIntoPieces(path)).toHaveLength(2);
  });
});
