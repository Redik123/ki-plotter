// Editing commands shared by the toolbar, menus and keyboard shortcuts.

import { askConfirm, backend, pickOpenFile, pickSaveFile, setWindowTitle } from "../backend";
import {
  boundsOf,
  cloneObj,
  deserialize,
  FILE_EXT,
  makePathObj,
  newId,
  objBounds,
  serialize,
  type Obj,
  type PathObj,
  type Store,
  type TextObj,
} from "../model/doc";
import { IDENTITY, mul, rotateAt, scaleAt, splitIntoPieces, transformPath, translate } from "../model/geom";
import type { Imported, Mat } from "../model/types";
import type { CanvasView } from "./canvas";
import { errorMessage, toast } from "./dom";

export class Actions {
  private clipboard: Obj[] = [];

  constructor(
    private store: Store,
    private view: CanvasView,
  ) {
    store.on((k) => {
      if (k === "doc") this.updateTitle();
    });
  }

  updateTitle(): void {
    const name = this.store.filePath?.split(/[\\/]/).pop() ?? "Sans titre";
    void setWindowTitle(`${this.store.dirty ? "• " : ""}${name} — KI Plotter`);
  }

  // ---- files

  private async confirmDiscard(): Promise<boolean> {
    if (!this.store.dirty || this.store.doc.objects.length === 0) return true;
    return askConfirm("Le document a des modifications non enregistrées. Les abandonner ?");
  }

  async newDoc(): Promise<void> {
    if (!(await this.confirmDiscard())) return;
    this.store.load({ version: 1, mediaWidth: this.store.doc.mediaWidth, objects: [] }, null);
    this.view.fit();
    this.updateTitle();
  }

  async open(): Promise<void> {
    if (!(await this.confirmDiscard())) return;
    const path = await pickOpenFile("Ouvrir un projet", [{ name: "Projet KI Plotter", extensions: [FILE_EXT] }]);
    if (!path) return;
    try {
      const doc = deserialize(await backend.readTextFile(path));
      this.store.load(doc, path);
      this.view.fit();
      this.updateTitle();
    } catch (e) {
      toast(`Impossible d'ouvrir le projet : ${errorMessage(e)}`, "error");
    }
  }

  async save(saveAs = false): Promise<void> {
    let path = this.store.filePath;
    if (!path || saveAs) {
      path = await pickSaveFile("Enregistrer le projet", [{ name: "Projet KI Plotter", extensions: [FILE_EXT] }], `projet.${FILE_EXT}`);
      if (!path) return;
      if (!path.toLowerCase().endsWith(`.${FILE_EXT}`)) path += `.${FILE_EXT}`;
    }
    try {
      await backend.writeTextFile(path, serialize(this.store.doc));
      this.store.filePath = path;
      this.store.dirty = false;
      this.updateTitle();
      toast("Projet enregistré.");
    } catch (e) {
      toast(`Impossible d'enregistrer : ${errorMessage(e)}`, "error");
    }
  }

  async importFile(): Promise<void> {
    const path = await pickOpenFile("Importer un dessin", [
      { name: "Dessins vectoriels", extensions: ["svg", "dxf", "plt", "hpgl", "hpg"] },
      { name: "SVG", extensions: ["svg"] },
      { name: "DXF (AutoCAD)", extensions: ["dxf"] },
      { name: "HPGL (SignMaster, etc.)", extensions: ["plt", "hpgl", "hpg"] },
    ]);
    if (!path) return;
    try {
      const imported = await backend.importFile(path);
      const name = path.split(/[\\/]/).pop()?.replace(/\.[^.]+$/, "") ?? "Import";
      this.placeImported(imported, name);
    } catch (e) {
      toast(`Import impossible : ${errorMessage(e)}`, "error");
    }
  }

  /** Free spot on the roll, to the right of what is already there. */
  nextFreeSpot(): [number, number] {
    const b = boundsOf(this.store.doc.objects);
    return b ? [b.x + b.w + 20, 10] : [10, 10];
  }

  /** Adds an import as one object per colour, placed on free roll space. */
  placeImported(imp: Imported, name: string, scale = 1): Obj[] {
    if (imp.items.length === 0) {
      toast("Le fichier ne contient aucun tracé découpable.", "error");
      return [];
    }
    const [x, y] = this.nextFreeSpot();
    const groups = new Map<string, PathObj>();
    for (const it of imp.items) {
      const key = `${it.color ?? ""}|${it.fillRule}`;
      let o = groups.get(key);
      if (!o) {
        const closed = it.path.some((s) => s.t === "Z");
        o = makePathObj(name, [], {
          color: it.color ?? "#2b2f36",
          fill: closed,
          fillRule: it.fillRule,
          transform: mul(translate(x, y), scaleAt(scale, scale, 0, 0)),
        });
        groups.set(key, o);
      }
      o.path = [...o.path, ...it.path];
    }
    const objs = [...groups.values()];
    if (objs.length > 1) objs.forEach((o, i) => (o.name = `${name} (${i + 1})`));
    this.store.checkpoint();
    this.store.add(objs);
    const b = boundsOf(objs);
    if (b) {
      this.view.fit();
      const tooWide = b.h > this.store.doc.mediaWidth;
      toast(
        `Importé : ${fmtMm(b.w)} × ${fmtMm(b.h)} mm` +
          (tooWide ? " — plus large que la laize, pensez à la mosaïque." : ""),
        tooWide ? "error" : "info",
      );
    }
    return objs;
  }

  // ---- selection and clipboard

  selectAll(): void {
    this.store.select(this.store.doc.objects.map((o) => o.id));
  }

  delete(): void {
    if (this.store.selection.length === 0) return;
    this.store.checkpoint();
    this.store.remove(this.store.selection);
  }

  copy(): void {
    this.clipboard = this.store.selected().map(cloneObj);
  }

  cut(): void {
    this.copy();
    this.delete();
  }

  paste(offset = 5): void {
    if (this.clipboard.length === 0) return;
    const objs = this.clipboard.map((o) => ({ ...cloneObj(o), id: newId(), transform: mul(translate(offset, offset), o.transform) }));
    this.clipboard = objs.map(cloneObj);
    this.store.checkpoint();
    this.store.add(objs);
  }

  duplicate(): void {
    this.copy();
    this.paste();
  }

  // ---- transforms

  /** Applies a document-space transform to the selection, as one undo step. */
  transformSelection(m: Mat): void {
    const sel = this.store.selected();
    if (sel.length === 0) return;
    this.store.checkpoint();
    for (const o of sel) o.transform = mul(m, o.transform);
    this.store.changed();
  }

  nudge(dx: number, dy: number): void {
    this.transformSelection(translate(dx, dy));
  }

  mirror(horizontal: boolean): void {
    const b = this.view.selectionBox();
    if (!b) return;
    const cx = b.x + b.w / 2;
    const cy = b.y + b.h / 2;
    this.transformSelection(horizontal ? scaleAt(-1, 1, cx, cy) : scaleAt(1, -1, cx, cy));
  }

  /** Rotates counter-clockwise (as shown on screen) by `deg`. */
  rotate(deg: number): void {
    const b = this.view.selectionBox();
    if (!b) return;
    this.transformSelection(rotateAt((-deg * Math.PI) / 180, b.x + b.w / 2, b.y + b.h / 2));
  }

  align(kind: "left" | "hcenter" | "right" | "top" | "vcenter" | "bottom" | "media"): void {
    const sel = this.store.selected();
    const box = boundsOf(sel);
    if (!box) return;
    this.store.checkpoint();
    if (kind === "media") {
      // Centre the whole selection across the roll width.
      const dy = (this.store.doc.mediaWidth - box.h) / 2 - box.y;
      for (const o of sel) o.transform = mul(translate(0, dy), o.transform);
    } else {
      for (const o of sel) {
        const b = objBounds(o);
        if (!b) continue;
        let dx = 0;
        let dy = 0;
        if (kind === "left") dx = box.x - b.x;
        if (kind === "right") dx = box.x + box.w - (b.x + b.w);
        if (kind === "hcenter") dx = box.x + box.w / 2 - (b.x + b.w / 2);
        if (kind === "top") dy = box.y - b.y;
        if (kind === "bottom") dy = box.y + box.h - (b.y + b.h);
        if (kind === "vcenter") dy = box.y + box.h / 2 - (b.y + b.h / 2);
        o.transform = mul(translate(dx, dy), o.transform);
      }
    }
    this.store.changed();
  }

  // ---- path operations

  /** Bakes transforms and merges the selection into one compound path. */
  combine(): void {
    const sel = this.store.selected();
    if (sel.length < 2) return;
    this.store.checkpoint();
    const path = sel.flatMap((o) => transformPath(o.path, o.transform));
    const merged = makePathObj("Combiné", path, {
      color: sel[0].color,
      fill: sel.some((o) => o.fill),
      fillRule: sel[0].fillRule,
    });
    const first = this.store.doc.objects.findIndex((o) => o.id === sel[0].id);
    this.store.remove(sel.map((o) => o.id));
    this.store.doc.objects.splice(Math.min(first, this.store.doc.objects.length), 0, merged);
    this.store.select([merged.id]);
    this.store.changed();
  }

  /** Splits each selected object into pieces (outer contour + its holes). */
  breakApart(): void {
    const sel = this.store.selected();
    if (sel.length === 0) return;
    this.store.checkpoint();
    const newIds: string[] = [];
    for (const o of sel) {
      const pieces = splitIntoPieces(transformPath(o.path, o.transform));
      if (pieces.length <= 1 && o.kind === "path") {
        newIds.push(o.id);
        continue;
      }
      const objs = pieces.map((p, i) =>
        makePathObj(`${o.name} ${i + 1}`, p, { color: o.color, fill: o.fill, fillRule: o.fillRule, transform: [...IDENTITY] }),
      );
      this.store.replace(o.id, objs);
      newIds.push(...objs.map((x) => x.id));
    }
    this.store.select(newIds);
    this.store.changed();
  }

  /** Text → plain path, so it can be broken apart or welded like any shape. */
  toCurves(): void {
    const texts = this.store.selected().filter((o): o is TextObj => o.kind === "text");
    if (texts.length === 0) return;
    this.store.checkpoint();
    for (const t of texts) {
      const p = makePathObj(t.name === "Texte" ? t.text.slice(0, 24) : t.name, t.path, {
        color: t.color,
        fill: t.fill,
        fillRule: t.fillRule,
        transform: t.transform,
      });
      p.id = t.id;
      this.store.replace(t.id, [p]);
    }
    this.store.changed();
    this.store.emit("selection");
  }
}

export function fmtMm(v: number): string {
  return v.toLocaleString("fr-FR", { maximumFractionDigits: 1 });
}
