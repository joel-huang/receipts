export function relativeTime(iso: string | null): string {
  if (!iso) return "";
  const then = new Date(iso).getTime();
  const secs = Math.round((Date.now() - then) / 1000);
  if (secs < 60) return "just now";
  const mins = Math.round(secs / 60);
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.round(hours / 24);
  if (days < 7) return `${days}d ago`;
  return new Date(iso).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

export function shortProject(path: string | null): string {
  if (!path) return "—";
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.slice(-2).join("/");
}

export function compactNumber(n: number): string {
  return Intl.NumberFormat(undefined, { notation: "compact", maximumFractionDigits: 1 }).format(n);
}

/** One-line summary of a tool call for the collapsed header. */
export function toolSummary(name: string | null, input: unknown): string {
  if (typeof input === "string") {
    // Freeform tool input, e.g. Codex apply_patch: show the first file header.
    const lines = input.split("\n").filter((l) => l.trim() && !l.startsWith("*** Begin"));
    return lines[0] ?? "";
  }
  if (!input || typeof input !== "object") return "";
  const i = input as Record<string, unknown>;
  const pick = (...keys: string[]) => keys.map((k) => i[k]).find((v) => typeof v === "string") as string | undefined;
  const cmd = i.command;
  if (Array.isArray(cmd)) return cmd.join(" ");
  return (
    pick("command", "cmd", "file_path", "path", "pattern", "url", "query", "description", "prompt") ??
    (name === "apply_patch" && typeof i.input === "string" ? toolSummary(name, i.input) : "")
  );
}
