// Cutter and output settings, remembered between sessions.

import type { CutSettings, Flow, MachineSettings } from "./model/types";

export type OutputKind = "serial" | "printer" | "file" | "network";

export interface OutputSettings {
  kind: OutputKind;
  port: string;
  baud: number;
  flow: Flow;
  printer: string;
  /** Raspberry Pi relay: address and TCP port (its line settings live on the Pi). */
  relayHost: string;
  relayPort: number;
  chunkSize: number;
  chunkDelayMs: number;
}

export interface Settings {
  cut: CutSettings;
  machine: Omit<MachineSettings, "mediaWidth">;
  output: OutputSettings;
  view: { wireframe: boolean; grid: boolean };
  lastFont: string | null;
}

export const DEFAULT_SETTINGS: Settings = {
  cut: {
    bladeOffset: 0.25,
    overcut: 1,
    passes: 1,
    optimize: true,
    mirror: false,
    fromOrigin: true,
    cornerThreshold: 15,
    tolerance: 0.02,
  },
  machine: {
    protocol: "hpgl",
    unitsPerMm: 40,
    scaleX: 1,
    scaleY: 1,
    swapXY: false,
    flipX: false,
    flipY: true,
    sendSpeedForce: false,
    speed: 20,
    force: 80,
    feedAfter: 0,
    maxPointsPerCommand: 0,
  },
  output: {
    kind: "serial",
    port: "",
    baud: 9600,
    flow: "hardware",
    printer: "",
    relayHost: "",
    relayPort: 7870,
    chunkSize: 256,
    chunkDelayMs: 0,
  },
  view: { wireframe: false, grid: true },
  lastFont: null,
};

const KEY = "ki-plotter.settings";

function merge<T extends object>(base: T, over: unknown): T {
  if (!over || typeof over !== "object") return structuredClone(base);
  const out = structuredClone(base) as Record<string, unknown>;
  for (const [k, v] of Object.entries(over as Record<string, unknown>)) {
    if (!(k in out)) continue;
    const b = out[k];
    if (b && typeof b === "object" && !Array.isArray(b)) out[k] = merge(b as object, v);
    else if (b === null || typeof v === typeof b) out[k] = v;
  }
  return out as T;
}

export function loadSettings(): Settings {
  try {
    const raw = localStorage.getItem(KEY);
    return merge(DEFAULT_SETTINGS, raw ? JSON.parse(raw) : null);
  } catch {
    return structuredClone(DEFAULT_SETTINGS);
  }
}

export function saveSettings(s: Settings): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(s));
  } catch {
    // Settings are a convenience; losing them is not fatal.
  }
}

export const settings: Settings = loadSettings();

export function persist(): void {
  saveSettings(settings);
}
