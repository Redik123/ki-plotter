// "Rouleau" dialog: create a roll, or edit one (rename, measure what is
// left, take back the last cut, renew, delete).

import { askConfirm } from "../../backend";
import { fmtMeters, rolls, type Roll } from "../../model/rolls";
import { errorMessage, h, modal, numberField, toast } from "../dom";

const WIDTHS: [number, string][] = [
  [720, "720 mm (KI-870A)"],
  [700, "700 mm"],
  [610, "610 mm (24 pouces)"],
  [500, "500 mm"],
  [300, "300 mm (12 pouces)"],
];

const COLORS: [string, string][] = [
  ["#ffffff", "Blanc"],
  ["#2b2f36", "Noir"],
  ["#d7263d", "Rouge"],
  ["#f6c90e", "Jaune"],
  ["#1b6ec2", "Bleu"],
  ["#2a9d4b", "Vert"],
  ["#c9a227", "Or"],
  ["#a7adb7", "Argent"],
  ["#f28c28", "Orange"],
  ["#7b3fa0", "Violet"],
];

export function openRollDialog(existing?: Roll): void {
  const r = existing;
  const f = {
    name: r?.name ?? rolls.nextName(),
    color: r?.color ?? "#ffffff",
    width: r?.width ?? 720,
    lengthM: (r?.length ?? 10_000) / 1000,
    remainingM: r ? rolls.remaining(r) / 1000 : 0,
  };

  const name = h("input", { type: "text", value: f.name, maxlength: 40, placeholder: "Feuille noir 1" });
  name.addEventListener("input", () => (f.name = name.value));
  const color = h("input", { type: "color", value: f.color });
  const setColor = (c: string) => {
    f.color = c;
    color.value = c;
    swatches.querySelectorAll("button").forEach((b) => b.classList.toggle("active", b.dataset.c === c));
  };
  color.addEventListener("input", () => setColor(color.value));
  const swatches = h(
    "div",
    { class: "swatches" },
    ...COLORS.map(([c, label]) =>
      h("button", { class: `swatch${c === f.color ? " active" : ""}`, style: `background:${c}`, title: label, "data-c": c, onclick: () => setColor(c) }),
    ),
  );

  const width = numberField({ label: "Laize", value: f.width, step: 1, min: 50, max: 3000, unit: "mm", onCommit: (v) => (f.width = v) });
  width.input.addEventListener("input", () => (f.width = Number(width.input.value) || f.width));
  const preset = h(
    "select",
    {},
    h("option", { value: "" }, "Laizes courantes…"),
    ...WIDTHS.map(([v, t]) => h("option", { value: String(v) }, t)),
  );
  preset.addEventListener("change", () => {
    if (!preset.value) return;
    f.width = Number(preset.value);
    width.input.value = preset.value;
    preset.value = "";
  });
  const length = numberField({ label: "Longueur", value: f.lengthM, step: 0.5, min: 0.1, unit: "m", title: "Longueur du rouleau neuf", onCommit: (v) => (f.lengthM = v) });
  length.input.addEventListener("input", () => (f.lengthM = Number(length.input.value.replace(",", ".")) || f.lengthM));

  const body = h(
    "div",
    { class: "form" },
    h("label", { class: "field" }, h("span", {}, "Nom"), name),
    h("div", { class: "field" }, h("span", {}, "Couleur"), h("div", { class: "color-row" }, color, swatches)),
    width.row,
    h("div", { class: "field" }, h("span", {}, ""), preset),
    length.row,
  );

  let remaining: HTMLInputElement | null = null;
  if (r) {
    const rem = numberField({
      label: "Reste mesuré",
      value: Math.round(f.remainingM * 100) / 100,
      step: 0.1,
      min: 0,
      unit: "m",
      title: "Si vous avez mesuré ce qui reste sur le rouleau, indiquez-le ici",
      onCommit: (v) => (f.remainingM = v),
    });
    rem.input.addEventListener("input", () => (f.remainingM = Number(rem.input.value.replace(",", ".")) || 0));
    remaining = rem.input;
    const last = r.cuts.slice(-5).reverse();
    body.append(
      rem.row,
      h("h3", { class: "form-title" }, `Historique (${r.cuts.length} découpe${r.cuts.length > 1 ? "s" : ""})`),
      last.length
        ? h(
            "ul",
            { class: "roll-history" },
            ...last.map((c) =>
              h(
                "li",
                {},
                h("span", {}, c.name),
                h("em", {}, `${fmtMeters(c.length)} · ${new Date(c.date).toLocaleDateString("fr-FR", { day: "numeric", month: "short" })}`),
              ),
            ),
          )
        : h("p", { class: "hint" }, "Aucune découpe enregistrée sur ce rouleau."),
      h(
        "div",
        { class: "btn-row" },
        h("button", {
          disabled: r.cuts.length === 0,
          title: "Si le dernier travail n'a finalement pas été coupé",
          onclick: () => {
            const cut = rolls.undoLast(r.id);
            if (cut) toast(`« ${cut.name} » retiré : ${fmtMeters(cut.length)} rendus au rouleau.`);
            dlg.close();
          },
        }, "Annuler la dernière découpe"),
        h("button", {
          title: "Nouveau rouleau du même vinyle : historique effacé, longueur complète",
          onclick: async () => {
            if (!(await askConfirm(`Remettre « ${r.name} » à neuf ? L'historique des découpes sera effacé.`))) return;
            rolls.renew(r.id);
            toast(`« ${r.name} » remis à neuf.`);
            dlg.close();
          },
        }, "Rouleau neuf"),
        h("button", {
          class: "danger",
          onclick: async () => {
            if (!(await askConfirm(`Supprimer le rouleau « ${r.name} » et son historique ?`))) return;
            rolls.remove(r.id);
            dlg.close();
          },
        }, "Supprimer"),
      ),
    );
  }

  const save = () => {
    // Pick up a value typed but not yet confirmed with Enter.
    f.width = Number(width.input.value) || f.width;
    f.lengthM = Number(length.input.value.replace(",", ".")) || f.lengthM;
    if (!f.name.trim()) {
      toast("Donnez un nom au rouleau.", "error");
      return;
    }
    if (!(f.width >= 50 && f.width <= 3000) || !(f.lengthM > 0)) {
      toast("Laize ou longueur invalide.", "error");
      return;
    }
    try {
      const fields = { name: f.name.trim(), color: f.color, width: Math.round(f.width), length: Math.round(f.lengthM * 1000) };
      if (r) {
        rolls.update(r.id, fields);
        const measured = remaining ? Number(remaining.value.replace(",", ".")) : NaN;
        if (Number.isFinite(measured) && Math.abs(measured * 1000 - rolls.remaining(r)) > 1) rolls.setRemaining(r.id, measured * 1000);
        toast(`« ${fields.name} » enregistré.`);
      } else {
        rolls.create(fields);
        toast(`« ${fields.name} » en place : ${fmtMeters(fields.length)} de ${fields.width} mm.`);
      }
      dlg.close();
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };

  const footer = h(
    "div",
    { class: "footer-row" },
    h("span", { class: "spacer" }),
    h("button", { onclick: () => dlg.close() }, "Annuler"),
    h("button", { class: "primary", onclick: save }, r ? "Enregistrer" : "Créer et mettre en place"),
  );
  const dlg = modal(r ? `Rouleau « ${r.name} »` : "Nouveau rouleau", body, footer);
  name.focus();
  name.select();
}
