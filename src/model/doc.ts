// The document, the selection and undo history.

import { IDENTITY, pathBounds, unionRects } from "./geom";
import type { Align, FillRule, Mat, PathData, Rect, Shape } from "./types";

interface BaseObj {
  id: string;
  name: string;
  /** Display colour (vinyl colour); also lets the user cut colour by colour later. */
  color: string;
  /** Filled preview. Off for open lines (weeding lines, frames). */
  fill: boolean;
  transform: Mat;
}

export interface PathObj extends BaseObj {
  kind: "path";
  path: PathData;
  fillRule: FillRule;
}

export interface TextObj extends BaseObj {
  kind: "text";
  text: string;
  fontId: string;
  /** "Family Style", shown in the UI and used to find the font again if its id changed. */
  fontName: string;
  size: number;
  letterSpacing: number;
  lineSpacing: number;
  align: Align;
  /** Outline from the backend, in the object's local coordinates. */
  path: PathData;
  fillRule: FillRule;
}

export type Obj = PathObj | TextObj;

export interface DocData {
  version: 1;
  /** Roll width in mm (the cutter's usable width across the roll). */
  mediaWidth: number;
  objects: Obj[];
}

export const FILE_EXT = "kplot";

let nextId = 1;
export function newId(): string {
  return `o${Date.now().toString(36)}${(nextId++).toString(36)}`;
}

export function objBounds(o: Obj): Rect | null {
  return pathBounds(o.path, o.transform);
}

export function boundsOf(objs: Obj[]): Rect | null {
  return unionRects(objs.map(objBounds));
}

export function toShape(o: Obj): Shape {
  return { path: o.path, transform: o.transform, fillRule: o.fillRule };
}

export function cloneObj<T extends Obj>(o: T): T {
  return structuredClone(o);
}

export function makePathObj(name: string, path: PathData, opts: Partial<PathObj> = {}): PathObj {
  return {
    id: newId(),
    kind: "path",
    name,
    color: "#2b2f36",
    fill: true,
    transform: [...IDENTITY],
    path,
    fillRule: "nonzero",
    ...opts,
  };
}

export type ChangeKind = "doc" | "selection" | "view";

interface Snapshot {
  doc: DocData;
  selection: string[];
}

const HISTORY_LIMIT = 150;

export class Store {
  doc: DocData = { version: 1, mediaWidth: 720, objects: [] };
  selection: string[] = [];
  filePath: string | null = null;
  dirty = false;

  private undoStack: Snapshot[] = [];
  private redoStack: Snapshot[] = [];
  private listeners: ((kind: ChangeKind) => void)[] = [];

  on(fn: (kind: ChangeKind) => void): void {
    this.listeners.push(fn);
  }

  emit(kind: ChangeKind): void {
    for (const fn of this.listeners) fn(kind);
  }

  private snap(): Snapshot {
    return { doc: structuredClone(this.doc), selection: [...this.selection] };
  }

  /** Call before every change that should be undoable. */
  checkpoint(): void {
    this.undoStack.push(this.snap());
    if (this.undoStack.length > HISTORY_LIMIT) this.undoStack.shift();
    this.redoStack = [];
  }

  /** Call after a change: marks the document dirty and redraws. */
  changed(): void {
    this.dirty = true;
    this.emit("doc");
  }

  get canUndo(): boolean {
    return this.undoStack.length > 0;
  }

  get canRedo(): boolean {
    return this.redoStack.length > 0;
  }

  undo(): void {
    const s = this.undoStack.pop();
    if (!s) return;
    this.redoStack.push(this.snap());
    this.restore(s);
  }

  redo(): void {
    const s = this.redoStack.pop();
    if (!s) return;
    this.undoStack.push(this.snap());
    this.restore(s);
  }

  private restore(s: Snapshot): void {
    this.doc = s.doc;
    this.selection = s.selection.filter((id) => this.doc.objects.some((o) => o.id === id));
    this.changed();
    this.emit("selection");
  }

  load(doc: DocData, filePath: string | null): void {
    this.doc = doc;
    this.selection = [];
    this.filePath = filePath;
    this.dirty = false;
    this.undoStack = [];
    this.redoStack = [];
    this.emit("doc");
    this.emit("selection");
  }

  get(id: string): Obj | undefined {
    return this.doc.objects.find((o) => o.id === id);
  }

  selected(): Obj[] {
    const ids = new Set(this.selection);
    return this.doc.objects.filter((o) => ids.has(o.id));
  }

  isSelected(id: string): boolean {
    return this.selection.includes(id);
  }

  select(ids: string[]): void {
    this.selection = ids;
    this.emit("selection");
  }

  add(objs: Obj[], select = true): void {
    this.doc.objects.push(...objs);
    if (select) this.selection = objs.map((o) => o.id);
    this.changed();
    if (select) this.emit("selection");
  }

  remove(ids: string[]): void {
    const set = new Set(ids);
    this.doc.objects = this.doc.objects.filter((o) => !set.has(o.id));
    this.selection = this.selection.filter((id) => !set.has(id));
    this.changed();
    this.emit("selection");
  }

  /** Replaces objects in place (same position in the stacking order). */
  replace(id: string, objs: Obj[]): void {
    const i = this.doc.objects.findIndex((o) => o.id === id);
    if (i < 0) return;
    this.doc.objects.splice(i, 1, ...objs);
  }
}

export function serialize(doc: DocData): string {
  return JSON.stringify({ app: "KI Plotter", ...doc });
}

export function deserialize(text: string): DocData {
  const raw = JSON.parse(text) as Partial<DocData> & { app?: string };
  if (raw.version !== 1 || !Array.isArray(raw.objects)) {
    throw new Error("Ce fichier n'est pas un projet KI Plotter valide.");
  }
  return {
    version: 1,
    mediaWidth: typeof raw.mediaWidth === "number" ? raw.mediaWidth : 720,
    objects: raw.objects,
  };
}
