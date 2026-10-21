import { backend, inTauri, setToastHook } from "./backend";
import { boundsOf, Store } from "./model/doc";
import { fmtMeters, rolls } from "./model/rolls";
import { persist, settings } from "./settings";
import { Actions } from "./ui/actions";
import { CanvasView, fmt, isTyping, type Tool } from "./ui/canvas";
import { openCutDialog } from "./ui/dialogs/cut";
import { IMAGE_EXTENSIONS, openTraceDialog } from "./ui/dialogs/trace";
import { openTileDialog, openWeedDialog } from "./ui/dialogs/weed";
import { $, errorMessage, h, toast } from "./ui/dom";
import { icon } from "./ui/icons";
import { SidePanel } from "./ui/panel";
import { createText, loadFonts } from "./ui/text";

const store = new Store();
setToastHook((m) => toast(m));
rolls.setErrorHandler((m) => toast(m, "error"));

/** With a roll in place, the media width is the roll's. */
function syncRollWidth(): void {
  const r = rolls.current();
  if (r && store.doc.mediaWidth !== r.width) {
    store.doc.mediaWidth = r.width;
    store.emit("doc");
  }
}
rolls.on(syncRollWidth);
store.on((k) => {
  if (k === "doc") syncRollWidth();
});

let panel: SidePanel;
const view = new CanvasView(store, $("#canvas") as HTMLCanvasElement, {
  onCreateText: (x, y) => {
    // The text is typed in the panel: make sure it is visible.
    setSidePanel(true, false);
    void createText(store, x, y).then(() => panel.focusText());
  },
  onEditText: () => {
    setSidePanel(true, false);
    panel.focusText();
  },
  onCursor: (x, y) => ($("#st-cursor").textContent = `X ${fmt(x)}   Y ${fmt(y)} mm`),
  onToolChange: (t) => markTool(t),
  onDragInfo: (text) => ($("#st-info").textContent = text ?? selectionInfo()),
});
const actions = new Actions(store, view);
panel = new SidePanel(store, view, actions, $("#side"));

// ---- side panel (collapsible)

const NARROW = 900;
const sideTab = h("button", { class: "side-tab", onclick: () => toggleSidePanel() }, "‹");
$("#stage").append(sideTab);
// In a narrow window the panel floats over the canvas: keep a tab to close it.
const narrowQuery = window.matchMedia(`(max-width: ${NARROW}px)`);

function updateSideTab(open: boolean): void {
  sideTab.hidden = open && !narrowQuery.matches;
  sideTab.classList.toggle("open", open);
  sideTab.textContent = open ? "›" : "‹";
  sideTab.title = open ? "Masquer le panneau (P)" : "Afficher le panneau (P)";
}
narrowQuery.addEventListener("change", () => updateSideTab(!$("#main").classList.contains("side-collapsed")));
let panelBtn: HTMLButtonElement | null = null;

function sidePanelOpen(): boolean {
  const pref = settings.view.sidePanel;
  return pref === "auto" ? window.innerWidth >= NARROW : pref === "open";
}

/** Opens or closes the panel; `remember` keeps the choice for next time. */
function setSidePanel(open: boolean, remember = true): void {
  if (remember) {
    settings.view.sidePanel = open ? "open" : "closed";
    persist();
  }
  $("#main").classList.toggle("side-collapsed", !open);
  updateSideTab(open);
  panelBtn?.classList.toggle("active", open);
}

function toggleSidePanel(): void {
  setSidePanel($("#main").classList.contains("side-collapsed"));
}

// ---- toolbar

function btn(name: string, label: string, title: string, onclick: () => void, cls = ""): HTMLButtonElement {
  const b = h("button", { class: `tb ${cls}`, title, onclick }, "");
  b.innerHTML = `${icon(name)}<span>${label}</span>`;
  return b;
}

function sep(): HTMLElement {
  return h("span", { class: "sep" });
}

const alignMenu = h(
  "div",
  { class: "menu", hidden: true },
  ...(
    [
      ["media", "Centrer sur la laize"],
      ["left", "Aligner à gauche"],
      ["hcenter", "Centrer horizontalement"],
      ["right", "Aligner à droite"],
      ["top", "Aligner en haut"],
      ["vcenter", "Centrer verticalement"],
      ["bottom", "Aligner en bas"],
    ] as const
  ).map(([k, label]) =>
    h("button", {
      onclick: () => {
        alignMenu.hidden = true;
        actions.align(k);
      },
    }, label),
  ),
);
const alignBtn = btn("align", "Aligner", "Aligner la sélection", () => (alignMenu.hidden = !alignMenu.hidden));
document.addEventListener("pointerdown", (e) => {
  if (!alignMenu.hidden && !alignMenu.contains(e.target as Node) && !alignBtn.contains(e.target as Node)) alignMenu.hidden = true;
});

panelBtn = btn("panel", "", "Afficher / masquer le panneau (P)", () => toggleSidePanel());
const undoBtn = btn("undo", "", "Annuler (Ctrl+Z)", () => store.undo());
const redoBtn = btn("redo", "", "Rétablir (Ctrl+Y)", () => store.redo());
const wireBtn = btn("wire", "", "Afficher seulement les traits de coupe (W)", () => toggleView("wireframe"));
const gridBtn = btn("grid", "", "Grille (G)", () => toggleView("grid"));

function syncViewButtons(): void {
  wireBtn.classList.toggle("active", settings.view.wireframe);
  gridBtn.classList.toggle("active", settings.view.grid);
}

function toggleView(k: "wireframe" | "grid"): void {
  settings.view[k] = !settings.view[k];
  persist();
  syncViewButtons();
  view.render();
}

$("#toolbar").append(
  h("div", { class: "brand", html: `${icon("cut")}<b>KI</b>&nbsp;Plotter` }),
  btn("new", "", "Nouveau (Ctrl+N)", () => void actions.newDoc()),
  btn("open", "", "Ouvrir (Ctrl+O)", () => void actions.open()),
  btn("save", "", "Enregistrer (Ctrl+S)", () => void actions.save()),
  sep(),
  btn("import", "Importer", "Importer SVG, DXF ou PLT (Ctrl+I)", () => void actions.importFile()),
  btn("trace", "Vectoriser", "Vectoriser une image PNG/JPG (Ctrl+B)", () => void openTraceDialog(actions)),
  sep(),
  undoBtn,
  redoBtn,
  sep(),
  h("div", { class: "menu-wrap" }, alignBtn, alignMenu),
  btn("weed", "Échenillage", "Cadre et lignes d'échenillage", () => openWeedDialog(store)),
  btn("tile", "Mosaïque", "Découper un grand dessin en panneaux", () => openTileDialog(store, view)),
  sep(),
  wireBtn,
  gridBtn,
  btn("zoomOut", "", "Zoom arrière (-)", () => view.zoomBy(1 / 1.25)),
  btn("zoomIn", "", "Zoom avant (+)", () => view.zoomBy(1.25)),
  btn("fit", "", "Tout voir (0) — sélection (F)", () => view.fit()),
  h("span", { class: "spacer" }),
  panelBtn,
  btn("cut", "Découper", "Envoyer au plotter (Ctrl+P)", () => openCutDialog(store), "primary"),
);
syncViewButtons();
setSidePanel(sidePanelOpen(), false);

// ---- tools

const TOOLS: [Tool, string, string][] = [
  ["select", "select", "Sélection (V)"],
  ["text", "text", "Texte (T)"],
  ["rect", "rect", "Rectangle (R)"],
  ["ellipse", "ellipse", "Ellipse (E)"],
];
const toolButtons = new Map<Tool, HTMLButtonElement>();
for (const [t, ic, title] of TOOLS) {
  const b = h("button", { class: "tool", title, onclick: () => view.setTool(t) });
  b.innerHTML = icon(ic);
  toolButtons.set(t, b);
  $("#tools").append(b);
}
function markTool(t: Tool): void {
  for (const [k, b] of toolButtons) b.classList.toggle("active", k === t);
}
markTool("select");

// ---- status bar

function selectionInfo(): string {
  const b = boundsOf(store.selected());
  if (!b) return `${store.doc.objects.length} objet${store.doc.objects.length > 1 ? "s" : ""}`;
  return `Sélection : ${fmt(b.w)} × ${fmt(b.h)} mm`;
}

function mediaInfo(): void {
  const r = rolls.current();
  $("#st-media").textContent = r
    ? `${r.name} · ${r.width} mm · reste ≈ ${fmtMeters(rolls.remaining(r))}`
    : `Laize ${fmt(store.doc.mediaWidth, 0)} mm`;
}
rolls.on(mediaInfo);

store.on(() => {
  $("#st-info").textContent = selectionInfo();
  mediaInfo();
  undoBtn.disabled = !store.canUndo;
  redoBtn.disabled = !store.canRedo;
});
setInterval(() => ($("#st-zoom").textContent = `${Math.round(view.zoom * 100 / 3.78)} %`), 250);

// ---- keyboard

window.addEventListener("keydown", (e) => {
  if (document.querySelector("dialog[open]")) return;
  if (isTyping(e)) {
    if (e.key === "Escape") (e.target as HTMLElement).blur();
    return;
  }
  const ctrl = e.ctrlKey || e.metaKey;
  const k = e.key.toLowerCase();
  const run = (fn: () => void) => {
    e.preventDefault();
    fn();
  };
  if (ctrl) {
    if (k === "n") return run(() => void actions.newDoc());
    if (k === "o") return run(() => void actions.open());
    if (k === "s") return run(() => void actions.save(e.shiftKey));
    if (k === "i") return run(() => void actions.importFile());
    if (k === "b") return run(() => void openTraceDialog(actions));
    if (k === "p" || k === "enter") return run(() => openCutDialog(store));
    if (k === "z" && !e.shiftKey) return run(() => store.undo());
    if (k === "y" || (k === "z" && e.shiftKey)) return run(() => store.redo());
    if (k === "c") return run(() => actions.copy());
    if (k === "x") return run(() => actions.cut());
    if (k === "v") return run(() => actions.paste());
    if (k === "d") return run(() => actions.duplicate());
    if (k === "a") return run(() => actions.selectAll());
    if (k === "l") return run(() => actions.combine());
    if (k === "k") return run(() => actions.breakApart());
    if (k === "q") return run(() => actions.toCurves());
  }
  if (e.altKey) return;
  const step = e.shiftKey ? 10 : ctrl ? 0.1 : 1;
  switch (e.key) {
    case "Delete":
    case "Backspace":
      return run(() => actions.delete());
    case "ArrowLeft":
      return run(() => actions.nudge(-step, 0));
    case "ArrowRight":
      return run(() => actions.nudge(step, 0));
    case "ArrowUp":
      return run(() => actions.nudge(0, -step));
    case "ArrowDown":
      return run(() => actions.nudge(0, step));
    case "Escape":
      return run(() => {
        view.setTool("select");
        store.select([]);
      });
    case "+":
    case "=":
      return run(() => view.zoomBy(1.25));
    case "-":
      return run(() => view.zoomBy(1 / 1.25));
    case "0":
      return run(() => view.fit());
  }
  if (ctrl) return;
  if (k === "v") view.setTool("select");
  else if (k === "t") view.setTool("text");
  else if (k === "r") view.setTool("rect");
  else if (k === "e") view.setTool("ellipse");
  else if (k === "w") toggleView("wireframe");
  else if (k === "p") toggleSidePanel();
  else if (k === "g") toggleView("grid");
  else if (k === "f") {
    const b = boundsOf(store.selected());
    view.fit(b ? { x: b.x - b.w * 0.1, y: b.y - b.h * 0.1, w: b.w * 1.2, h: b.h * 1.2 } : null);
  }
});

// ---- window integration (drag & drop, close guard)

async function setupWindow(): Promise<void> {
  if (!inTauri) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  const win = getCurrentWindow();
  await win.onCloseRequested(async (e) => {
    if (!store.dirty || store.doc.objects.length === 0) return;
    const { confirm } = await import("@tauri-apps/plugin-dialog");
    const ok = await confirm("Le document a des modifications non enregistrées. Quitter quand même ?", {
      title: "KI Plotter",
      kind: "warning",
      okLabel: "Quitter",
      cancelLabel: "Annuler",
    });
    if (!ok) e.preventDefault();
  });
  await getCurrentWebview().onDragDropEvent(async (e) => {
    if (e.payload.type !== "drop") return;
    for (const path of e.payload.paths) {
      const ext = path.split(".").pop()?.toLowerCase() ?? "";
      try {
        if (IMAGE_EXTENSIONS.includes(ext)) await openTraceDialog(actions, path);
        else if (ext === "kplot") toast("Utilisez Ouvrir (Ctrl+O) pour un projet.");
        else {
          const imp = await backend.importFile(path);
          actions.placeImported(imp, path.split(/[\\/]/).pop()?.replace(/\.[^.]+$/, "") ?? "Import");
        }
      } catch (err) {
        toast(`Import impossible : ${errorMessage(err)}`, "error");
      }
    }
  });
}

async function start(): Promise<void> {
  await Promise.all([loadFonts(), rolls.load()]);
  store.emit("doc");
  view.fit();
  actions.updateTitle();
  await setupWindow();
  if (!inTauri) toast("Mode navigateur : le plotter, les fichiers et les vraies polices ne sont disponibles que dans l'application.");
}

void start();
