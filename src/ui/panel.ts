// Right-hand panel: document, transform, text and style properties, object list.

import { boundsOf, type Obj, type Store, type TextObj } from "../model/doc";
import { angleOf, scaleAt, translate } from "../model/geom";
import type { Align } from "../model/types";
import type { Actions } from "./actions";
import type { CanvasView } from "./canvas";
import { fmtMeters, rolls } from "../model/rolls";
import { openRollDialog } from "./dialogs/roll";
import { checkbox, h, numberField, round, select } from "./dom";
import { families, fontById, fontName, refreshText, regularOf, rememberFont, stylesOf } from "./text";

const MEDIA_PRESETS: [string, string][] = [
  ["720", "720 mm (KI-870A)"],
  ["610", "610 mm (24 pouces)"],
  ["500", "500 mm"],
  ["300", "300 mm (12 pouces)"],
  ["custom", "Autre…"],
];

const SWATCHES = ["#2b2f36", "#ffffff", "#d7263d", "#f6c90e", "#1b6ec2", "#2a9d4b", "#c9a227", "#a7adb7", "#f28c28", "#7b3fa0"];

export class SidePanel {
  private lockRatio = true;
  private transformInputs: Record<"x" | "y" | "w" | "h" | "rot", HTMLInputElement> | null = null;
  private textArea: HTMLTextAreaElement | null = null;
  private textEditing = false;
  private textTimer = 0;
  private list: HTMLElement | null = null;

  constructor(
    private store: Store,
    private view: CanvasView,
    private actions: Actions,
    private root: HTMLElement,
  ) {
    store.on((k) => {
      if (k === "selection") this.build();
      else if (k === "doc") this.refresh();
    });
    // The roll summary lives in the "nothing selected" view.
    rolls.on(() => {
      if (this.store.selection.length === 0) this.build();
    });
    this.build();
  }

  focusText(): void {
    this.textArea?.focus();
    this.textArea?.select();
  }

  // ---- building

  build(): void {
    const sel = this.store.selected();
    this.transformInputs = null;
    this.textArea = null;
    const sections: HTMLElement[] = [];
    if (sel.length === 0) sections.push(this.documentSection());
    else {
      sections.push(this.transformSection());
      if (sel.length === 1 && sel[0].kind === "text") sections.push(this.textSection(sel[0]));
      sections.push(this.styleSection(sel));
    }
    sections.push(this.objectsSection());
    this.root.replaceChildren(...sections);
    this.refresh();
  }

  private section(title: string, ...children: (HTMLElement | null)[]): HTMLElement {
    return h("section", { class: "panel-section" }, h("h3", {}, title), ...children);
  }

  private documentSection(): HTMLElement {
    const roll = rolls.current();
    const pick = h(
      "select",
      { title: "Le rouleau chargé dans le plotter" },
      h("option", { value: "", selected: !roll }, "Aucun (laize libre)"),
      ...rolls.data.rolls.map((r) =>
        h("option", { value: r.id, selected: r.id === roll?.id }, `${r.name} — reste ${fmtMeters(rolls.remaining(r))}`),
      ),
    );
    pick.addEventListener("change", () => rolls.setCurrent(pick.value || null));
    const head = h("label", { class: "field" }, h("span", {}, "Rouleau"), pick);
    if (!roll) {
      return this.section(
        "Support",
        head,
        ...this.mediaWidthRows(),
        h("div", { class: "btn-row" }, h("button", { onclick: () => openRollDialog() }, "Nouveau rouleau…")),
        h(
          "p",
          { class: "hint" },
          "Créez un rouleau (ex. « Feuille 1 », 700 mm × 10 m) pour voir ses anciennes découpes et ce qui reste.",
        ),
      );
    }
    const rem = rolls.remaining(roll);
    const n = roll.cuts.length;
    return this.section(
      "Rouleau en place",
      head,
      h(
        "div",
        { class: "roll-card" },
        h("i", { class: "roll-swatch", style: `background:${roll.color}` }),
        h(
          "div",
          { class: "roll-info" },
          h("b", {}, roll.name),
          h("span", {}, `${roll.width} mm × ${fmtMeters(roll.length)} · ${n} découpe${n > 1 ? "s" : ""}`),
          h("div", { class: "roll-bar" }, h("i", { style: `transform: scaleX(${roll.length ? rem / roll.length : 0})` })),
          h("span", { class: rem < 500 ? "roll-low" : "" }, `Reste ≈ ${fmtMeters(rem)}`),
        ),
      ),
      h(
        "div",
        { class: "btn-row" },
        h("button", { onclick: () => openRollDialog(roll) }, "Modifier…"),
        h("button", { onclick: () => openRollDialog() }, "Nouveau rouleau…"),
      ),
      h(
        "p",
        { class: "hint" },
        "Les découpes déjà faites sont en gris, à gauche du trait orange qui marque le début du vinyle libre. Le rouleau défile vers la droite.",
      ),
    );
  }

  private mediaWidthRows(): HTMLElement[] {
    const width = this.store.doc.mediaWidth;
    const preset = MEDIA_PRESETS.some(([v]) => Number(v) === width) ? String(width) : "custom";
    const field = numberField({
      label: "Laize",
      value: width,
      step: 1,
      min: 50,
      max: 3000,
      unit: "mm",
      title: "Largeur utile du rouleau, en travers",
      onCommit: (v) => this.setMediaWidth(v),
    });
    const presets = select<string>(null, MEDIA_PRESETS, preset, (v) => {
      if (v !== "custom") this.setMediaWidth(Number(v));
      else field.input.focus();
    });
    return [field.row, h("div", { class: "field" }, h("span", {}, ""), presets.select)];
  }

  private setMediaWidth(v: number): void {
    if (!(v >= 50 && v <= 3000) || v === this.store.doc.mediaWidth) return;
    this.store.checkpoint();
    this.store.doc.mediaWidth = v;
    this.store.changed();
    this.view.fit();
    this.build();
  }

  private transformSection(): HTMLElement {
    const sel = this.store.selected();
    const box = () => boundsOf(this.store.selected());
    const x = numberField({ label: "X", value: 0, unit: "mm", onCommit: (v) => this.moveTo(v, null) });
    const y = numberField({ label: "Y", value: 0, unit: "mm", onCommit: (v) => this.moveTo(null, v) });
    const w = numberField({
      label: "L",
      value: 0,
      min: 0.1,
      unit: "mm",
      title: "Largeur (le long du rouleau)",
      onCommit: (v) => {
        const b = box();
        if (!b || b.w <= 0 || v <= 0) return;
        const f = v / b.w;
        this.actions.transformSelection(scaleAt(f, this.lockRatio ? f : 1, b.x, b.y));
      },
    });
    const hh = numberField({
      label: "H",
      value: 0,
      min: 0.1,
      unit: "mm",
      title: "Hauteur (en travers du rouleau)",
      onCommit: (v) => {
        const b = box();
        if (!b || b.h <= 0 || v <= 0) return;
        const f = v / b.h;
        this.actions.transformSelection(scaleAt(this.lockRatio ? f : 1, f, b.x, b.y));
      },
    });
    const rot = numberField({
      label: "Angle",
      value: 0,
      step: 1,
      unit: "°",
      title: sel.length === 1 ? "Angle de l'objet" : "Rotation à appliquer à la sélection",
      onCommit: (v) => {
        const one = this.store.selected();
        if (one.length === 1) this.actions.rotate(v - angleOf(one[0].transform));
        else {
          this.actions.rotate(v);
          rot.input.value = "0";
        }
      },
    });
    this.transformInputs = { x: x.input, y: y.input, w: w.input, h: hh.input, rot: rot.input };
    const lock = checkbox("Proportions", this.lockRatio, (v) => (this.lockRatio = v), "Garder les proportions en changeant L ou H");
    return this.section(
      sel.length === 1 ? sel[0].name : `${sel.length} objets`,
      h("div", { class: "grid2" }, x.row, y.row, w.row, hh.row),
      lock,
      rot.row,
      h(
        "div",
        { class: "btn-row" },
        h("button", { title: "Miroir horizontal", onclick: () => this.actions.mirror(true) }, "⇋ Miroir"),
        h("button", { title: "Miroir vertical", onclick: () => this.actions.mirror(false) }, "⇵ Miroir"),
        h("button", { title: "Pivoter de 90°", onclick: () => this.actions.rotate(90) }, "⟲ 90°"),
      ),
      h(
        "div",
        { class: "btn-row" },
        h("button", { title: "Dupliquer (Ctrl+D)", onclick: () => this.actions.duplicate() }, "Dupliquer"),
        sel.length > 1 ? h("button", { title: "Fusionner en un seul objet", onclick: () => this.actions.combine() }, "Combiner") : null,
        h("button", { title: "Séparer en pièces (lettres, morceaux)", onclick: () => this.actions.breakApart() }, "Dissocier"),
        h("button", { class: "danger", title: "Supprimer (Suppr)", onclick: () => this.actions.delete() }, "Supprimer"),
      ),
    );
  }

  private moveTo(x: number | null, y: number | null): void {
    const b = boundsOf(this.store.selected());
    if (!b) return;
    this.actions.transformSelection(translate(x === null ? 0 : x - b.x, y === null ? 0 : y - b.y));
  }

  private textSection(o: TextObj): HTMLElement {
    const area = h("textarea", { rows: 3, spellcheck: false });
    area.value = o.text;
    const preview = h("div", { class: "font-preview" }, o.text.split("\n")[0] || "Aperçu");
    area.addEventListener("input", () => {
      const live = this.store.get(o.id);
      if (live?.kind !== "text") return;
      preview.textContent = area.value.split("\n")[0] || "Aperçu";
      if (!this.textEditing) {
        this.store.checkpoint();
        this.textEditing = true;
      }
      live.text = area.value;
      this.store.dirty = true;
      clearTimeout(this.textTimer);
      this.textTimer = window.setTimeout(() => void refreshText(this.store, live), 60);
    });
    area.addEventListener("blur", () => {
      if (this.textEditing) {
        this.textEditing = false;
        this.store.changed();
      }
    });
    this.textArea = area;

    const font = fontById(o.fontId);
    const family = font?.family ?? o.fontName;
    const listId = "font-families";
    const famInput = h("input", { type: "text", list: listId, value: family, spellcheck: false, title: "Tapez pour chercher une police" });
    const datalist = h("datalist", { id: listId }, ...families().map((f) => h("option", { value: f })));
    const styleSel = h(
      "select",
      {},
      ...stylesOf(family).map((f) => h("option", { value: f.id, selected: f.id === o.fontId }, f.style)),
    );
    const applyPreview = (id: string) => {
      const f = fontById(id);
      if (!f) return;
      preview.style.fontFamily = `"${f.family}"`;
      preview.style.fontWeight = String(f.weight);
      preview.style.fontStyle = f.italic ? "italic" : "normal";
    };
    applyPreview(o.fontId);
    const setFont = (id: string) => {
      this.editText(o, (t) => (t.fontId = id));
      const f = fontById(id);
      if (f) {
        this.withText(o, (t) => (t.fontName = fontName(f)));
        rememberFont(id);
      }
      applyPreview(id);
    };
    famInput.addEventListener("change", () => {
      const fam = famInput.value.trim();
      const cur = fontById(o.fontId);
      const same = stylesOf(fam).find((f) => f.style === cur?.style);
      const target = same ?? regularOf(fam);
      if (!target) {
        famInput.value = cur?.family ?? "";
        return;
      }
      styleSel.replaceChildren(
        ...stylesOf(fam).map((f) => h("option", { value: f.id, selected: f.id === target.id }, f.style)),
      );
      setFont(target.id);
    });
    famInput.addEventListener("focus", () => famInput.select());
    styleSel.addEventListener("change", () => setFont(styleSel.value));

    const size = numberField({
      label: "Taille",
      value: o.size,
      step: 1,
      min: 1,
      unit: "mm",
      title: "Corps de la police (cadratin), en mm",
      onCommit: (v) => v > 0 && this.editText(o, (t) => (t.size = v)),
    });
    const spacing = numberField({
      label: "Espacement",
      value: o.letterSpacing,
      step: 0.5,
      unit: "mm",
      title: "Espace ajouté entre les lettres",
      onCommit: (v) => this.editText(o, (t) => (t.letterSpacing = v)),
    });
    const lines = numberField({
      label: "Interligne",
      value: o.lineSpacing,
      step: 0.05,
      min: 0.1,
      unit: "×",
      onCommit: (v) => v > 0 && this.editText(o, (t) => (t.lineSpacing = v)),
    });
    const alignBtn = (a: Align, label: string, title: string) =>
      h("button", { class: o.align === a ? "active" : "", title, onclick: () => this.editText(o, (t) => (t.align = a), true) }, label);

    return this.section(
      "Texte",
      area,
      h("label", { class: "field" }, h("span", {}, "Police"), famInput),
      datalist,
      h("label", { class: "field" }, h("span", {}, "Style"), styleSel),
      preview,
      size.row,
      spacing.row,
      lines.row,
      h(
        "div",
        { class: "btn-row" },
        alignBtn("left", "Gauche", "Aligner à gauche"),
        alignBtn("center", "Centre", "Centrer"),
        alignBtn("right", "Droite", "Aligner à droite"),
      ),
      h(
        "div",
        { class: "btn-row" },
        h("button", { title: "Transformer le texte en tracé modifiable", onclick: () => this.actions.toCurves() }, "Convertir en courbes"),
      ),
    );
  }

  private withText(o: TextObj, fn: (t: TextObj) => void): void {
    const live = this.store.get(o.id);
    if (live?.kind === "text") fn(live);
  }

  /** One undoable text property change, then a new outline. */
  private editText(o: TextObj, fn: (t: TextObj) => void, rebuild = false): void {
    const live = this.store.get(o.id);
    if (live?.kind !== "text") return;
    this.store.checkpoint();
    fn(live);
    this.store.dirty = true;
    void refreshText(this.store, live).then(() => {
      this.store.changed();
      if (rebuild) this.build();
    });
  }

  private styleSection(sel: Obj[]): HTMLElement {
    const color = h("input", { type: "color", value: sel[0].color });
    const setColor = (c: string) => {
      this.store.checkpoint();
      for (const o of this.store.selected()) o.color = c;
      this.store.changed();
      color.value = c;
    };
    color.addEventListener("change", () => setColor(color.value));
    const swatches = h(
      "div",
      { class: "swatches" },
      ...SWATCHES.map((c) => h("button", { class: "swatch", style: `background:${c}`, title: c, onclick: () => setColor(c) })),
    );
    const fill = checkbox(
      "Aperçu plein",
      sel.every((o) => o.fill),
      (v) => {
        this.store.checkpoint();
        for (const o of this.store.selected()) o.fill = v;
        this.store.changed();
      },
      "Afficher l'objet rempli (sinon seulement le trait de coupe)",
    );
    return this.section("Couleur du vinyle", h("div", { class: "field" }, h("span", {}, "Couleur"), color), swatches, fill);
  }

  private objectsSection(): HTMLElement {
    this.list = h("ul", { class: "object-list" });
    return this.section(`Objets`, this.list);
  }

  // ---- refreshing values without rebuilding (keeps focus)

  refresh(): void {
    const t = this.transformInputs;
    const b = boundsOf(this.store.selected());
    if (t && b) {
      const set = (inp: HTMLInputElement, v: number) => {
        if (document.activeElement !== inp) inp.value = round(v);
      };
      set(t.x, b.x);
      set(t.y, b.y);
      set(t.w, b.w);
      set(t.h, b.h);
      const sel = this.store.selected();
      if (sel.length === 1) set(t.rot, normAngle(angleOf(sel[0].transform)));
    }
    this.refreshList();
  }

  private refreshList(): void {
    const ul = this.list;
    if (!ul) return;
    if (ul.querySelector("input")) return; // renaming in progress
    const objs = [...this.store.doc.objects].reverse();
    ul.replaceChildren(
      ...objs.map((o) => {
        const li = h(
          "li",
          { class: this.store.isSelected(o.id) ? "selected" : "" },
          h("i", { class: "swatch-dot", style: `background:${o.color}` }),
          h("span", {}, o.kind === "text" ? `« ${o.text.replace(/\n/g, " ").slice(0, 28)} »` : o.name),
          h("em", {}, o.kind === "text" ? "texte" : "tracé"),
        );
        li.addEventListener("click", (e) => {
          if (e.shiftKey) {
            const ids = this.store.isSelected(o.id)
              ? this.store.selection.filter((id) => id !== o.id)
              : [...this.store.selection, o.id];
            this.store.select(ids);
          } else this.store.select([o.id]);
        });
        li.addEventListener("dblclick", () => this.rename(li, o));
        return li;
      }),
    );
    if (objs.length === 0) ul.append(h("li", { class: "empty" }, "Aucun objet"));
  }

  private rename(li: HTMLElement, o: Obj): void {
    if (o.kind === "text") return;
    const input = h("input", { type: "text", value: o.name });
    const done = (commit: boolean) => {
      if (commit && input.value.trim() && input.value !== o.name) {
        this.store.checkpoint();
        o.name = input.value.trim();
        input.remove();
        this.store.changed();
        this.build();
      } else {
        input.remove();
        this.refreshList();
      }
    };
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") done(true);
      if (e.key === "Escape") done(false);
    });
    input.addEventListener("blur", () => done(true));
    li.replaceChildren(input);
    input.focus();
    input.select();
  }
}

function normAngle(a: number): number {
  let v = a % 360;
  if (v > 180) v -= 360;
  if (v <= -180) v += 360;
  return Math.abs(v) < 1e-9 ? 0 : v;
}
