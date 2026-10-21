// Small geometry helpers for the editor: matrices, bounds, Path2D.
// The heavy lifting (flattening, cut paths, clipping) lives in Rust.

import type { Mat, PathData, Rect, Seg } from "./types";

export const IDENTITY: Mat = [1, 0, 0, 1, 0, 0];

/** m ∘ n: apply n first, then m. */
export function mul(m: Mat, n: Mat): Mat {
  const [a, b, c, d, e, f] = m;
  const [a2, b2, c2, d2, e2, f2] = n;
  return [
    a * a2 + c * b2,
    b * a2 + d * b2,
    a * c2 + c * d2,
    b * c2 + d * d2,
    a * e2 + c * f2 + e,
    b * e2 + d * f2 + f,
  ];
}

export function translate(tx: number, ty: number): Mat {
  return [1, 0, 0, 1, tx, ty];
}

export function scaleAt(sx: number, sy: number, cx: number, cy: number): Mat {
  return [sx, 0, 0, sy, cx - sx * cx, cy - sy * cy];
}

/** Rotation by `rad` (clockwise on screen, since y points down) around (cx, cy). */
export function rotateAt(rad: number, cx: number, cy: number): Mat {
  const c = Math.cos(rad);
  const s = Math.sin(rad);
  return [c, s, -s, c, cx - c * cx + s * cy, cy - s * cx - c * cy];
}

export function invert(m: Mat): Mat {
  const [a, b, c, d, e, f] = m;
  const det = a * d - b * c || 1e-12;
  return [d / det, -b / det, -c / det, a / det, (c * f - d * e) / det, (b * e - a * f) / det];
}

export function apply(m: Mat, x: number, y: number): [number, number] {
  return [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]];
}

/** Counter-clockwise angle of the matrix's x axis, in degrees (as shown in the UI). */
export function angleOf(m: Mat): number {
  return (-Math.atan2(m[1], m[0]) * 180) / Math.PI;
}

export function transformPath(path: PathData, m: Mat): PathData {
  return path.map((s): Seg => {
    switch (s.t) {
      case "M":
      case "L": {
        const [x, y] = apply(m, s.x, s.y);
        return { t: s.t, x, y };
      }
      case "C": {
        const [x1, y1] = apply(m, s.x1, s.y1);
        const [x2, y2] = apply(m, s.x2, s.y2);
        const [x, y] = apply(m, s.x, s.y);
        return { t: "C", x1, y1, x2, y2, x, y };
      }
      case "Z":
        return s;
    }
  });
}

/** Parameters in (0, 1) where one coordinate of a cubic has an extremum. */
function cubicExtrema(p0: number, p1: number, p2: number, p3: number): number[] {
  const a = p1 - p0;
  const b = p2 - p1;
  const c = p3 - p2;
  const qa = a - 2 * b + c;
  const qb = 2 * (b - a);
  const qc = a;
  const out: number[] = [];
  if (Math.abs(qa) < 1e-12) {
    if (Math.abs(qb) > 1e-12) out.push(-qc / qb);
  } else {
    const disc = qb * qb - 4 * qa * qc;
    if (disc >= 0) {
      const r = Math.sqrt(disc);
      out.push((-qb + r) / (2 * qa), (-qb - r) / (2 * qa));
    }
  }
  return out.filter((t) => t > 0 && t < 1);
}

function cubicAt(p0: number, p1: number, p2: number, p3: number, t: number): number {
  const u = 1 - t;
  return u * u * u * p0 + 3 * u * u * t * p1 + 3 * u * t * t * p2 + t * t * t * p3;
}

export class Bounds {
  minX = Infinity;
  minY = Infinity;
  maxX = -Infinity;
  maxY = -Infinity;

  add(x: number, y: number): void {
    if (x < this.minX) this.minX = x;
    if (y < this.minY) this.minY = y;
    if (x > this.maxX) this.maxX = x;
    if (y > this.maxY) this.maxY = y;
  }

  addRect(r: Rect | null): void {
    if (!r) return;
    this.add(r.x, r.y);
    this.add(r.x + r.w, r.y + r.h);
  }

  get empty(): boolean {
    return this.minX > this.maxX;
  }

  rect(): Rect | null {
    if (this.empty) return null;
    return { x: this.minX, y: this.minY, w: this.maxX - this.minX, h: this.maxY - this.minY };
  }
}

/** Exact bounds of a path under an affine transform. */
export function pathBounds(path: PathData, m: Mat = IDENTITY): Rect | null {
  const b = new Bounds();
  let cx = 0;
  let cy = 0;
  for (const s of path) {
    if (s.t === "M" || s.t === "L") {
      [cx, cy] = apply(m, s.x, s.y);
      b.add(cx, cy);
    } else if (s.t === "C") {
      const [x1, y1] = apply(m, s.x1, s.y1);
      const [x2, y2] = apply(m, s.x2, s.y2);
      const [x3, y3] = apply(m, s.x, s.y);
      b.add(x3, y3);
      for (const t of cubicExtrema(cx, x1, x2, x3)) b.add(cubicAt(cx, x1, x2, x3, t), cubicAt(cy, y1, y2, y3, t));
      for (const t of cubicExtrema(cy, y1, y2, y3)) b.add(cubicAt(cx, x1, x2, x3, t), cubicAt(cy, y1, y2, y3, t));
      cx = x3;
      cy = y3;
    }
  }
  return b.rect();
}

export function unionRects(rects: (Rect | null)[]): Rect | null {
  const b = new Bounds();
  for (const r of rects) b.addRect(r);
  return b.rect();
}

export function rectContains(outer: Rect, inner: Rect): boolean {
  return (
    inner.x >= outer.x &&
    inner.y >= outer.y &&
    inner.x + inner.w <= outer.x + outer.w &&
    inner.y + inner.h <= outer.y + outer.h
  );
}

const path2dCache = new WeakMap<PathData, Path2D>();

/** Path2D in the path's own coordinates; cached per path array (paths are never mutated in place). */
export function toPath2D(path: PathData): Path2D {
  let p = path2dCache.get(path);
  if (p) return p;
  p = new Path2D();
  for (const s of path) {
    switch (s.t) {
      case "M":
        p.moveTo(s.x, s.y);
        break;
      case "L":
        p.lineTo(s.x, s.y);
        break;
      case "C":
        p.bezierCurveTo(s.x1, s.y1, s.x2, s.y2, s.x, s.y);
        break;
      case "Z":
        p.closePath();
        break;
    }
  }
  path2dCache.set(path, p);
  return p;
}

/** Splits a path into its subpaths (each starts with M). */
export function subpaths(path: PathData): PathData[] {
  const out: PathData[] = [];
  let cur: PathData | null = null;
  for (const s of path) {
    if (s.t === "M" || !cur) {
      cur = [];
      out.push(cur);
    }
    cur.push(s);
  }
  return out.filter((p) => p.length > 1);
}

/** Rough polygon of a subpath (curves sampled), good enough for containment tests. */
export function roughPolygon(sub: PathData, steps = 8): [number, number][] {
  const pts: [number, number][] = [];
  let cx = 0;
  let cy = 0;
  for (const s of sub) {
    if (s.t === "M" || s.t === "L") {
      cx = s.x;
      cy = s.y;
      pts.push([cx, cy]);
    } else if (s.t === "C") {
      for (let i = 1; i <= steps; i++) {
        const t = i / steps;
        pts.push([cubicAt(cx, s.x1, s.x2, s.x, t), cubicAt(cy, s.y1, s.y2, s.y, t)]);
      }
      cx = s.x;
      cy = s.y;
    }
  }
  return pts;
}

export function polygonArea(pts: [number, number][]): number {
  let a = 0;
  for (let i = 0, j = pts.length - 1; i < pts.length; j = i++) {
    a += (pts[j][0] + pts[i][0]) * (pts[j][1] - pts[i][1]);
  }
  return Math.abs(a / 2);
}

export function pointInPolygon(x: number, y: number, pts: [number, number][]): boolean {
  let inside = false;
  for (let i = 0, j = pts.length - 1; i < pts.length; j = i++) {
    const [xi, yi] = pts[i];
    const [xj, yj] = pts[j];
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) inside = !inside;
  }
  return inside;
}

/**
 * Groups subpaths into pieces: each outer contour with the holes directly
 * inside it, so "break apart" keeps letter counters as holes.
 */
export function splitIntoPieces(path: PathData): PathData[] {
  const subs = subpaths(path);
  if (subs.length <= 1) return subs;
  const polys = subs.map((s) => roughPolygon(s));
  const areas = polys.map(polygonArea);
  const boxes = subs.map((s) => pathBounds(s));
  const containers: number[][] = subs.map(() => []);
  for (let i = 0; i < subs.length; i++) {
    const bi = boxes[i];
    if (!bi || polys[i].length === 0) continue;
    const [px, py] = polys[i][0];
    for (let j = 0; j < subs.length; j++) {
      const bj = boxes[j];
      if (i === j || !bj || areas[j] <= areas[i]) continue;
      if (rectContains(bj, bi) && pointInPolygon(px, py, polys[j])) containers[i].push(j);
    }
  }
  const pieces = new Map<number, PathData>();
  const order: number[] = [];
  for (let i = 0; i < subs.length; i++) {
    if (containers[i].length % 2 === 0) {
      pieces.set(i, [...subs[i]]);
      order.push(i);
    }
  }
  for (let i = 0; i < subs.length; i++) {
    if (containers[i].length % 2 === 1) {
      // Attach the hole to its smallest container.
      const parent = containers[i].reduce((best, j) => (areas[j] < areas[best] ? j : best));
      const piece = pieces.get(parent);
      if (piece) piece.push(...subs[i]);
      else {
        pieces.set(i, [...subs[i]]);
        order.push(i);
      }
    }
  }
  return order.map((i) => pieces.get(i)!);
}

export function rectPath(x: number, y: number, w: number, h: number): PathData {
  return [
    { t: "M", x, y },
    { t: "L", x: x + w, y },
    { t: "L", x: x + w, y: y + h },
    { t: "L", x, y: y + h },
    { t: "Z" },
  ];
}

/** Ellipse inscribed in the rect, as four cubic arcs. */
export function ellipsePath(x: number, y: number, w: number, h: number): PathData {
  const k = 0.5522847498;
  const rx = w / 2;
  const ry = h / 2;
  const cx = x + rx;
  const cy = y + ry;
  return [
    { t: "M", x: cx + rx, y: cy },
    { t: "C", x1: cx + rx, y1: cy + k * ry, x2: cx + k * rx, y2: cy + ry, x: cx, y: cy + ry },
    { t: "C", x1: cx - k * rx, y1: cy + ry, x2: cx - rx, y2: cy + k * ry, x: cx - rx, y: cy },
    { t: "C", x1: cx - rx, y1: cy - k * ry, x2: cx - k * rx, y2: cy - ry, x: cx, y: cy - ry },
    { t: "C", x1: cx + k * rx, y1: cy - ry, x2: cx + rx, y2: cy - k * ry, x: cx + rx, y: cy },
    { t: "Z" },
  ];
}
