// Typed access to the Rust backend. Outside Tauri (plain browser, for UI
// work) a small mock stands in so the editor still runs.

import { apply, pathBounds, rectPath, roughPolygon, subpaths, unionRects } from "./model/geom";
import type {
  FontInfo,
  ImageInfo,
  Imported,
  JobFinished,
  JobProgress,
  JobRequest,
  JobResult,
  Move,
  PathData,
  PortInfo,
  RelayInfo,
  RelayStatus,
  SendRequest,
  TextOutline,
  TextRequest,
  Tile,
  TileRequest,
  TraceRequest,
  WeedRequest,
} from "./model/types";

export const inTauri = "__TAURI_INTERNALS__" in window;

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (inTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>(cmd, args);
  }
  return mock(cmd, args ?? {}) as Promise<T>;
}

export const backend = {
  listFonts: () => call<FontInfo[]>("list_fonts"),
  textOutline: (req: TextRequest) => call<TextOutline>("text_outline", { req }),
  importFile: (path: string) => call<Imported>("import_file", { path }),
  loadImage: (path: string) => call<ImageInfo>("load_image", { path }),
  traceImage: (req: TraceRequest) => call<Imported>("trace_image", { req }),
  makeWeeding: (req: WeedRequest) => call<PathData>("make_weeding", { req }),
  makeTiles: (req: TileRequest) => call<Tile[]>("make_tiles", { req }),
  buildJob: (req: JobRequest) => call<JobResult>("build_job", { req }),
  listPorts: () => call<PortInfo[]>("list_ports"),
  listPrinters: () => call<string[]>("list_printers"),
  discoverRelays: () => call<RelayInfo[]>("discover_relays"),
  relayStatus: (host: string, port: number) => call<RelayStatus>("relay_status", { host, port }),
  sendJob: (req: SendRequest) => call<void>("send_job", { req }),
  cancelJob: () => call<void>("cancel_job"),
  readTextFile: (path: string) => call<string>("read_text_file", { path }),
  writeTextFile: (path: string, contents: string) => call<void>("write_text_file", { path, contents }),
};

type Unlisten = () => void;

export async function onJobProgress(cb: (p: JobProgress) => void): Promise<Unlisten> {
  if (inTauri) {
    const { listen } = await import("@tauri-apps/api/event");
    return listen<JobProgress>("job-progress", (e) => cb(e.payload));
  }
  mockBus.progress.push(cb);
  return () => (mockBus.progress = mockBus.progress.filter((f) => f !== cb));
}

export async function onJobFinished(cb: (p: JobFinished) => void): Promise<Unlisten> {
  if (inTauri) {
    const { listen } = await import("@tauri-apps/api/event");
    return listen<JobFinished>("job-finished", (e) => cb(e.payload));
  }
  mockBus.finished.push(cb);
  return () => (mockBus.finished = mockBus.finished.filter((f) => f !== cb));
}

export interface FileFilter {
  name: string;
  extensions: string[];
}

export async function pickOpenFile(title: string, filters: FileFilter[]): Promise<string | null> {
  if (!inTauri) {
    toastHook?.("Ouvrir un fichier n'est possible que dans l'application.");
    return null;
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const r = await open({ title, filters, multiple: false, directory: false });
  return typeof r === "string" ? r : null;
}

export async function pickSaveFile(title: string, filters: FileFilter[], defaultPath?: string): Promise<string | null> {
  if (!inTauri) {
    toastHook?.("Enregistrer n'est possible que dans l'application.");
    return null;
  }
  const { save } = await import("@tauri-apps/plugin-dialog");
  return (await save({ title, filters, defaultPath })) ?? null;
}

export async function askConfirm(message: string, title = "KI Plotter"): Promise<boolean> {
  if (!inTauri) return window.confirm(message);
  const { confirm } = await import("@tauri-apps/plugin-dialog");
  return confirm(message, { title, kind: "warning", okLabel: "Continuer", cancelLabel: "Annuler" });
}

export async function setWindowTitle(title: string): Promise<void> {
  document.title = title;
  if (!inTauri) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().setTitle(title);
}

let toastHook: ((msg: string) => void) | null = null;
export function setToastHook(fn: (msg: string) => void): void {
  toastHook = fn;
}

// ---------------------------------------------------------------- mock

const mockBus = {
  progress: [] as ((p: JobProgress) => void)[],
  finished: [] as ((p: JobFinished) => void)[],
  cancel: false,
};

const MOCK_FONTS: FontInfo[] = [
  { id: "mock/arial#0", family: "Arial", style: "Regular", weight: 400, italic: false },
  { id: "mock/arial-bold#0", family: "Arial", style: "Bold", weight: 700, italic: false },
  { id: "mock/impact#0", family: "Impact", style: "Regular", weight: 400, italic: false },
];

function mockText(req: TextRequest): TextOutline {
  const scale = req.size;
  const ascent = 0.9 * scale;
  const descent = 0.2 * scale;
  const lineH = (ascent + descent) * req.lineSpacing;
  const lines = req.text.split("\n");
  const widths = lines.map((l) => l.length * 0.6 * scale + Math.max(0, l.length - 1) * req.letterSpacing);
  const width = Math.max(0, ...widths);
  const path: PathData = [];
  lines.forEach((line, li) => {
    let x = req.align === "left" ? 0 : req.align === "center" ? (width - widths[li]) / 2 : width - widths[li];
    for (const ch of line) {
      if (ch !== " ") path.push(...rectPath(x + 0.05 * scale, li * lineH + 0.2 * scale, 0.5 * scale, 0.7 * scale));
      x += 0.6 * scale + req.letterSpacing;
    }
  });
  return { path, width, height: lines.length * lineH, ascent, descent };
}

function mockJob(req: JobRequest): JobResult {
  const moves: Move[] = [];
  let cut = 0;
  let travel = 0;
  let pos: [number, number] = [0, 0];
  const rects = req.shapes.map((s) => pathBounds(s.path, s.transform));
  const bounds = unionRects(rects) ?? { x: 0, y: 0, w: 0, h: 0 };
  let data = "IN;";
  for (const s of req.shapes) {
    for (const sub of subpaths(s.path)) {
      const pts = roughPolygon(sub).map(([x, y]) => apply(s.transform, x, y));
      if (sub[sub.length - 1].t === "Z" && pts.length) pts.push(pts[0]);
      if (pts.length < 2) continue;
      travel += Math.hypot(pts[0][0] - pos[0], pts[0][1] - pos[1]);
      moves.push({ down: false, pts: [pos, pts[0]] });
      moves.push({ down: true, pts });
      for (let i = 1; i < pts.length; i++) cut += Math.hypot(pts[i][0] - pts[i - 1][0], pts[i][1] - pts[i - 1][1]);
      pos = pts[pts.length - 1];
      const k = req.machine.unitsPerMm;
      const xy = (p: [number, number]) => `${Math.round((p[0] - bounds.x) * k)},${Math.round((bounds.y + bounds.h - p[1]) * k)}`;
      data += `PU${xy(pts[0])};PD${pts.slice(1).map(xy).join(",")};`;
    }
  }
  data += "PU0,0;";
  const speed = (req.machine.speed || 20) * 10;
  return {
    data,
    moves,
    stats: {
      pathCount: moves.length / 2,
      cutLength: cut,
      travelLength: travel,
      estimatedSeconds: cut / speed + travel / (2 * speed) + 0.1 * (moves.length / 2),
      bounds,
    },
  };
}

async function mockSend(req: SendRequest): Promise<void> {
  mockBus.cancel = false;
  const total = req.data.length;
  let sent = 0;
  const step = () => {
    if (mockBus.cancel) {
      mockBus.finished.forEach((f) => f({ ok: false, cancelled: true, error: null }));
      return;
    }
    sent = Math.min(total, sent + Math.max(1, req.chunkSize));
    mockBus.progress.forEach((f) => f({ sent, total }));
    if (sent >= total) mockBus.finished.forEach((f) => f({ ok: true, cancelled: false, error: null }));
    else setTimeout(step, 30);
  };
  setTimeout(step, 30);
}

async function mock(cmd: string, args: Record<string, unknown>): Promise<unknown> {
  await new Promise((r) => setTimeout(r, 5));
  switch (cmd) {
    case "list_fonts":
      return MOCK_FONTS;
    case "text_outline":
      return mockText(args.req as TextRequest);
    case "build_job":
      return mockJob(args.req as JobRequest);
    case "make_weeding": {
      const req = args.req as WeedRequest;
      const r = unionRects(req.shapes.map((s) => pathBounds(s.path, s.transform)));
      if (!r) throw "Rien à écheniller.";
      const m = req.margin;
      return rectPath(r.x - m, r.y - m, r.w + 2 * m, r.h + 2 * m);
    }
    case "make_tiles":
      throw "La mosaïque n'est disponible que dans l'application.";
    case "list_ports":
      return [
        { name: "COM3", kind: "usb", description: "Loupedeck", manufacturer: null, vid: 0x2ec2, pid: 4 },
        { name: "COM5", kind: "usb", description: "USB-SERIAL CH340", manufacturer: "wch.cn", vid: 0x1a86, pid: 0x7523 },
      ] satisfies PortInfo[];
    case "list_printers":
      return ["Microsoft Print to PDF"];
    case "discover_relays":
      return [{ name: "atelier", host: "192.168.1.42", port: 7870, version: "0.1.0" }] satisfies RelayInfo[];
    case "relay_status": {
      if (args.host !== "192.168.1.42") throw `Impossible de joindre le relais ${String(args.host)}:${String(args.port)}.`;
      return {
        name: "atelier",
        version: "0.1.0",
        busy: false,
        sent: 0,
        total: 0,
        device: "/dev/ttyUSB0",
        deviceError: null,
        ports: [{ name: "/dev/ttyUSB0", kind: "usb", description: "USB Serial", manufacturer: "1a86", vid: 0x1a86, pid: 0x7523 }],
        last: null,
      } satisfies RelayStatus;
    }
    case "send_job":
      return mockSend(args.req as SendRequest);
    case "cancel_job":
      mockBus.cancel = true;
      return;
    default:
      throw `« ${cmd} » n'est disponible que dans l'application.`;
  }
}
