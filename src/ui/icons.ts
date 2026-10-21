// Stroke icons (24×24), drawn for this app.

const P: Record<string, string> = {
  select: '<path d="M6 3l12 9-5.5 1.2L16 20l-2.6 1.2-3.4-6.6L6 18z"/>',
  text: '<path d="M5 7V4h14v3M12 4v16M9 20h6"/>',
  rect: '<rect x="4" y="6" width="16" height="12" rx="1"/>',
  ellipse: '<ellipse cx="12" cy="12" rx="8.5" ry="6.5"/>',
  new: '<path d="M6 3h8l4 4v14H6zM14 3v4h4M12 11v6M9 14h6"/>',
  open: '<path d="M3 6h6l2 2h10v11H3zM3 10h18"/>',
  save: '<path d="M5 3h11l3 3v15H5zM8 3v5h7V3M8 21v-7h8v7"/>',
  import: '<path d="M12 3v12M7 10l5 5 5-5M4 20h16"/>',
  trace: '<rect x="3" y="4" width="18" height="16" rx="1"/><path d="M3 16l5-5 4 4 3-3 6 6"/><circle cx="15.5" cy="8.5" r="1.5"/>',
  undo: '<path d="M9 14L4 9l5-5M4 9h10a6 6 0 0 1 0 12h-3"/>',
  redo: '<path d="M15 14l5-5-5-5M20 9H10a6 6 0 0 0 0 12h3"/>',
  weed: '<rect x="3" y="4" width="18" height="16"/><path d="M3 12h4M17 12h4M10 8v8M14 8v8"/>',
  tile: '<rect x="3" y="3" width="8" height="8"/><rect x="13" y="3" width="8" height="8"/><rect x="3" y="13" width="8" height="8"/><rect x="13" y="13" width="8" height="8"/>',
  cut: '<path d="M4 20l8-8M12 12l6.5-6.5a2.1 2.1 0 0 1 3 3L15 15z"/><path d="M3 21h6" />',
  fit: '<path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/>',
  align: '<path d="M4 3v18M8 6h11v4H8zM8 14h7v4H8z"/>',
  wire: '<path d="M4 17c3-9 6-9 8-4s5 5 8-4"/><circle cx="4" cy="17" r="1"/><circle cx="20" cy="9" r="1"/>',
  grid: '<rect x="4" y="4" width="16" height="16"/><path d="M4 10h16M4 16h16M10 4v16M16 4v16"/>',
  zoomIn: '<circle cx="11" cy="11" r="7"/><path d="M21 21l-5-5M8 11h6M11 8v6"/>',
  zoomOut: '<circle cx="11" cy="11" r="7"/><path d="M21 21l-5-5M8 11h6"/>',
};

export function icon(name: keyof typeof P | string): string {
  return `<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">${P[name] ?? ""}</svg>`;
}
