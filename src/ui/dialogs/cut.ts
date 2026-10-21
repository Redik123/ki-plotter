// "Découper" dialog: cut settings, live preview/simulation, output and sending.

import { backend, onJobFinished, onJobProgress, pickSaveFile } from "../../backend";
import { toShape, type Store } from "../../model/doc";
import { IDENTITY, mul, rectPath, scaleAt } from "../../model/geom";
import type { Destination, JobResult, Move, PortInfo, Rect, RelayInfo, Shape } from "../../model/types";
import { DEFAULT_SETTINGS, persist, settings, type OutputKind } from "../../settings";
import { fmtMm } from "../actions";
import { checkbox, errorMessage, h, modal, numberField, select, toast } from "../dom";

const BAUDS = [2400, 4800, 9600, 19200, 38400, 57600, 115200];

/** Small asymmetric test: a square with a triangle pointing along the feed, near its top. */
function testShapes(): Shape[] {
  const tri = [
    { t: "M" as const, x: 6, y: 5 },
    { t: "L" as const, x: 19, y: 9 },
    { t: "L" as const, x: 6, y: 13 },
    { t: "Z" as const },
  ];
  return [
    { path: rectPath(0, 0, 25, 25), transform: [1, 0, 0, 1, 0, 0], fillRule: "nonzero" },
    { path: tri, transform: [1, 0, 0, 1, 0, 0], fillRule: "nonzero" },
  ];
}

interface Seg {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
  down: boolean;
  t0: number;
  t1: number;
}

/** Turns moves into timed segments for the simulation (travel counts half, as it is faster). */
function timeline(moves: Move[]): { segs: Seg[]; total: number } {
  const segs: Seg[] = [];
  let t = 0;
  for (const m of moves) {
    for (let i = 1; i < m.pts.length; i++) {
      const [x0, y0] = m.pts[i - 1];
      const [x1, y1] = m.pts[i];
      const len = Math.hypot(x1 - x0, y1 - y0) * (m.down ? 1 : 0.5);
      segs.push({ x0, y0, x1, y1, down: m.down, t0: t, t1: t + len });
      t += len;
    }
  }
  return { segs, total: t };
}

function fmtDuration(s: number): string {
  if (s < 60) return `${Math.max(1, Math.round(s))} s`;
  const m = Math.floor(s / 60);
  const r = Math.round(s % 60);
  return r ? `${m} min ${r} s` : `${m} min`;
}

export function openCutDialog(store: Store): void {
  const hasSelection = store.selection.length > 0;
  let onlySelection = hasSelection;
  let testMode = false;
  let job: JobResult | null = null;
  let jobShapes: Shape[] = [];
  let rebuildTimer = 0;
  let sending = false;
  let ports: PortInfo[] = [];

  const s = settings;

  // ---- preview

  const canvas = h("canvas", { class: "cut-preview" });
  const stats = h("div", { class: "cut-stats" }, "Préparation…");
  const scrub = h("input", { type: "range", min: 0, max: 1000, value: 1000 });
  const playBtn = h("button", { class: "icon", title: "Simuler la découpe" }, "▶");
  const speedSel = select<string>(null, [["1", "×1"], ["4", "×4"], ["16", "×16"], ["64", "×64"]], "16", () => {}).select;
  let tl = timeline([]);
  let simT = Infinity;
  let playing = false;
  let lastFrame = 0;

  const draw = () => {
    const ctx = canvas.getContext("2d")!;
    const dpr = window.devicePixelRatio || 1;
    const W = canvas.clientWidth;
    const H = canvas.clientHeight;
    if (canvas.width !== Math.round(W * dpr)) {
      canvas.width = Math.round(W * dpr);
      canvas.height = Math.round(H * dpr);
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.fillStyle = "#f7f8fa";
    ctx.fillRect(0, 0, W, H);
    if (!job) return;
    const b = job.stats.bounds;
    const origin = originPoint(b);
    const minX = Math.min(b.x, origin[0]);
    const minY = Math.min(b.y, origin[1]);
    const maxX = Math.max(b.x + b.w, origin[0]);
    const maxY = Math.max(b.y + b.h, origin[1]);
    const pad = 24;
    const k = Math.min((W - 2 * pad) / Math.max(1, maxX - minX), (H - 2 * pad) / Math.max(1, maxY - minY));
    const ox = pad + (W - 2 * pad - (maxX - minX) * k) / 2 - minX * k;
    const oy = pad + (H - 2 * pad - (maxY - minY) * k) / 2 - minY * k;
    ctx.setTransform(dpr * k, 0, 0, dpr * k, dpr * ox, dpr * oy);
    const px = 1 / k;

    // Design, faint. The moves show where the knife really goes, mirror included,
    // so mirror the design the same way.
    ctx.fillStyle = "rgba(43,47,54,0.08)";
    const flip = s.cut.mirror ? scaleAt(-1, 1, b.x + b.w / 2, 0) : IDENTITY;
    for (const sh of jobShapes) {
      const p = new Path2D();
      p.addPath(pathOf(sh), new DOMMatrix(mul(flip, sh.transform)));
      ctx.fill(p, sh.fillRule);
    }

    const t = Math.min(simT, tl.total);
    // Whole path, light; then what has been "cut" so far, dark.
    for (const pass of [0, 1]) {
      ctx.beginPath();
      ctx.lineWidth = (pass ? 1.4 : 1) * px;
      ctx.strokeStyle = pass ? "#1d2230" : "#c3c9d4";
      for (const sg of tl.segs) {
        if (!sg.down) continue;
        if (pass && sg.t0 >= t) break;
        let x1 = sg.x1;
        let y1 = sg.y1;
        if (pass && sg.t1 > t) {
          const f = (t - sg.t0) / (sg.t1 - sg.t0);
          x1 = sg.x0 + (sg.x1 - sg.x0) * f;
          y1 = sg.y0 + (sg.y1 - sg.y0) * f;
        }
        ctx.moveTo(sg.x0, sg.y0);
        ctx.lineTo(x1, y1);
      }
      ctx.stroke();
    }
    // Travel moves.
    ctx.beginPath();
    ctx.setLineDash([3 * px, 3 * px]);
    ctx.lineWidth = px;
    ctx.strokeStyle = "rgba(255,138,61,0.7)";
    for (const sg of tl.segs) {
      if (sg.down || sg.t0 >= t) continue;
      ctx.moveTo(sg.x0, sg.y0);
      ctx.lineTo(sg.x1, sg.y1);
    }
    ctx.stroke();
    ctx.setLineDash([]);

    // Knife position during the simulation.
    if (t < tl.total) {
      const sg = tl.segs.find((x) => x.t1 >= t);
      if (sg) {
        const f = sg.t1 > sg.t0 ? (t - sg.t0) / (sg.t1 - sg.t0) : 0;
        ctx.beginPath();
        ctx.arc(sg.x0 + (sg.x1 - sg.x0) * f, sg.y0 + (sg.y1 - sg.y0) * f, 4 * px, 0, Math.PI * 2);
        ctx.fillStyle = sg.down ? "#d7263d" : "#ff8a3d";
        ctx.fill();
      }
    }

    // Machine origin.
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const sx = ox + origin[0] * k;
    const sy = oy + origin[1] * k;
    ctx.strokeStyle = "#ff8a3d";
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(sx - 8, sy);
    ctx.lineTo(sx + 8, sy);
    ctx.moveTo(sx, sy - 8);
    ctx.lineTo(sx, sy + 8);
    ctx.stroke();
    ctx.fillStyle = "#b4561c";
    ctx.font = "11px system-ui, sans-serif";
    ctx.fillText("origine", sx + 6, sy < 20 ? sy + 14 : sy - 6);
    ctx.fillStyle = "#7b8494";
    ctx.fillText("avance du rouleau →", 8, H - 8);
  };

  const pathCache = new WeakMap<Shape, Path2D>();
  function pathOf(sh: Shape): Path2D {
    let p = pathCache.get(sh);
    if (!p) {
      p = new Path2D();
      for (const sg of sh.path) {
        if (sg.t === "M") p.moveTo(sg.x, sg.y);
        else if (sg.t === "L") p.lineTo(sg.x, sg.y);
        else if (sg.t === "C") p.bezierCurveTo(sg.x1, sg.y1, sg.x2, sg.y2, sg.x, sg.y);
        else p.closePath();
      }
      pathCache.set(sh, p);
    }
    return p;
  }

  function originPoint(b: Rect): [number, number] {
    const m = s.machine;
    const R = s.cut.fromOrigin
      ? b
      : { x: 0, y: 0, w: b.x + b.w, h: store.doc.mediaWidth };
    return [R.x + (m.flipX ? R.w : 0), R.y + (m.flipY ? R.h : 0)];
  }

  const tick = (now: number) => {
    if (!playing) return;
    const dt = lastFrame ? (now - lastFrame) / 1000 : 0;
    lastFrame = now;
    // Simulated speed: the cutter's speed setting (cm/s → mm/s) times the chosen factor.
    simT += dt * (s.machine.speed || 20) * 10 * Number(speedSel.value);
    if (simT >= tl.total) {
      simT = Infinity;
      playing = false;
      playBtn.textContent = "▶";
    }
    scrub.value = String(tl.total ? Math.round((Math.min(simT, tl.total) / tl.total) * 1000) : 1000);
    draw();
    if (playing) requestAnimationFrame(tick);
  };
  playBtn.addEventListener("click", () => {
    playing = !playing;
    playBtn.textContent = playing ? "⏸" : "▶";
    if (playing) {
      if (simT >= tl.total) simT = 0;
      lastFrame = 0;
      requestAnimationFrame(tick);
    }
  });
  scrub.addEventListener("input", () => {
    playing = false;
    playBtn.textContent = "▶";
    simT = (Number(scrub.value) / 1000) * tl.total;
    draw();
  });

  // ---- job building

  const shapesToCut = (): Shape[] => {
    if (testMode) return testShapes();
    const objs = onlySelection ? store.selected() : store.doc.objects;
    return objs.map(toShape);
  };

  const rebuild = () => {
    clearTimeout(rebuildTimer);
    rebuildTimer = window.setTimeout(async () => {
      persist();
      jobShapes = shapesToCut();
      if (jobShapes.length === 0) {
        job = null;
        stats.textContent = "Rien à découper.";
        sendBtn.disabled = true;
        draw();
        return;
      }
      try {
        job = await backend.buildJob({
          shapes: jobShapes,
          cut: s.cut,
          machine: { ...s.machine, mediaWidth: store.doc.mediaWidth },
        });
        tl = timeline(job.moves);
        simT = Infinity;
        scrub.value = "1000";
        const st = job.stats;
        stats.textContent =
          `${testMode ? "Test · " : ""}${st.pathCount} tracé${st.pathCount > 1 ? "s" : ""} · ` +
          `${fmtMm(st.cutLength / 1000)} m de coupe · ≈ ${fmtDuration(st.estimatedSeconds)} · ` +
          `${fmtMm(st.bounds.w)} × ${fmtMm(st.bounds.h)} mm`;
        stats.classList.toggle("warn", st.bounds.h > store.doc.mediaWidth);
        if (st.bounds.h > store.doc.mediaWidth) stats.textContent += " — plus large que la laize !";
        sendBtn.disabled = sending;
      } catch (e) {
        job = null;
        stats.textContent = errorMessage(e);
        sendBtn.disabled = true;
      }
      draw();
    }, 120);
  };

  // ---- settings form

  const num = (label: string, get: () => number, set: (v: number) => void, unit: string, step = 0.05, title?: string) =>
    numberField({
      label,
      value: get(),
      step,
      unit,
      title,
      onCommit: (v) => {
        set(v);
        rebuild();
      },
    }).row;
  const chk = (label: string, get: () => boolean, set: (v: boolean) => void, title?: string) =>
    checkbox(label, get(), (v) => {
      set(v);
      rebuild();
    }, title);

  const what = h(
    "div",
    { class: "radio-row" },
    radio("what", "Tout le document", !onlySelection, () => {
      onlySelection = false;
      testMode = false;
      rebuild();
    }),
    radio("what", `Sélection (${store.selection.length})`, onlySelection, () => {
      onlySelection = true;
      testMode = false;
      rebuild();
    }, !hasSelection),
    radio("what", "Test (carré 25 mm)", false, () => {
      testMode = true;
      rebuild();
    }),
  );

  const blade = h(
    "fieldset",
    {},
    h("legend", {}, "Lame"),
    num("Décalage lame", () => s.cut.bladeOffset, (v) => (s.cut.bladeOffset = Math.max(0, v)), "mm", 0.05,
      "Distance entre la pointe et l'axe de la lame : 0,25 pour une lame 45°, 0,5 pour 60°. 0 = pas de compensation."),
    num("Dépassement", () => s.cut.overcut, (v) => (s.cut.overcut = Math.max(0, v)), "mm", 0.5,
      "Les formes fermées sont recoupées sur cette longueur pour bien se détacher."),
    num("Passes", () => s.cut.passes, (v) => (s.cut.passes = Math.min(10, Math.max(1, Math.round(v)))), "×", 1),
  );

  const options = h(
    "fieldset",
    {},
    h("legend", {}, "Options"),
    chk("Optimiser l'ordre (intérieurs d'abord)", () => s.cut.optimize, (v) => (s.cut.optimize = v)),
    chk("Miroir (flex thermocollant)", () => s.cut.mirror, (v) => (s.cut.mirror = v)),
    chk("Partir de l'origine du plotter", () => s.cut.fromOrigin, (v) => (s.cut.fromOrigin = v),
      "Coché : le dessin commence là où la lame a été placée. Décoché : la position sur la laize est respectée."),
  );

  // Output.
  const portSel = h("select", {});
  const printerSel = h("select", {});
  const fillPorts = async () => {
    try {
      ports = await backend.listPorts();
    } catch {
      ports = [];
    }
    if (!s.output.port || !ports.some((p) => p.name === s.output.port)) {
      // Prefer a USB-serial chip over other ports.
      const guess = ports.find((p) => /ch34|ch91|pl2303|cp210|ftdi|usb.?serial/i.test(`${p.description} ${p.manufacturer}`));
      if (guess) s.output.port = guess.name;
    }
    // Never pick an unknown port by ourselves: it could be another device (a Loupedeck, a modem…).
    if (s.output.port && !ports.some((p) => p.name === s.output.port)) s.output.port = "";
    portSel.replaceChildren(
      h("option", { value: "", selected: !s.output.port }, ports.length ? "— Choisir le port du plotter —" : "Aucun port série trouvé"),
      ...ports.map((p) =>
        h("option", { value: p.name, selected: p.name === s.output.port }, `${p.name}${p.description ? ` — ${p.description}` : ""}`),
      ),
    );
  };
  portSel.addEventListener("change", () => {
    s.output.port = portSel.value;
    persist();
  });
  const fillPrinters = async () => {
    let names: string[] = [];
    try {
      names = await backend.listPrinters();
    } catch {
      names = [];
    }
    printerSel.replaceChildren(
      ...(names.length
        ? names.map((n) => h("option", { value: n, selected: n === s.output.printer }, n))
        : [h("option", { value: "" }, "Aucune imprimante")]),
    );
    if (!names.includes(s.output.printer) && names[0]) s.output.printer = names[0];
  };
  printerSel.addEventListener("change", () => {
    s.output.printer = printerSel.value;
    persist();
  });

  const serialBox = h(
    "div",
    {},
    h("label", { class: "field" }, h("span", {}, "Port"), portSel,
      h("button", { class: "icon", title: "Actualiser la liste", onclick: () => void fillPorts() }, "↻")),
  );
  // Line settings: for a local port, or for the cutter wired to the Pi.
  const lineBox = h(
    "div",
    {},
    select<string>("Débit", BAUDS.map((b) => [String(b), `${b} bauds`]), String(s.output.baud), (v) => {
      s.output.baud = Number(v);
      persist();
    }).row,
    select("Contrôle de flux", [["hardware", "Matériel (RTS/CTS)"], ["software", "Logiciel (XON/XOFF)"], ["none", "Aucun"]], s.output.flow, (v) => {
      s.output.flow = v;
      persist();
    }).row,
  );
  const printerBox = h("div", {}, h("label", { class: "field" }, h("span", {}, "Imprimante"), printerSel));
  const fileBox = h("p", { class: "hint" }, "Le fichier .plt est enregistré là où vous le choisirez à l'envoi.");

  // Raspberry Pi relay.
  const hostInput = h("input", {
    type: "text",
    value: s.output.relayHost,
    placeholder: "ki-plotter.local ou 192.168.1.20",
    spellcheck: false,
  });
  const relayState = h("p", { class: "hint" }, "");
  let relayCheck = 0;
  const setRelayState = (text: string, kind: "ok" | "err" | "" = "") => {
    relayState.textContent = text;
    relayState.className = `hint relay-${kind || "info"}`;
  };
  const checkRelay = async () => {
    const host = s.output.relayHost.trim();
    const gen = ++relayCheck;
    if (!host) {
      setRelayState("Indiquez l'adresse du Raspberry Pi, ou cliquez sur Rechercher.");
      return;
    }
    setRelayState("Connexion…");
    try {
      const st = await backend.relayStatus(host, s.output.relayPort);
      if (gen !== relayCheck) return;
      const queue =
        st.mode === "queue"
          ? ` Les travaux attendent sur son écran qu'on lance la découpe${st.queued ? ` (${st.queued} en attente)` : ""}.`
          : " Les travaux sont coupés dès leur arrivée.";
      if (st.deviceError) setRelayState(`Relais « ${st.name} » joignable, mais : ${st.deviceError}`, "err");
      else if (st.busy) setRelayState(`Relais « ${st.name} » : une découpe est en cours.${queue}`, "err");
      else setRelayState(`✓ Relais « ${st.name} » prêt — plotter sur ${st.device}.${queue}`, "ok");
    } catch (e) {
      if (gen !== relayCheck) return;
      setRelayState(errorMessage(e), "err");
    }
  };
  const useRelay = (r: RelayInfo) => {
    s.output.relayHost = r.host;
    s.output.relayPort = r.port;
    hostInput.value = r.host;
    persist();
    void checkRelay();
  };
  const found = h("div", { class: "relay-found" });
  const discover = async () => {
    setRelayState("Recherche sur le réseau…");
    found.replaceChildren();
    let relays: RelayInfo[] = [];
    try {
      relays = await backend.discoverRelays();
    } catch (e) {
      setRelayState(errorMessage(e), "err");
      return;
    }
    if (relays.length === 0) {
      setRelayState("Aucun relais trouvé. Le Pi est-il allumé, sur le même réseau, avec le service installé ?", "err");
    } else if (relays.length === 1) {
      useRelay(relays[0]);
    } else {
      setRelayState(`${relays.length} relais trouvés :`);
      found.replaceChildren(
        ...relays.map((r) => h("button", { onclick: () => (found.replaceChildren(), useRelay(r)) }, `${r.name} (${r.host})`)),
      );
    }
  };
  hostInput.addEventListener("change", () => {
    s.output.relayHost = hostInput.value.trim();
    persist();
    void checkRelay();
  });
  const networkBox = h(
    "div",
    {},
    h("label", { class: "field" }, h("span", {}, "Raspberry Pi"), hostInput,
      h("button", { class: "icon", title: "Chercher le relais sur le réseau", onclick: () => void discover() }, "Rechercher")),
    relayState,
    found,
    h("p", { class: "hint" }, "Le port, le débit et le contrôle de flux du plotter se règlent sur l'écran du Pi (ou sa page web)."),
  );

  let relayChecked = false;
  const showOutput = () => {
    const k = s.output.kind;
    serialBox.hidden = k !== "serial";
    lineBox.hidden = k !== "serial";
    printerBox.hidden = k !== "printer";
    fileBox.hidden = k !== "file";
    networkBox.hidden = k !== "network";
    if (k === "network" && !relayChecked) {
      relayChecked = true;
      void (s.output.relayHost ? checkRelay() : discover());
    }
  };
  const output = h(
    "fieldset",
    {},
    h("legend", {}, "Sortie"),
    select<OutputKind>(
      "Envoyer vers",
      [
        ["serial", "Port série (USB)"],
        ["network", "Raspberry Pi (réseau)"],
        ["printer", "Imprimante Windows"],
        ["file", "Fichier .plt"],
      ],
      s.output.kind,
      (v) => {
        s.output.kind = v;
        persist();
        showOutput();
      },
    ).row,
    serialBox,
    networkBox,
    lineBox,
    printerBox,
    fileBox,
  );

  const m = s.machine;
  const machine = h(
    "details",
    { class: "machine" },
    h("summary", {}, "Réglages machine"),
    select("Protocole", [["hpgl", "HPGL"], ["dmpl", "DMPL (expérimental)"]], m.protocol, (v) => {
      m.protocol = v;
      rebuild();
    }).row,
    num("Unités par mm", () => m.unitsPerMm, (v) => (m.unitsPerMm = v > 0 ? v : 40), "", 1),
    num("Échelle X", () => m.scaleX, (v) => (m.scaleX = v > 0 ? v : 1), "×", 0.001, "Correction de longueur : longueur voulue ÷ longueur mesurée"),
    num("Échelle Y", () => m.scaleY, (v) => (m.scaleY = v > 0 ? v : 1), "×", 0.001),
    chk("Inverser Y", () => m.flipY, (v) => (m.flipY = v)),
    chk("Inverser X", () => m.flipX, (v) => (m.flipX = v)),
    chk("Échanger X et Y", () => m.swapXY, (v) => (m.swapXY = v)),
    chk("Envoyer vitesse et pression", () => m.sendSpeedForce, (v) => (m.sendSpeedForce = v),
      "Sinon, ce sont les réglages du panneau du plotter qui s'appliquent."),
    num("Vitesse", () => m.speed, (v) => (m.speed = Math.max(1, v)), "cm/s", 1),
    num("Pression", () => m.force, (v) => (m.force = Math.max(1, v)), "g", 5),
    num("Avance après", () => m.feedAfter, (v) => (m.feedAfter = Math.max(0, v)), "mm", 10,
      "Après la découpe, avancer le vinyle de cette longueur au-delà du dessin (0 = retour à l'origine)."),
    num("Points par commande", () => m.maxPointsPerCommand, (v) => (m.maxPointsPerCommand = Math.max(0, Math.round(v))), "", 1,
      "0 = illimité. Réduire si le plotter s'arrête sur de longs tracés."),
    num("Taille des blocs", () => s.output.chunkSize, (v) => (s.output.chunkSize = Math.max(16, Math.round(v))), "octets", 64),
    num("Pause entre blocs", () => s.output.chunkDelayMs, (v) => (s.output.chunkDelayMs = Math.max(0, Math.round(v))), "ms", 5),
    h("button", {
      class: "link",
      onclick: () => {
        Object.assign(s.machine, structuredClone(DEFAULT_SETTINGS.machine));
        persist();
        dlg.close();
        openCutDialog(store);
      },
    }, "Rétablir les réglages par défaut"),
  );

  // ---- footer and sending

  const progress = h("progress", { max: 1, value: 0 });
  const progressText = h("span", {}, "");
  const progressRow = h("div", { class: "send-progress", hidden: true }, progress, progressText);
  const stopBtn = h("button", { class: "danger", hidden: true, onclick: () => void backend.cancelJob() }, "Arrêter");
  const sendBtn = h("button", { class: "primary", disabled: true }, "Envoyer au plotter");
  const saveBtn = h("button", { title: "Enregistrer le fichier HPGL/DMPL sans l'envoyer" }, "Enregistrer .plt…");

  const savePlt = async () => {
    if (!job) return;
    const path = await pickSaveFile("Enregistrer le travail", [{ name: "HPGL", extensions: ["plt"] }], "decoupe.plt");
    if (!path) return;
    try {
      await backend.writeTextFile(path, job.data);
      toast("Fichier enregistré.");
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };
  saveBtn.addEventListener("click", () => void savePlt());

  /** What the Pi's screen calls this job: the project, else the first text. */
  const jobName = (): string => {
    if (testMode) return "Coupe test 25 mm";
    const file = store.filePath?.split(/[\\/]/).pop()?.replace(/\.[^.]+$/, "");
    const objs = onlySelection ? store.selected() : store.doc.objects;
    const text = objs.find((o) => o.kind === "text");
    const base = file || (text?.kind === "text" ? text.text.replace(/\s+/g, " ").trim().slice(0, 40) : "") || "Sans titre";
    return onlySelection && file ? `${base} (sélection)` : base;
  };

  const send = async () => {
    if (!job || sending) return;
    let destination: Destination;
    if (s.output.kind === "serial") {
      if (!s.output.port) {
        toast("Choisissez le port série du plotter.", "error");
        return;
      }
      destination = { kind: "serial", port: s.output.port, baud: s.output.baud, flow: s.output.flow };
    } else if (s.output.kind === "network") {
      if (!s.output.relayHost.trim()) {
        toast("Indiquez l'adresse du Raspberry Pi, ou cliquez sur Rechercher.", "error");
        return;
      }
      destination = {
        kind: "network",
        host: s.output.relayHost.trim(),
        port: s.output.relayPort,
        // The relay's own settings (its screen) decide the line.
        device: "",
        baud: 0,
        flow: null,
      };
    } else if (s.output.kind === "printer") {
      if (!s.output.printer) {
        toast("Choisissez l'imprimante du plotter.", "error");
        return;
      }
      destination = { kind: "printer", name: s.output.printer };
    } else {
      const path = await pickSaveFile("Enregistrer le travail", [{ name: "HPGL", extensions: ["plt"] }], "decoupe.plt");
      if (!path) return;
      destination = { kind: "file", path };
    }
    sending = true;
    sendBtn.disabled = true;
    saveBtn.disabled = true;
    stopBtn.hidden = false;
    progressRow.hidden = false;
    progress.value = 0;
    progressText.textContent = "Envoi…";
    const unProgress = await onJobProgress((p) => {
      progress.value = p.total ? p.sent / p.total : 1;
      progressText.textContent = `${Math.round((p.total ? p.sent / p.total : 1) * 100)} %`;
    });
    const unFinished = await onJobFinished((f) => {
      unProgress();
      unFinished();
      sending = false;
      sendBtn.disabled = false;
      saveBtn.disabled = false;
      stopBtn.hidden = true;
      if (f.queued) {
        progress.value = 1;
        progressText.textContent = "En attente sur le Pi ✓";
        toast(
          f.position > 1
            ? `Travail en attente sur le Pi (${f.position} dans la file). Lancez la découpe depuis son écran.`
            : "Travail en attente sur le Pi. Lancez la découpe depuis son écran, une fois le vinyle en place.",
        );
      } else if (f.ok) {
        progress.value = 1;
        progressText.textContent = "Envoyé ✓";
        toast("Travail envoyé au plotter.");
      } else if (f.cancelled) {
        progressText.textContent = "Arrêté";
        toast("Envoi arrêté. Le plotter peut finir ce qu'il a déjà reçu.");
      } else {
        progressText.textContent = "Erreur";
        toast(f.error ?? "Erreur d'envoi.", "error");
      }
    });
    try {
      await backend.sendJob({
        destination,
        data: job.data,
        chunkSize: s.output.chunkSize,
        chunkDelayMs: s.output.chunkDelayMs,
        info: {
          name: jobName(),
          widthMm: job.stats.bounds.w,
          heightMm: job.stats.bounds.h,
          estimatedSeconds: job.stats.estimatedSeconds,
          settings: { cut: s.cut, machine: { ...s.machine, mediaWidth: store.doc.mediaWidth } },
        },
      });
    } catch (e) {
      unProgress();
      unFinished();
      sending = false;
      sendBtn.disabled = false;
      saveBtn.disabled = false;
      stopBtn.hidden = true;
      progressText.textContent = "Erreur";
      toast(errorMessage(e), "error");
    }
  };
  sendBtn.addEventListener("click", () => void send());

  const body = h(
    "div",
    { class: "cut-layout" },
    h("div", { class: "cut-left" }, canvas, h("div", { class: "sim-row" }, playBtn, scrub, speedSel), stats),
    h("div", { class: "cut-right" }, what, blade, options, output, machine),
  );
  const footer = h(
    "div",
    { class: "footer-row" },
    saveBtn,
    progressRow,
    h("span", { class: "spacer" }),
    stopBtn,
    h("button", { onclick: () => dlg.close() }, "Fermer"),
    sendBtn,
  );
  const dlg = modal("Découper", body, footer, "cut-dialog");
  dlg.addEventListener("cancel", (e) => {
    if (sending) e.preventDefault();
  });
  new ResizeObserver(() => draw()).observe(canvas);
  showOutput();
  void fillPorts();
  void fillPrinters();
  rebuild();
}

function radio(name: string, label: string, checked: boolean, onSelect: () => void, disabled = false): HTMLElement {
  const input = h("input", { type: "radio", name, checked, disabled });
  input.addEventListener("change", () => input.checked && onSelect());
  return h("label", { class: `check${disabled ? " disabled" : ""}` }, input, h("span", {}, label));
}
