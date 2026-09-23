export type Session = {
  id: string;
  source: string;
  path: string;
  project: string | null;
  title: string | null;
  started_at: string | null;
  updated_at: string | null;
  message_count: number;
  model: string | null;
  git_branch: string | null;
  input_tokens: number;
  output_tokens: number;
  /** false once the agent has deleted its own log; Receipts still has a copy. */
  available: boolean;
};

export type Message = {
  role: "user" | "assistant" | "system";
  kind: "text" | "thinking" | "tool_use" | "tool_result";
  text: string;
  tool_name: string | null;
  tool_input: unknown;
  is_error: boolean;
  timestamp: string | null;
};

export type Facet = { name: string; count: number };

export type SearchHit = {
  session_id: string;
  idx: number;
  role: string;
  snippet: string;
  title: string | null;
  source: string;
  project: string | null;
  updated_at: string | null;
};

export type Status = {
  version: string;
  indexing: boolean;
  /** Counts scans that changed data. */
  generation: number;
  /** Unix time in milliseconds when the last scan finished. */
  last_indexed_at: number | null;
  sessions: number;
  db_path: string;
  /** SSH target when `receipts remote` started this server, such as "devbox". */
  remote?: string | null;
};

async function get<T>(path: string, params: Record<string, string | undefined> = {}): Promise<T> {
  const qs = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) if (v) qs.set(k, v);
  const res = await fetch(`${path}${qs.size ? `?${qs}` : ""}`);
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return res.json();
}

export const api = {
  status: () => get<Status>("/api/status"),
  facets: () => get<{ sources: Facet[]; projects: Facet[] }>("/api/facets"),
  sessions: (source?: string, project?: string) =>
    get<Session[]>("/api/sessions", { source, project, limit: "500" }),
  session: (id: string) =>
    get<{ session: Session; messages: Message[] }>(`/api/sessions/${encodeURIComponent(id)}`),
  search: (q: string, source?: string) => get<SearchHit[]>("/api/search", { q, source }),
  reindex: () => fetch("/api/reindex", { method: "POST" }),
};
