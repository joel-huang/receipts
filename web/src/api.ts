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
  /** The chat view got a shortened copy. `api.message` returns the full message. */
  truncated?: boolean;
};

/** Part of a chat: the messages from `offset` on. `total` counts the whole chat. */
export type SessionPage = { session: Session; total: number; offset: number; messages: Message[] };

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
  /** Messages from position `after` on, with long tool output shortened. */
  session: (id: string, after = 0) =>
    get<SessionPage>(`/api/sessions/${encodeURIComponent(id)}`, { after: after ? String(after) : undefined }),
  /** One message in full. */
  message: (id: string, idx: number) => get<Message>(`/api/sessions/${encodeURIComponent(id)}/messages/${idx}`),
  search: (q: string, source?: string) => get<SearchHit[]>("/api/search", { q, source }),
  reindex: () => fetch("/api/reindex", { method: "POST" }),
};
