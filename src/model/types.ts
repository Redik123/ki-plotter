// Wire types shared with the Rust backend (see docs/ARCHITECTURE.md).
// Everything is in millimetres, document space: x along the roll (feed),
// y across it, pointing down on screen.

export type Seg =
  | { t: "M"; x: number; y: number }
  | { t: "L"; x: number; y: number }
  | { t: "C"; x1: number; y1: number; x2: number; y2: number; x: number; y: number }
  | { t: "Z" };

/** One or more subpaths, each starting with M. */
export type PathData = Seg[];

/** [a, b, c, d, e, f] as in SVG/canvas: x' = a*x + c*y + e, y' = b*x + d*y + f. */
export type Mat = [number, number, number, number, number, number];

export type FillRule = "nonzero" | "evenodd";

export interface Shape {
  path: PathData;
  transform: Mat;
  fillRule: FillRule;
}

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

// ---- Fonts and text ----

export interface FontInfo {
  id: string;
  family: string;
  style: string;
  weight: number;
  italic: boolean;
}

export type Align = "left" | "center" | "right";

export interface TextRequest {
  fontId: string;
  text: string;
  /** Em size in mm. */
  size: number;
  /** Extra space after each glyph, mm. */
  letterSpacing: number;
  /** Multiplier of the font's natural line height. */
  lineSpacing: number;
  align: Align;
}

export interface TextOutline {
  path: PathData;
  width: number;
  height: number;
  ascent: number;
  descent: number;
}

// ---- Imports and tracing ----

export interface ImportedItem {
  path: PathData;
  color: string | null;
  fillRule: FillRule;
}

export interface Imported {
  items: ImportedItem[];
  width: number;
  height: number;
}

export interface ImageInfo {
  dataUrl: string;
  width: number;
  height: number;
  dpi: number | null;
}

export interface TraceRequest {
  path: string;
  threshold: number;
  invert: boolean;
  useAlpha: boolean;
  despeckle: number;
  cornerThreshold: number;
  widthMm: number;
}

// ---- Weeding and tiling ----

export interface WeedRequest {
  shapes: Shape[];
  margin: number;
  box: boolean;
  horizontal: number;
  vertical: number;
  gap: number;
}

export interface TileRequest {
  shapes: Shape[];
  tileWidth: number;
  tileHeight: number;
  overlap: number;
  frame: boolean;
}

export interface Tile {
  col: number;
  row: number;
  rect: Rect;
  path: PathData;
}

// ---- Cut job ----

export interface CutSettings {
  bladeOffset: number;
  overcut: number;
  passes: number;
  optimize: boolean;
  mirror: boolean;
  fromOrigin: boolean;
  cornerThreshold: number;
  tolerance: number;
}

export type Protocol = "hpgl" | "dmpl";

export interface MachineSettings {
  protocol: Protocol;
  unitsPerMm: number;
  scaleX: number;
  scaleY: number;
  swapXY: boolean;
  flipX: boolean;
  flipY: boolean;
  sendSpeedForce: boolean;
  speed: number;
  force: number;
  feedAfter: number;
  maxPointsPerCommand: number;
  mediaWidth: number;
}

export interface JobRequest {
  shapes: Shape[];
  cut: CutSettings;
  machine: MachineSettings;
}

export interface Move {
  down: boolean;
  pts: [number, number][];
}

export interface JobStats {
  pathCount: number;
  cutLength: number;
  travelLength: number;
  estimatedSeconds: number;
  bounds: Rect;
}

export interface JobResult {
  data: string;
  moves: Move[];
  stats: JobStats;
}

// ---- Output ----

export type Flow = "none" | "hardware" | "software";

export type Destination =
  | { kind: "serial"; port: string; baud: number; flow: Flow }
  | { kind: "printer"; name: string }
  | { kind: "file"; path: string }
  /** KI Plotter relay (Raspberry Pi wired to the cutter). Empty device, baud 0, flow null = the relay's settings. */
  | { kind: "network"; host: string; port: number; device: string; baud: number; flow: Flow | null };

/** What the relay shows on its screen, and the settings its own test cut uses. */
export interface JobInfo {
  name: string;
  widthMm: number;
  heightMm: number;
  estimatedSeconds: number;
  settings: { cut: CutSettings; machine: MachineSettings } | null;
}

export interface SendRequest {
  destination: Destination;
  data: string;
  chunkSize: number;
  chunkDelayMs: number;
  info: JobInfo;
}

export interface PortInfo {
  name: string;
  kind: "usb" | "bluetooth" | "pci" | "unknown";
  description: string | null;
  manufacturer: string | null;
  vid: number | null;
  pid: number | null;
}

export interface RelayInfo {
  name: string;
  host: string;
  port: number;
  version: string;
}

export interface RelayStatus {
  name: string;
  version: string;
  busy: boolean;
  sent: number;
  total: number;
  device: string;
  deviceError: string | null;
  ports: PortInfo[];
  last: { ok: boolean; cancelled: boolean; error: string | null; bytes: number; client: string } | null;
  mode: "queue" | "direct";
  queued: number;
}

export interface JobProgress {
  sent: number;
  total: number;
}

export interface JobFinished {
  ok: boolean;
  cancelled: boolean;
  error: string | null;
  /** The relay keeps the job in its queue; it is cut when started on the relay's screen. */
  queued: boolean;
  position: number;
}
