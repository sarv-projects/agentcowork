/**
 * P59.1 / P59.11 — preference ladder in code, not a prompt.
 * Office file → office engines; http(s)/file URL → the Rust Browse engine; else Desktop.
 * Mirrors `agentcowork-core::route_work_surface`.
 */

export type WorkSurface = "office" | "browse" | "desktop";

const OFFICE_EXT = [
  ".docx",
  ".xlsx",
  ".pptx",
  ".pdf",
  ".doc",
  ".xls",
  ".ppt",
  ".odt",
  ".ods",
  ".odp",
];

export function routeWorkSurface(target: string): WorkSurface {
  const t = target.trim();
  const lower = t.toLowerCase();
  const path = lower.split(/[?#]/)[0] ?? lower;
  if (OFFICE_EXT.some((e) => path.endsWith(e))) return "office";
  if (lower.startsWith("http://") || lower.startsWith("https://") || lower.startsWith("file://")) {
    return "browse";
  }
  return "desktop";
}

export function refuseDesktopIfWrongSurface(
  toolId: string,
  args: Record<string, unknown>,
): { ok: true } | { ok: false; surface: WorkSurface; error: string } {
  if (!toolId.startsWith("desktop.")) return { ok: true };
  const target =
    (typeof args.target === "string" && args.target) ||
    (typeof args.path === "string" && args.path) ||
    (typeof args.url === "string" && args.url) ||
    "";
  if (!target) return { ok: true };
  const surface = routeWorkSurface(target);
  if (surface === "office") {
    return {
      ok: false,
      surface,
      error: "use office engines for this file — CUA is last on the ladder",
    };
  }
  if (surface === "browse") {
    return {
      ok: false,
      surface,
      error: "use the Browse engine's CDP for this URL — CUA is last on the ladder",
    };
  }
  return { ok: true };
}
