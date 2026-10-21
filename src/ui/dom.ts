// Tiny DOM helpers for building panels and dialogs without a framework.

type Attrs = Record<string, string | number | boolean | undefined | null | ((e: Event) => void)>;
type Child = Node | string | null | undefined | false;

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Attrs = {},
  ...children: (Child | Child[])[]
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === undefined || v === null || v === false) continue;
    if (typeof v === "function") el.addEventListener(k.replace(/^on/, ""), v as EventListener);
    else if (k === "class") el.className = String(v);
    else if (k === "html") el.innerHTML = String(v);
    else if (v === true) el.setAttribute(k, "");
    else el.setAttribute(k, String(v));
  }
  for (const c of children.flat()) {
    if (c === null || c === undefined || c === false) continue;
    el.append(c);
  }
  return el;
}

export function $(sel: string, root: ParentNode = document): HTMLElement {
  const el = root.querySelector<HTMLElement>(sel);
  if (!el) throw new Error(`Élément introuvable : ${sel}`);
  return el;
}

/** A labelled number input bound to getter/setter; `onCommit` fires on change/Enter. */
export function numberField(opts: {
  label: string;
  value: number;
  step?: number;
  min?: number;
  max?: number;
  unit?: string;
  title?: string;
  onCommit: (v: number) => void;
}): { row: HTMLElement; input: HTMLInputElement } {
  const input = h("input", {
    type: "number",
    step: opts.step ?? 0.1,
    min: opts.min,
    max: opts.max,
    value: round(opts.value),
    title: opts.title,
  });
  const commit = () => {
    const v = parseFloat(input.value.replace(",", "."));
    if (Number.isFinite(v)) opts.onCommit(v);
  };
  input.addEventListener("change", commit);
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      commit();
      input.select();
    }
  });
  const row = h("label", { class: "field" }, h("span", {}, opts.label), input, opts.unit ? h("em", {}, opts.unit) : null);
  return { row, input };
}

export function checkbox(label: string, checked: boolean, onChange: (v: boolean) => void, title?: string): HTMLElement {
  const input = h("input", { type: "checkbox", checked });
  input.addEventListener("change", () => onChange(input.checked));
  return h("label", { class: "check", title }, input, h("span", {}, label));
}

export function select<T extends string>(
  label: string | null,
  options: [T, string][],
  value: T,
  onChange: (v: T) => void,
): { row: HTMLElement; select: HTMLSelectElement } {
  const sel = h("select", {}, ...options.map(([v, text]) => h("option", { value: v, selected: v === value }, text)));
  sel.addEventListener("change", () => onChange(sel.value as T));
  const row = label ? h("label", { class: "field" }, h("span", {}, label), sel) : sel;
  return { row, select: sel };
}

export function round(v: number, digits = 2): string {
  if (!Number.isFinite(v)) return "";
  const k = 10 ** digits;
  return String(Math.round(v * k) / k);
}

let toastTimer = 0;
export function toast(message: string, kind: "info" | "error" = "info"): void {
  const el = $("#toast");
  el.textContent = message;
  el.className = `show ${kind}`;
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (el.className = ""), kind === "error" ? 6000 : 3000);
}

export function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

/** Opens a modal <dialog>; resolves when it closes. */
export function modal(title: string, body: HTMLElement, footer: HTMLElement, cls = ""): HTMLDialogElement {
  const dlg = h(
    "dialog",
    { class: `modal ${cls}` },
    h("header", {}, h("h2", {}, title), h("button", { class: "icon close", title: "Fermer", onclick: () => dlg.close() }, "×")),
    h("div", { class: "body" }, body),
    h("footer", {}, footer),
  );
  dlg.addEventListener("close", () => dlg.remove());
  document.body.append(dlg);
  dlg.showModal();
  return dlg;
}
