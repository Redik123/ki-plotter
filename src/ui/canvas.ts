// The design canvas: media, rulers, objects, selection and direct manipulation.

import { boundsOf, makePathObj, objBounds, type Obj, type Store } from "../model/doc";
import { ellipsePath, mul, rectContains, rectPath, rotateAt, scaleAt, toPath2D, translate } from "../model/geom";
import type { Mat, Rect } from "../model/types";
import { fmtMeters, rolls, type RollCut } from "../model/rolls";
import { settings } from "../settings";

export type Tool = "select" | "text" | "rect" | "ellipse";

const RULER = 22;
const HANDLE = 8;
const ROT_OFFSET = 26;
const ACCENT = "#ff8a3d";

type Drag =
  | { kind: "pan"; sx: number; sy: number; panX: number; panY: number }
  | { kind: "move"; sx: number; sy: number; orig: Map<string, Mat>; moved: boolean }
  | { kind: "scale"; handle: number; box: Rect; orig: Map<string, Mat>; moved: boolean }
  | { kind: "rotate"; cx: number; cy: number; a0: number; orig: Map<string, Mat>; moved: boolean }
  | { kind: "marquee"; x0: number; y0: number; x1: number; y1: number; add: boolean }
  | { kind: "create"; tool: "rect" | "ellipse"; x0: number; y0: number; x1: number; y1: number };

export interface CanvasHooks {
  onCreateText(x: number, y: number): void;
  onEditText(o: Obj): void;
  onCursor(x: number, y: number): void;
  onToolChange(t: Tool): void;
  onDragInfo(text: string | null): void;
}

/** Overlay drawn in document coordinates on top of the objects (tiling grid, etc.). */
export type Overlay = (ctx: CanvasRenderingContext2D, pxPerMm: number) => void;

interface DocPathCache {
  path: unknown;
  transform: Mat;
  p2d: Path2D;
}

export class CanvasView {
  zoom = 1.2;
  panX = RULER + 40;
  panY = RULER + 40;
  tool: Tool = "select";
  overlay: Overlay | null = null;

  private ctx: CanvasRenderingContext2D;
  private dpr = 1;
  private drag: Drag | null = null;
  private hover: string | null = null;
  private spaceDown = false;
  private pending = false;
  private docPaths = new WeakMap<Obj, DocPathCache>();

  constructor(
    private store: Store,
    private canvas: HTMLCanvasElement,
    private hooks: CanvasHooks,
  ) {
    this.ctx = canvas.getContext("2d")!;
    new ResizeObserver(() => this.resize()).observe(canvas.parentElement!);
    canvas.addEventListener("pointerdown", (e) => this.down(e));
    canvas.addEventListener("pointermove", (e) => this.move(e));
    canvas.addEventListener("pointerup", (e) => this.up(e));
    canvas.addEventListener("dblclick", (e) => this.dblclick(e));
    canvas.addEventListener("wheel", (e) => this.wheel(e), { passive: false });
    canvas.addEventListener("pointerleave", () => {
      this.hover = null;
      this.render();
    });
    window.addEventListener("keydown", (e) => {
      if (e.code === "Space" && !isTyping(e)) {
        this.spaceDown = true;
        this.canvas.style.cursor = "grab";
        e.preventDefault();
      }
    });
    window.addEventListener("keyup", (e) => {
      if (e.code === "Space") {
        this.spaceDown = false;
        this.updateCursor(null);
      }
    });
    store.on(() => this.render());
    rolls.on(() => this.render());
    this.resize();
  }

  setTool(t: Tool): void {
    this.tool = t;
    this.canvas.style.cursor = t === "select" ? "default" : "crosshair";
    this.hooks.onToolChange(t);
  }

  // ---- coordinates

  toDoc(sx: number, sy: number): [number, number] {
    return [(sx - this.panX) / this.zoom, (sy - this.panY) / this.zoom];
  }

  toScreen(x: number, y: number): [number, number] {
    return [x * this.zoom + this.panX, y * this.zoom + this.panY];
  }

  private local(e: MouseEvent): [number, number] {
    const r = this.canvas.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top];
  }

  get mediaLength(): number {
    // With a roll in place, the strip is the vinyl that is left.
    const roll = rolls.current();
    if (roll) return Math.max(1, rolls.remaining(roll));
    const b = boundsOf(this.store.doc.objects);
    return Math.max(1000, b ? b.x + b.w + 300 : 0);
  }

  /** Zooms to show a rect (document mm), or the whole media width and the design. */
  fit(rect?: Rect | null): void {
    const w = this.canvas.clientWidth - RULER - 60;
    const h = this.canvas.clientHeight - RULER - 60;
    const b = boundsOf(this.store.doc.objects);
    // Show a little of the roll's history too, left of the free part.
    const roll = rolls.current();
    const x0 = roll ? -Math.min(roll.used, 300) : 0;
    const target = rect ?? {
      x: x0,
      y: 0,
      w: Math.max(400, b ? b.x + b.w + 20 : 0) - x0,
      h: this.store.doc.mediaWidth,
    };
    if (target.w <= 0 || target.h <= 0 || w <= 0 || h <= 0) return;
    this.zoom = Math.min(w / target.w, h / target.h, 40);
    // The whole roll view starts at its left edge (it runs to the right); a given rect is centred.
    const slackX = rect ? (w - target.w * this.zoom) / 2 : 0;
    this.panX = RULER + 30 + slackX - target.x * this.zoom;
    this.panY = RULER + 30 + (h - target.h * this.zoom) / 2 - target.y * this.zoom;
    this.render();
  }

  zoomBy(f: number, sx = this.canvas.clientWidth / 2, sy = this.canvas.clientHeight / 2): void {
    const z = Math.min(80, Math.max(0.05, this.zoom * f));
    const [x, y] = this.toDoc(sx, sy);
    this.zoom = z;
    this.panX = sx - x * z;
    this.panY = sy - y * z;
    this.render();
  }

  private resize(): void {
    const parent = this.canvas.parentElement!;
    this.dpr = window.devicePixelRatio || 1;
    this.canvas.width = Math.round(parent.clientWidth * this.dpr);
    this.canvas.height = Math.round(parent.clientHeight * this.dpr);
    this.canvas.style.width = `${parent.clientWidth}px`;
    this.canvas.style.height = `${parent.clientHeight}px`;
    this.paint();
  }

  // ---- rendering

  render(): void {
    if (this.pending) return;
    this.pending = true;
    requestAnimationFrame(() => {
      this.pending = false;
      this.paint();
    });
  }

  private docPath(o: Obj): Path2D {
    const c = this.docPaths.get(o);
    if (c && c.path === o.path && c.transform === o.transform) return c.p2d;
    const p2d = new Path2D();
    const [a, b, cc, d, e, f] = o.transform;
    p2d.addPath(toPath2D(o.path), new DOMMatrix([a, b, cc, d, e, f]));
    this.docPaths.set(o, { path: o.path, transform: o.transform, p2d });
    return p2d;
  }

  private viewTransform(): void {
    const k = this.dpr;
    this.ctx.setTransform(k * this.zoom, 0, 0, k * this.zoom, k * this.panX, k * this.panY);
  }

  private paint(): void {
    const { ctx, dpr } = this;
    const W = this.canvas.clientWidth;
    const H = this.canvas.clientHeight;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.fillStyle = "#15181e";
    ctx.fillRect(0, 0, W, H);

    const media = this.store.doc.mediaWidth;
    const len = this.mediaLength;
    const px = 1 / this.zoom;

    // Media strip.
    this.viewTransform();
    ctx.shadowColor = "rgba(0,0,0,0.45)";
    ctx.shadowBlur = 18;
    ctx.fillStyle = "#ffffff";
    ctx.fillRect(0, 0, len, media);
    ctx.shadowBlur = 0;
    if (settings.view.grid) this.paintGrid(len, media);
    ctx.strokeStyle = "#9aa3b2";
    ctx.lineWidth = px;
    ctx.strokeRect(0, 0, len, media);
    this.paintRoll(len, media);

    // Objects.
    const wire = settings.view.wireframe;
    for (const o of this.store.doc.objects) {
      const p = this.docPath(o);
      const sel = this.store.isSelected(o.id);
      if (o.fill && !wire) {
        ctx.fillStyle = o.color;
        ctx.fill(p, o.fillRule);
      }
      ctx.lineWidth = (sel || this.hover === o.id ? 1.5 : 1) * px;
      ctx.strokeStyle = sel ? ACCENT : this.hover === o.id ? "#ffb27a" : o.fill && !wire ? "rgba(0,0,0,0.55)" : o.color;
      ctx.stroke(p);
    }

    this.overlay?.(ctx, this.zoom);

    // Drag previews.
    const d = this.drag;
    if (d?.kind === "marquee" || d?.kind === "create") {
      const x = Math.min(d.x0, d.x1);
      const y = Math.min(d.y0, d.y1);
      const w = Math.abs(d.x1 - d.x0);
      const h = Math.abs(d.y1 - d.y0);
      ctx.lineWidth = px;
      ctx.setLineDash([4 * px, 3 * px]);
      ctx.strokeStyle = ACCENT;
      if (d.kind === "create") {
        ctx.setLineDash([]);
        ctx.stroke(toPath2D(d.tool === "rect" ? rectPath(x, y, w, h) : ellipsePath(x, y, w, h)));
      } else {
        ctx.fillStyle = "rgba(255,138,61,0.08)";
        ctx.fillRect(x, y, w, h);
        ctx.strokeRect(x, y, w, h);
      }
      ctx.setLineDash([]);
    }

    this.paintSelection();
    this.paintRulers(W, H);
  }

  private cutPaths = new Map<string, Path2D>();

  private cutPath(c: RollCut): Path2D {
    let p = this.cutPaths.get(c.id);
    if (!p) {
      p = new Path2D();
      for (const line of c.lines) {
        line.forEach(([x, y], i) => (i ? p!.lineTo(x, y) : p!.moveTo(x, y)));
      }
      this.cutPaths.set(c.id, p);
    }
    return p;
  }

  /** The used part of the roll (old cuts, in grey) left of x = 0, and the roll's end. */
  private paintRoll(len: number, media: number): void {
    const roll = rolls.current();
    if (!roll) return;
    const { ctx, dpr } = this;
    const px = 1 / this.zoom;
    const used = roll.used;
    const [vx0] = this.toDoc(RULER, 0);
    const [vx1] = this.toDoc(this.canvas.clientWidth, 0);

    if (used > 0) {
      ctx.fillStyle = "#e4e7ec";
      ctx.fillRect(-used, 0, used, media);
      // Old cuts, in roll coordinates shifted so the free part starts at 0.
      ctx.save();
      ctx.translate(-used, 0);
      ctx.lineWidth = px;
      for (const c of roll.cuts) {
        const x = c.x - used;
        if (x > vx1 || x + c.length < vx0) continue;
        ctx.strokeStyle = "#9aa2b0";
        ctx.stroke(this.cutPath(c));
      }
      ctx.restore();
      ctx.setLineDash([3 * px, 4 * px]);
      ctx.strokeStyle = "#c2c8d2";
      ctx.beginPath();
      for (const c of roll.cuts) {
        const x = c.x - used;
        if (x > vx1 || x < vx0) continue;
        ctx.moveTo(x, 0);
        ctx.lineTo(x, media);
      }
      ctx.stroke();
      ctx.setLineDash([]);
    }

    // Start of the free vinyl (where the blade should be) and the roll's end.
    ctx.lineWidth = 2 * px;
    ctx.strokeStyle = ACCENT;
    ctx.setLineDash([8 * px, 5 * px]);
    ctx.beginPath();
    ctx.moveTo(0, -6 * px);
    ctx.lineTo(0, media + 6 * px);
    ctx.stroke();
    ctx.setLineDash([]);
    ctx.strokeStyle = "#e5484d";
    ctx.beginPath();
    ctx.moveTo(len, -6 * px);
    ctx.lineTo(len, media + 6 * px);
    ctx.stroke();

    // Labels, in screen space.
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.font = "11px system-ui, sans-serif";
    ctx.textBaseline = "top";
    const [, top] = this.toScreen(0, 0);
    const [, bottom] = this.toScreen(0, media);
    for (const c of roll.cuts) {
      const [sx] = this.toScreen(c.x - used, 0);
      const w = c.length * this.zoom;
      if (w < 50 || sx > this.canvas.clientWidth || sx + w < RULER) continue;
      ctx.fillStyle = "#6f7787";
      const date = new Date(c.date).toLocaleDateString("fr-FR", { day: "numeric", month: "short" });
      ctx.fillText(fitText(ctx, `${c.name} · ${date}`, w - 8), sx + 4, top + 4);
    }
    const [s0] = this.toScreen(0, 0);
    const [sEnd] = this.toScreen(len, 0);
    ctx.fillStyle = "#b4561c";
    ctx.textBaseline = "bottom";
    ctx.fillText(`${roll.name} : début du vinyle libre`, s0 + 5, top - 2);
    ctx.fillStyle = "#b0353a";
    const end = `Fin ≈ (reste ${fmtMeters(len)})`;
    ctx.fillText(end, sEnd - ctx.measureText(end).width - 5, top - 2);
    if (used > 0) {
      ctx.fillStyle = "#8a93a3";
      ctx.textBaseline = "top";
      ctx.fillText(`Déjà utilisé : ${fmtMeters(used)}`, Math.max(RULER + 4, s0 - 150), bottom + 4);
    }
    this.viewTransform();
  }

  private paintGrid(len: number, media: number): void {
    const { ctx } = this;
    const px = 1 / this.zoom;
    const step = this.zoom * 10 >= 6 ? 10 : this.zoom * 50 >= 6 ? 50 : 100;
    ctx.lineWidth = px;
    for (let pass = 0; pass < 2; pass++) {
      ctx.beginPath();
      ctx.strokeStyle = pass === 0 ? "#eef0f4" : "#dce0e8";
      for (let x = step; x < len; x += step) {
        if ((x % 100 === 0) === (pass === 1)) {
          ctx.moveTo(x, 0);
          ctx.lineTo(x, media);
        }
      }
      for (let y = step; y < media; y += step) {
        if ((y % 100 === 0) === (pass === 1)) {
          ctx.moveTo(0, y);
          ctx.lineTo(len, y);
        }
      }
      ctx.stroke();
    }
  }

  selectionBox(): Rect | null {
    return boundsOf(this.store.selected());
  }

  /** Handle positions in screen space: 0..7 clockwise from top-left, 8 = rotation. */
  private handles(box: Rect): [number, number][] {
    const [x0, y0] = this.toScreen(box.x, box.y);
    const [x1, y1] = this.toScreen(box.x + box.w, box.y + box.h);
    const mx = (x0 + x1) / 2;
    const my = (y0 + y1) / 2;
    return [
      [x0, y0],
      [mx, y0],
      [x1, y0],
      [x1, my],
      [x1, y1],
      [mx, y1],
      [x0, y1],
      [x0, my],
      [mx, y0 - ROT_OFFSET],
    ];
  }

  private paintSelection(): void {
    const box = this.selectionBox();
    if (!box) return;
    const { ctx, dpr } = this;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const hs = this.handles(box);
    const [x0, y0] = hs[0];
    const [x1, y1] = hs[4];
    ctx.strokeStyle = ACCENT;
    ctx.lineWidth = 1;
    ctx.setLineDash([5, 4]);
    ctx.strokeRect(Math.round(x0) + 0.5, Math.round(y0) + 0.5, Math.round(x1 - x0), Math.round(y1 - y0));
    ctx.setLineDash([]);
    ctx.beginPath();
    ctx.moveTo(hs[1][0], y0);
    ctx.lineTo(hs[8][0], hs[8][1]);
    ctx.stroke();
    for (let i = 0; i < 8; i++) {
      const [hx, hy] = hs[i];
      ctx.fillStyle = "#ffffff";
      ctx.fillRect(hx - HANDLE / 2, hy - HANDLE / 2, HANDLE, HANDLE);
      ctx.strokeRect(hx - HANDLE / 2 + 0.5, hy - HANDLE / 2 + 0.5, HANDLE - 1, HANDLE - 1);
    }
    ctx.beginPath();
    ctx.arc(hs[8][0], hs[8][1], 5, 0, Math.PI * 2);
    ctx.fillStyle = ACCENT;
    ctx.fill();
  }

  private paintRulers(W: number, H: number): void {
    const { ctx, dpr } = this;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.fillStyle = "#1d2129";
    ctx.fillRect(0, 0, W, RULER);
    ctx.fillRect(0, 0, RULER, H);
    ctx.strokeStyle = "#2e3440";
    ctx.beginPath();
    ctx.moveTo(0, RULER + 0.5);
    ctx.lineTo(W, RULER + 0.5);
    ctx.moveTo(RULER + 0.5, 0);
    ctx.lineTo(RULER + 0.5, H);
    ctx.stroke();

    // Pick a labelled step that leaves ~60px between labels.
    const steps = [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000];
    const major = steps.find((s) => s * this.zoom >= 60) ?? 10000;
    const minor = major / (String(major).startsWith("2") ? 4 : 5);
    ctx.font = "10px system-ui, sans-serif";
    ctx.fillStyle = "#8a93a3";
    ctx.strokeStyle = "#566070";
    ctx.textBaseline = "top";

    const [dx0] = this.toDoc(RULER, 0);
    const [dx1] = this.toDoc(W, 0);
    ctx.beginPath();
    for (let v = Math.floor(dx0 / minor) * minor; v <= dx1; v += minor) {
      const [sx] = this.toScreen(v, 0);
      if (sx < RULER) continue;
      const isMajor = Math.abs(v / major - Math.round(v / major)) < 1e-6;
      ctx.moveTo(Math.round(sx) + 0.5, RULER);
      ctx.lineTo(Math.round(sx) + 0.5, isMajor ? 4 : RULER - 5);
      if (isMajor) ctx.fillText(String(Math.round(v)), sx + 3, 3);
    }
    const [, dy0] = this.toDoc(0, RULER);
    const [, dy1] = this.toDoc(0, H);
    for (let v = Math.floor(dy0 / minor) * minor; v <= dy1; v += minor) {
      const [, sy] = this.toScreen(0, v);
      if (sy < RULER) continue;
      const isMajor = Math.abs(v / major - Math.round(v / major)) < 1e-6;
      ctx.moveTo(RULER, Math.round(sy) + 0.5);
      ctx.lineTo(isMajor ? 4 : RULER - 5, Math.round(sy) + 0.5);
      if (isMajor) {
        ctx.save();
        ctx.translate(3, sy + 3);
        ctx.rotate(Math.PI / 2);
        ctx.fillText(String(Math.round(v)), 0, -RULER + 6);
        ctx.restore();
      }
    }
    ctx.stroke();
    ctx.fillStyle = "#1d2129";
    ctx.fillRect(0, 0, RULER, RULER);
    ctx.fillStyle = "#8a93a3";
    ctx.fillText("mm", 3, 6);
  }

  // ---- hit testing

  private hitObject(sx: number, sy: number): Obj | null {
    const [x, y] = this.toDoc(sx, sy);
    const tol = 5 / this.zoom;
    const { ctx, dpr } = this;
    const objs = this.store.doc.objects;
    for (let i = objs.length - 1; i >= 0; i--) {
      const o = objs[i];
      const b = objBounds(o);
      if (!b || x < b.x - tol || x > b.x + b.w + tol || y < b.y - tol || y > b.y + b.h + tol) continue;
      const p = this.docPath(o);
      this.viewTransform();
      ctx.lineWidth = tol * 2;
      if (o.fill && !settings.view.wireframe && ctx.isPointInPath(p, sx * dpr, sy * dpr, o.fillRule)) return o;
      if (ctx.isPointInStroke(p, sx * dpr, sy * dpr)) return o;
    }
    return null;
  }

  private hitHandle(sx: number, sy: number): number {
    const box = this.selectionBox();
    if (!box) return -1;
    const hs = this.handles(box);
    for (let i = 8; i >= 0; i--) {
      if (Math.abs(hs[i][0] - sx) <= HANDLE && Math.abs(hs[i][1] - sy) <= HANDLE) return i;
    }
    return -1;
  }

  private updateCursor(e: PointerEvent | null): void {
    if (this.spaceDown) {
      this.canvas.style.cursor = this.drag?.kind === "pan" ? "grabbing" : "grab";
      return;
    }
    if (this.tool !== "select") {
      this.canvas.style.cursor = this.tool === "text" ? "text" : "crosshair";
      return;
    }
    if (!e) {
      this.canvas.style.cursor = "default";
      return;
    }
    const [sx, sy] = this.local(e);
    const h = this.hitHandle(sx, sy);
    const cursors = ["nwse-resize", "ns-resize", "nesw-resize", "ew-resize", "nwse-resize", "ns-resize", "nesw-resize", "ew-resize", "grab"];
    this.canvas.style.cursor = h >= 0 ? cursors[h] : this.hover ? "move" : "default";
  }

  // ---- pointer handling

  private origTransforms(): Map<string, Mat> {
    return new Map(this.store.selected().map((o) => [o.id, o.transform]));
  }

  private down(e: PointerEvent): void {
    // The canvas can't take focus; release the panel's inputs so shortcuts work again.
    (document.activeElement as HTMLElement | null)?.blur?.();
    this.canvas.setPointerCapture(e.pointerId);
    const [sx, sy] = this.local(e);
    const [x, y] = this.toDoc(sx, sy);
    if (e.button === 1 || this.spaceDown) {
      this.drag = { kind: "pan", sx, sy, panX: this.panX, panY: this.panY };
      this.updateCursor(e);
      return;
    }
    if (e.button !== 0) return;
    if (this.tool === "text") {
      this.hooks.onCreateText(x, y);
      this.setTool("select");
      return;
    }
    if (this.tool === "rect" || this.tool === "ellipse") {
      this.drag = { kind: "create", tool: this.tool, x0: x, y0: y, x1: x, y1: y };
      return;
    }
    const h = this.hitHandle(sx, sy);
    if (h >= 0) {
      const box = this.selectionBox()!;
      if (h === 8) {
        const cx = box.x + box.w / 2;
        const cy = box.y + box.h / 2;
        this.drag = { kind: "rotate", cx, cy, a0: Math.atan2(y - cy, x - cx), orig: this.origTransforms(), moved: false };
      } else {
        this.drag = { kind: "scale", handle: h, box, orig: this.origTransforms(), moved: false };
      }
      return;
    }
    const hit = this.hitObject(sx, sy);
    if (hit) {
      if (e.shiftKey) {
        const sel = this.store.isSelected(hit.id)
          ? this.store.selection.filter((id) => id !== hit.id)
          : [...this.store.selection, hit.id];
        this.store.select(sel);
      } else if (!this.store.isSelected(hit.id)) {
        this.store.select([hit.id]);
      }
      this.drag = { kind: "move", sx: x, sy: y, orig: this.origTransforms(), moved: false };
      return;
    }
    if (!e.shiftKey) this.store.select([]);
    this.drag = { kind: "marquee", x0: x, y0: y, x1: x, y1: y, add: e.shiftKey };
  }

  /** Applies `m` (document space) on top of each object's transform at drag start. */
  private applyToSelection(orig: Map<string, Mat>, m: Mat): void {
    for (const o of this.store.selected()) {
      const t = orig.get(o.id);
      if (t) o.transform = mul(m, t);
    }
    this.store.emit("doc");
  }

  private beginChange(d: { moved: boolean }): void {
    if (d.moved) return;
    d.moved = true;
    // Objects still hold their starting transforms here, so this snapshot is the "before".
    this.store.checkpoint();
  }

  private move(e: PointerEvent): void {
    const [sx, sy] = this.local(e);
    const [x, y] = this.toDoc(sx, sy);
    this.hooks.onCursor(x, y);
    const d = this.drag;
    if (!d) {
      const hit = this.tool === "select" ? this.hitObject(sx, sy) : null;
      const id = hit?.id ?? null;
      if (id !== this.hover) {
        this.hover = id;
        this.render();
      }
      this.updateCursor(e);
      return;
    }
    switch (d.kind) {
      case "pan":
        this.panX = d.panX + sx - d.sx;
        this.panY = d.panY + sy - d.sy;
        this.render();
        break;
      case "move": {
        let dx = x - d.sx;
        let dy = y - d.sy;
        if (!d.moved && Math.hypot(dx, dy) * this.zoom < 3) return;
        if (e.shiftKey) {
          if (Math.abs(dx) > Math.abs(dy)) dy = 0;
          else dx = 0;
        }
        this.beginChange(d);
        this.applyToSelection(d.orig, translate(dx, dy));
        this.hooks.onDragInfo(`Déplacement ${fmt(dx)} ; ${fmt(dy)} mm`);
        break;
      }
      case "scale": {
        this.beginChange(d);
        const { box, handle } = d;
        const hx = [0, 0.5, 1, 1, 1, 0.5, 0, 0][handle];
        const hy = [0, 0, 0, 0.5, 1, 1, 1, 0.5][handle];
        const center = e.altKey;
        const ax = center ? box.x + box.w / 2 : box.x + box.w * (1 - hx);
        const ay = center ? box.y + box.h / 2 : box.y + box.h * (1 - hy);
        const px = box.x + box.w * hx;
        const py = box.y + box.h * hy;
        let fx = hx === 0.5 || Math.abs(px - ax) < 1e-9 ? 1 : (x - ax) / (px - ax);
        let fy = hy === 0.5 || Math.abs(py - ay) < 1e-9 ? 1 : (y - ay) / (py - ay);
        const corner = hx !== 0.5 && hy !== 0.5;
        if (corner && !e.shiftKey) {
          const f = Math.abs(fx) > Math.abs(fy) ? fx : fy;
          fx = f;
          fy = f;
        }
        fx = clampScale(fx);
        fy = clampScale(fy);
        this.applyToSelection(d.orig, scaleAt(fx, fy, ax, ay));
        this.hooks.onDragInfo(`${fmt(Math.abs(box.w * fx))} × ${fmt(Math.abs(box.h * fy))} mm`);
        break;
      }
      case "rotate": {
        this.beginChange(d);
        let a = Math.atan2(y - d.cy, x - d.cx) - d.a0;
        if (e.shiftKey) a = Math.round(a / (Math.PI / 12)) * (Math.PI / 12);
        this.applyToSelection(d.orig, rotateAt(a, d.cx, d.cy));
        this.hooks.onDragInfo(`Rotation ${fmt((-a * 180) / Math.PI)}°`);
        break;
      }
      case "marquee":
      case "create":
        d.x1 = x;
        d.y1 = y;
        if (d.kind === "create") this.hooks.onDragInfo(`${fmt(Math.abs(d.x1 - d.x0))} × ${fmt(Math.abs(d.y1 - d.y0))} mm`);
        this.render();
        break;
    }
  }

  private up(e: PointerEvent): void {
    const d = this.drag;
    this.drag = null;
    this.hooks.onDragInfo(null);
    if (!d) return;
    if (d.kind === "marquee") {
      const r = { x: Math.min(d.x0, d.x1), y: Math.min(d.y0, d.y1), w: Math.abs(d.x1 - d.x0), h: Math.abs(d.y1 - d.y0) };
      if (r.w * this.zoom > 2 || r.h * this.zoom > 2) {
        const inside = this.store.doc.objects.filter((o) => {
          const b = objBounds(o);
          return b && rectContains(r, b);
        });
        const ids = new Set(d.add ? this.store.selection : []);
        for (const o of inside) ids.add(o.id);
        this.store.select([...ids]);
      }
    } else if (d.kind === "create") {
      const x = Math.min(d.x0, d.x1);
      const y = Math.min(d.y0, d.y1);
      const w = Math.abs(d.x1 - d.x0);
      const h = Math.abs(d.y1 - d.y0);
      if (w * this.zoom > 3 && h * this.zoom > 3) {
        this.store.checkpoint();
        const name = d.tool === "rect" ? "Rectangle" : "Ellipse";
        const path = d.tool === "rect" ? rectPath(0, 0, w, h) : ellipsePath(0, 0, w, h);
        this.store.add([makePathObj(name, path, { transform: translate(x, y), fill: false })]);
      }
      this.setTool("select");
    } else if ((d.kind === "move" || d.kind === "scale" || d.kind === "rotate") && d.moved) {
      this.store.changed();
    }
    this.render();
    this.updateCursor(e);
  }

  private dblclick(e: MouseEvent): void {
    const [sx, sy] = this.local(e);
    const hit = this.hitObject(sx, sy);
    if (hit?.kind === "text") this.hooks.onEditText(hit);
  }

  private wheel(e: WheelEvent): void {
    e.preventDefault();
    const [sx, sy] = this.local(e);
    if (e.shiftKey) {
      this.panX -= e.deltaY || e.deltaX;
      this.render();
      return;
    }
    this.zoomBy(Math.exp(-e.deltaY * 0.0015), sx, sy);
  }
}

/** Shortens a label with "…" to fit `max` pixels. */
function fitText(ctx: CanvasRenderingContext2D, text: string, max: number): string {
  if (ctx.measureText(text).width <= max) return text;
  let t = text;
  while (t.length > 1 && ctx.measureText(`${t}…`).width > max) t = t.slice(0, -1);
  return `${t}…`;
}

function clampScale(f: number): number {
  if (!Number.isFinite(f)) return 1;
  if (Math.abs(f) < 0.01) return f < 0 ? -0.01 : 0.01;
  return f;
}

export function fmt(v: number, digits = 1): string {
  return v.toLocaleString("fr-FR", { minimumFractionDigits: 0, maximumFractionDigits: digits });
}

export function isTyping(e: KeyboardEvent): boolean {
  const t = e.target as HTMLElement | null;
  return !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable);
}
