// "Vectoriser" dialog: bitmap → cut paths, with a live threshold preview.

import { backend, pickOpenFile } from "../../backend";
import { toPath2D } from "../../model/geom";
import type { ImageInfo, Imported, TraceRequest } from "../../model/types";
import type { Actions } from "../actions";
import { fmtMm } from "../actions";
import { checkbox, errorMessage, h, modal, numberField, toast } from "../dom";

export const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "bmp", "gif"];

export async function openTraceDialog(actions: Actions, presetPath?: string): Promise<void> {
  const path = presetPath ?? (await pickOpenFile("Vectoriser une image", [{ name: "Images", extensions: IMAGE_EXTENSIONS }]));
  if (!path) return;
  let info: ImageInfo;
  try {
    info = await backend.loadImage(path);
  } catch (e) {
    toast(`Image illisible : ${errorMessage(e)}`, "error");
    return;
  }

  const img = new Image();
  img.src = info.dataUrl;
  await img.decode().catch(() => undefined);

  const dpi = info.dpi && info.dpi > 30 ? info.dpi : 150;
  const req: TraceRequest = {
    path,
    threshold: 128,
    invert: false,
    useAlpha: false,
    despeckle: 4,
    cornerThreshold: 60,
    widthMm: Math.round(Math.min(600, (info.width / dpi) * 25.4)),
  };
  let result: Imported | null = null;

  const canvas = h("canvas", { class: "trace-preview" });
  const status = h("div", { class: "cut-stats" }, `${info.width} × ${info.height} px`);

  // Thresholded preview computed here from the downscaled image (fast); the
  // real trace runs on the full-resolution image in the backend.
  const work = document.createElement("canvas");
  const draw = () => {
    const W = canvas.clientWidth;
    const H = canvas.clientHeight;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(W * dpr);
    canvas.height = Math.round(H * dpr);
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.fillStyle = "#e9ecf1";
    ctx.fillRect(0, 0, W, H);
    if (!img.naturalWidth) return;
    const k = Math.min((W - 20) / img.naturalWidth, (H - 20) / img.naturalHeight);
    const w = Math.max(1, Math.round(img.naturalWidth * k));
    const hh = Math.max(1, Math.round(img.naturalHeight * k));
    const x = (W - w) / 2;
    const y = (H - hh) / 2;
    work.width = w;
    work.height = hh;
    const wctx = work.getContext("2d", { willReadFrequently: true })!;
    wctx.clearRect(0, 0, w, hh);
    wctx.drawImage(img, 0, 0, w, hh);
    const data = wctx.getImageData(0, 0, w, hh);
    const px = data.data;
    // Top-left of the foreground: the traced result is normalised to (0, 0), so this
    // is where it sits on the image.
    let fgX = w;
    let fgY = hh;
    for (let i = 0; i < px.length; i += 4) {
      const a = px[i + 3] / 255;
      let fg: boolean;
      if (req.useAlpha) fg = a >= 0.5;
      else {
        // Composite over white, then luminance.
        const r = px[i] * a + 255 * (1 - a);
        const g = px[i + 1] * a + 255 * (1 - a);
        const b = px[i + 2] * a + 255 * (1 - a);
        fg = 0.299 * r + 0.587 * g + 0.114 * b < req.threshold;
      }
      if (req.invert) fg = !fg;
      if (fg) {
        const p = i / 4;
        fgX = Math.min(fgX, p % w);
        fgY = Math.min(fgY, Math.floor(p / w));
      }
      const v = fg ? 30 : 255;
      px[i] = v;
      px[i + 1] = fg ? 34 : 255;
      px[i + 2] = fg ? 44 : 255;
      px[i + 3] = 255;
    }
    wctx.putImageData(data, 0, 0);
    ctx.globalAlpha = result ? 0.35 : 1;
    ctx.drawImage(work, x, y);
    ctx.globalAlpha = 1;
    if (result) {
      // Outline of the traced result, scaled back onto the image.
      const mmToPx = w / req.widthMm;
      ctx.setTransform(dpr * mmToPx, 0, 0, dpr * mmToPx, dpr * (x + fgX), dpr * (y + fgY));
      ctx.lineWidth = 1.2 / mmToPx;
      ctx.strokeStyle = "#ff6a00";
      for (const it of result.items) ctx.stroke(toPath2D(it.path));
    }
  };

  const invalidate = () => {
    result = null;
    addBtn.disabled = true;
    status.textContent = `${info.width} × ${info.height} px → ${fmtMm(req.widthMm)} mm de large`;
    draw();
  };

  const threshold = h("input", { type: "range", min: 1, max: 254, value: req.threshold });
  const thresholdVal = h("em", {}, String(req.threshold));
  threshold.addEventListener("input", () => {
    req.threshold = Number(threshold.value);
    thresholdVal.textContent = threshold.value;
    invalidate();
  });

  const traceBtn = h("button", {}, "Vectoriser");
  const addBtn = h("button", { class: "primary", disabled: true }, "Ajouter au dessin");
  traceBtn.addEventListener("click", async () => {
    traceBtn.disabled = true;
    status.textContent = "Vectorisation…";
    try {
      result = await backend.traceImage(req);
      const n = result.items.reduce((acc, it) => acc + it.path.filter((s) => s.t === "M").length, 0);
      status.textContent = `${n} contour${n > 1 ? "s" : ""} · ${fmtMm(result.width)} × ${fmtMm(result.height)} mm`;
      addBtn.disabled = n === 0;
    } catch (e) {
      status.textContent = errorMessage(e);
    }
    traceBtn.disabled = false;
    draw();
  });
  addBtn.addEventListener("click", () => {
    if (!result) return;
    const name = path.split(/[\\/]/).pop()?.replace(/\.[^.]+$/, "") ?? "Image";
    // Traced output is a single colour; show it filled like vinyl.
    actions.placeImported({ ...result, items: result.items.map((it) => ({ ...it, color: null })) }, name);
    dlg.close();
  });

  const controls = h(
    "div",
    { class: "cut-right" },
    h("label", { class: "field" }, h("span", {}, "Seuil"), threshold, thresholdVal),
    checkbox("Inverser (découper le clair)", req.invert, (v) => {
      req.invert = v;
      invalidate();
    }),
    checkbox("Utiliser la transparence", req.useAlpha, (v) => {
      req.useAlpha = v;
      invalidate();
    }, "Pour les PNG détourés : tout ce qui est opaque est découpé"),
    numberField({
      label: "Largeur",
      value: req.widthMm,
      step: 1,
      min: 1,
      unit: "mm",
      onCommit: (v) => {
        if (v > 0) req.widthMm = v;
        invalidate();
      },
    }).row,
    numberField({
      label: "Taches ignorées",
      value: req.despeckle,
      step: 1,
      min: 0,
      unit: "px",
      title: "Supprime les petites taches (surface en pixels)",
      onCommit: (v) => {
        req.despeckle = Math.max(0, Math.round(v));
        invalidate();
      },
    }).row,
    numberField({
      label: "Angles vifs",
      value: req.cornerThreshold,
      step: 5,
      min: 0,
      max: 180,
      unit: "°",
      title: "En dessous de cet angle, un coin reste un coin ; au-dessus il est lissé",
      onCommit: (v) => {
        req.cornerThreshold = Math.min(180, Math.max(0, v));
        invalidate();
      },
    }).row,
    h("p", { class: "hint" }, "Réglez le seuil pour que ce qui doit être découpé apparaisse en noir, puis Vectoriser."),
  );

  const body = h("div", { class: "cut-layout" }, h("div", { class: "cut-left" }, canvas, status), controls);
  const footer = h("div", { class: "footer-row" }, h("span", { class: "spacer" }), h("button", { onclick: () => dlg.close() }, "Annuler"), traceBtn, addBtn);
  const dlg = modal("Vectoriser une image", body, footer, "cut-dialog");
  new ResizeObserver(() => draw()).observe(canvas);
  invalidate();
}
