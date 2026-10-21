// Fonts and text objects: outlines come from the backend (rustybuzz).

import { backend } from "../backend";
import { newId, type Store, type TextObj } from "../model/doc";
import { translate } from "../model/geom";
import type { FontInfo } from "../model/types";
import { persist, settings } from "../settings";
import { errorMessage, toast } from "./dom";

export let fonts: FontInfo[] = [];
let byFamily = new Map<string, FontInfo[]>();

export async function loadFonts(): Promise<void> {
  try {
    fonts = await backend.listFonts();
  } catch (e) {
    toast(`Polices introuvables : ${errorMessage(e)}`, "error");
    fonts = [];
  }
  byFamily = new Map();
  for (const f of fonts) {
    const list = byFamily.get(f.family) ?? [];
    list.push(f);
    byFamily.set(f.family, list);
  }
}

export function families(): string[] {
  return [...byFamily.keys()];
}

export function stylesOf(family: string): FontInfo[] {
  return byFamily.get(family) ?? [];
}

export function fontById(id: string): FontInfo | undefined {
  return fonts.find((f) => f.id === id);
}

export function fontName(f: FontInfo): string {
  return `${f.family} ${f.style}`;
}

/** The style closest to "Regular" in a family. */
export function regularOf(family: string): FontInfo | undefined {
  const list = stylesOf(family);
  const upright = list.filter((f) => !f.italic);
  // Families can hold widths too ("Arial" + "Narrow"), so prefer the plainest style name.
  return (
    upright.find((f) => /^(regular|normal|standard|book|roman)$/i.test(f.style)) ??
    upright.filter((f) => f.weight === 400).sort((a, b) => a.style.length - b.style.length)[0] ??
    upright[0] ??
    list[0]
  );
}

export function defaultFont(): FontInfo | undefined {
  const last = settings.lastFont ? fontById(settings.lastFont) : undefined;
  return last ?? regularOf("Arial") ?? regularOf("Segoe UI") ?? fonts[0];
}

/** Finds the font of a text object, falling back to its saved name (ids are file paths and can change). */
export function resolveFont(o: TextObj): FontInfo | undefined {
  return fontById(o.fontId) ?? fonts.find((f) => fontName(f) === o.fontName) ?? defaultFont();
}

const generation = new Map<string, number>();

/** Recomputes a text object's outline. Results that arrive out of order are dropped. */
export async function refreshText(store: Store, o: TextObj): Promise<void> {
  const font = resolveFont(o);
  if (!font) {
    toast("Aucune police disponible.", "error");
    return;
  }
  o.fontId = font.id;
  o.fontName = fontName(font);
  const gen = (generation.get(o.id) ?? 0) + 1;
  generation.set(o.id, gen);
  try {
    const out = await backend.textOutline({
      fontId: font.id,
      text: o.text,
      size: o.size,
      letterSpacing: o.letterSpacing,
      lineSpacing: o.lineSpacing,
      align: o.align,
    });
    if (generation.get(o.id) !== gen) return;
    // The store may have swapped the object (undo); write to the live one.
    const live = store.get(o.id);
    if (live?.kind !== "text") return;
    live.path = out.path;
    live.fontId = font.id;
    live.fontName = o.fontName;
    store.emit("doc");
  } catch (e) {
    toast(`Texte : ${errorMessage(e)}`, "error");
  }
}

export async function createText(store: Store, x: number, y: number): Promise<TextObj | null> {
  const font = defaultFont();
  if (!font) {
    toast("Aucune police trouvée sur ce PC.", "error");
    return null;
  }
  const o: TextObj = {
    id: newId(),
    kind: "text",
    name: "Texte",
    color: "#2b2f36",
    fill: true,
    transform: translate(x, y),
    text: "Texte",
    fontId: font.id,
    fontName: fontName(font),
    size: 50,
    letterSpacing: 0,
    lineSpacing: 1,
    align: "left",
    path: [],
    fillRule: "nonzero",
  };
  store.checkpoint();
  store.add([o]);
  await refreshText(store, o);
  return o;
}

export function rememberFont(id: string): void {
  settings.lastFont = id;
  persist();
}
