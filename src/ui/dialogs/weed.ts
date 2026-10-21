// "Échenillage" (weeding) and "Mosaïque" (tiling) dialogs.

import { backend } from "../../backend";
import { boundsOf, makePathObj, toShape, type Obj, type Store } from "../../model/doc";
import { translate } from "../../model/geom";
import type { Rect } from "../../model/types";
import type { CanvasView } from "../canvas";
import { checkbox, errorMessage, h, modal, numberField, toast } from "../dom";

const WEED_COLOR = "#8a93a3";

/** Objects the operation applies to: the selection, or everything. */
function targets(store: Store): Obj[] {
  const sel = store.selected();
  return sel.length ? sel : store.doc.objects;
}

const weedOpts = { margin: 5, box: true, horizontal: 1, vertical: 0, gap: 0.5 };

export function openWeedDialog(store: Store): void {
  const objs = targets(store);
  if (objs.length === 0) {
    toast("Rien à écheniller : le document est vide.", "error");
    return;
  }
  const o = weedOpts;
  const n = (label: string, key: "margin" | "horizontal" | "vertical" | "gap", unit: string, step: number, title?: string) =>
    numberField({
      label,
      value: o[key],
      step,
      min: 0,
      unit,
      title,
      onCommit: (v) => (o[key] = key === "horizontal" || key === "vertical" ? Math.min(50, Math.max(0, Math.round(v))) : Math.max(0, v)),
    }).row;

  const apply = async () => {
    try {
      const path = await backend.makeWeeding({ shapes: objs.map(toShape), ...o });
      if (path.length === 0) {
        toast("Aucune ligne d'échenillage à ajouter.", "error");
        return;
      }
      store.checkpoint();
      // Weeding lines go first so they are drawn under the design.
      const weed = makePathObj("Échenillage", path, { color: WEED_COLOR, fill: false });
      store.doc.objects.unshift(weed);
      store.select([...objs.map((x) => x.id), weed.id]);
      store.changed();
      dlg.close();
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };

  const body = h(
    "div",
    { class: "form" },
    h("p", { class: "hint" }, `${objs.length} objet${objs.length > 1 ? "s" : ""} ${store.selection.length ? "sélectionné(s)" : "(tout le document)"}.`),
    checkbox("Cadre autour du dessin", o.box, (v) => (o.box = v)),
    n("Marge du cadre", "margin", "mm", 1),
    n("Lignes horizontales", "horizontal", "", 1, "Lignes qui traversent le cadre entre les lettres"),
    n("Lignes verticales", "vertical", "", 1),
    n("Écart aux lettres", "gap", "mm", 0.5, "Les lignes s'arrêtent à cette distance des formes"),
  );
  const footer = h(
    "div",
    { class: "footer-row" },
    h("span", { class: "spacer" }),
    h("button", { onclick: () => dlg.close() }, "Annuler"),
    h("button", { class: "primary", onclick: () => void apply() }, "Ajouter"),
  );
  const dlg = modal("Échenillage", body, footer);
}

const tileOpts = { tileWidth: 0, tileHeight: 0, overlap: 10, frame: true, spacing: 20, replace: true };

export function openTileDialog(store: Store, view: CanvasView): void {
  const objs = targets(store);
  const box = boundsOf(objs);
  if (!box) {
    toast("Rien à découper en panneaux.", "error");
    return;
  }
  const o = tileOpts;
  // Default: panels as wide as the roll allows (with a margin), long sheets of 1 m.
  if (!o.tileHeight) o.tileHeight = Math.max(50, store.doc.mediaWidth - 20);
  if (!o.tileWidth) o.tileWidth = 1000;

  const info = h("p", { class: "hint" });
  const grid = (): { cols: number; rows: number } => ({
    cols: o.tileWidth > 0 ? Math.max(1, Math.ceil(box.w / o.tileWidth)) : 1,
    rows: o.tileHeight > 0 ? Math.max(1, Math.ceil(box.h / o.tileHeight)) : 1,
  });
  const update = () => {
    const { cols, rows } = grid();
    info.textContent = `Dessin ${Math.round(box.w)} × ${Math.round(box.h)} mm → ${cols * rows} panneau${cols * rows > 1 ? "x" : ""} (${cols} × ${rows}).`;
    view.render();
  };

  // Grid preview on the main canvas while the dialog is open.
  view.overlay = (ctx, zoom) => {
    const { cols, rows } = grid();
    ctx.lineWidth = 1.5 / zoom;
    ctx.strokeStyle = "#ff8a3d";
    ctx.setLineDash([8 / zoom, 5 / zoom]);
    for (let c = 0; c < cols; c++) {
      for (let r = 0; r < rows; r++) {
        const rect = cellRect(box, c, r, cols, rows);
        ctx.strokeRect(rect.x, rect.y, rect.w, rect.h);
      }
    }
    ctx.setLineDash([]);
  };

  const n = (label: string, key: "tileWidth" | "tileHeight" | "overlap" | "spacing", title?: string) =>
    numberField({
      label,
      value: o[key],
      step: 5,
      min: 0,
      unit: "mm",
      title,
      onCommit: (v) => {
        o[key] = Math.max(0, v);
        update();
      },
    }).row;

  function cellRect(b: Rect, c: number, r: number, cols: number, rows: number): Rect {
    const tw = o.tileWidth > 0 ? o.tileWidth : b.w;
    const th = o.tileHeight > 0 ? o.tileHeight : b.h;
    const x = b.x + c * tw;
    const y = b.y + r * th;
    const w = Math.min(tw + (c < cols - 1 ? o.overlap : 0), b.x + b.w - x);
    const hh = Math.min(th + (r < rows - 1 ? o.overlap : 0), b.y + b.h - y);
    return { x, y, w, h: hh };
  }

  const apply = async () => {
    try {
      const tiles = await backend.makeTiles({
        shapes: objs.map(toShape),
        tileWidth: o.tileWidth,
        tileHeight: o.tileHeight,
        overlap: o.overlap,
        frame: o.frame,
      });
      if (tiles.length === 0) {
        toast("Aucun panneau produit.", "error");
        return;
      }
      store.checkpoint();
      if (o.replace) store.remove(objs.map((x) => x.id));
      // Lay the panels out one after another along the roll, top-aligned.
      const start = boundsOf(store.doc.objects);
      let cursor = start ? start.x + start.w + o.spacing : 10;
      const created: Obj[] = [];
      const ordered = [...tiles].sort((a, b) => a.row - b.row || a.col - b.col);
      for (const t of ordered) {
        const name = `Panneau ${String.fromCharCode(65 + (t.row % 26))}${t.col + 1}`;
        const obj = makePathObj(name, t.path, {
          fill: false,
          color: objs[0].color,
          transform: translate(cursor - t.rect.x, 10 - t.rect.y),
        });
        created.push(obj);
        cursor += t.rect.w + o.spacing;
      }
      store.add(created);
      view.fit();
      toast(`${created.length} panneaux créés, rangés le long du rouleau.`);
      dlg.close();
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };

  const body = h(
    "div",
    { class: "form" },
    info,
    n("Longueur max.", "tileWidth", "Le long du rouleau (0 = pas de coupure)"),
    n("Hauteur max.", "tileHeight", "En travers du rouleau, au plus la laize (0 = pas de coupure)"),
    n("Recouvrement", "overlap", "Les panneaux se chevauchent de cette longueur pour la pose"),
    checkbox("Cadre autour de chaque panneau", o.frame, (v) => (o.frame = v)),
    checkbox("Remplacer le dessin par les panneaux", o.replace, (v) => (o.replace = v)),
    n("Espace entre panneaux", "spacing", "Écart entre les panneaux rangés sur le rouleau"),
  );
  const footer = h(
    "div",
    { class: "footer-row" },
    h("span", { class: "spacer" }),
    h("button", { onclick: () => dlg.close() }, "Annuler"),
    h("button", { class: "primary", onclick: () => void apply() }, "Créer les panneaux"),
  );
  const dlg = modal("Mosaïque", body, footer);
  dlg.addEventListener("close", () => {
    view.overlay = null;
    view.render();
  });
  update();
}
