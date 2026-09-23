import { useCallback, useEffect, useRef, useState } from "react";
import { api, type Facet, type SearchHit, type Session, type Status } from "./api";
import { compactNumber, relativeTime, shortProject } from "./format";
import { Transcript } from "./Transcript";

/** How often the app asks the server to rescan the agent logs. */
const REFRESH_MS = 5_000;

/** App state lives in the URL hash so every view is linkable and survives reloads. */
type UpdateParams = (patch: Record<string, string | null>, opts?: { replace?: boolean }) => void;

function useHashParams(): [URLSearchParams, UpdateParams] {
  const read = () => new URLSearchParams(window.location.hash.replace(/^#\/?\??/, ""));
  const [params, setParams] = useState(read);
  useEffect(() => {
    const onHash = () => setParams(read());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);
  const update = useCallback<UpdateParams>((patch, opts) => {
    const next = read();
    for (const [k, v] of Object.entries(patch)) (v ? next.set(k, v) : next.delete(k));
    // `replace` swaps the URL without a new history entry, so Back does not return to it.
    if (opts?.replace) window.location.replace(`#/?${next}`);
    else window.location.hash = `/?${next}`;
  }, []);
  return [params, update];
}

export function App() {
  const [params, update] = useHashParams();
  const source = params.get("source") ?? undefined;
  const project = params.get("project") ?? undefined;
  const query = params.get("q") ?? "";
  const selected = params.get("s");
  const focusIdx = params.get("m");

  const [status, setStatus] = useState<Status | null>(null);
  const [facets, setFacets] = useState<{ sources: Facet[]; projects: Facet[] }>({ sources: [], projects: [] });
  const [sessions, setSessions] = useState<Session[]>([]);
  const [hits, setHits] = useState<SearchHit[] | null>(null);
  const [draft, setDraft] = useState(query);
  const [error, setError] = useState<string | null>(null);

  // `version` changes when the index has new data. Views that fetch data depend on it.
  const [version, setVersion] = useState(0);
  const seen = useRef("");
  const busy = useRef(false);

  const poll = useCallback(async () => {
    const s = await api.status();
    setStatus(s);
    // The session count changes during the first scan, so the list fills in while it runs.
    const key = `${s.generation}:${s.sessions}`;
    if (key !== seen.current) {
      seen.current = key;
      setVersion((v) => v + 1);
    }
    return s;
  }, []);

  // Ask the server to rescan the agent logs, then poll until the scan finishes.
  const [refreshing, setRefreshing] = useState(false);
  const refresh = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    setRefreshing(true);
    try {
      await api.reindex();
      while ((await poll()).indexing) await new Promise((r) => setTimeout(r, 500));
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setRefreshing(false);
      busy.current = false;
    }
  }, [poll]);

  useEffect(() => {
    refresh();
    const timer = setInterval(refresh, REFRESH_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  useEffect(() => {
    api.facets().then(setFacets).catch((e) => setError(String(e)));
  }, [version]);

  useEffect(() => {
    api.sessions(source, project).then(setSessions).catch((e) => setError(String(e)));
  }, [source, project, version]);

  // On entry, open the most recently updated chat if the URL names no chat or search.
  const autoOpened = useRef(false);
  useEffect(() => {
    if (autoOpened.current || sessions.length === 0) return;
    autoOpened.current = true;
    if (!selected && !query) update({ s: sessions[0].id }, { replace: true });
  }, [sessions, selected, query, update]);

  useEffect(() => setDraft(query), [query]);

  useEffect(() => {
    if (!query.trim()) return setHits(null);
    api.search(query, source).then(setHits).catch((e) => setError(String(e)));
  }, [query, source, version]);

  // Debounce typing into the URL.
  useEffect(() => {
    if (draft === query) return;
    const t = setTimeout(() => update({ q: draft || null }), 250);
    return () => clearTimeout(t);
  }, [draft, query, update]);

  const total = facets.sources.reduce((n, f) => n + f.count, 0);

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <span className="logo">🧾</span> Receipts
          {status && <span className="muted version">v{status.version}</span>}
        </div>
        <nav>
          <div className="nav-label">Agents</div>
          <FacetButton label="All" count={total} active={!source} onClick={() => update({ source: null })} />
          {facets.sources.map((f) => (
            <FacetButton
              key={f.name}
              label={f.name}
              count={f.count}
              active={source === f.name}
              onClick={() => update({ source: f.name })}
              badge={f.name}
            />
          ))}
          <div className="nav-label">Projects</div>
          {project && <FacetButton label="All projects" active={false} onClick={() => update({ project: null })} />}
          <div className="projects">
            {facets.projects.map((f) => (
              <FacetButton
                key={f.name}
                label={shortProject(f.name)}
                title={f.name}
                count={f.count}
                active={project === f.name}
                onClick={() => update({ project: f.name })}
              />
            ))}
          </div>
        </nav>
        <footer className="sidebar-footer">
          <LastUpdated at={status?.last_indexed_at ?? null} />
          <RefreshButton refreshing={refreshing} onClick={refresh} />
        </footer>
      </aside>

      <section className="list">
        <div className="search">
          <input
            placeholder="Search every prompt, reply and tool call…"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            autoFocus
          />
        </div>
        {error && <div className="error" onClick={() => setError(null)}>{error}</div>}
        <div className="list-body">
          {hits
            ? hits.map((h) => (
                <button
                  key={`${h.session_id}:${h.idx}`}
                  className={`row ${selected === h.session_id && focusIdx === String(h.idx) ? "active" : ""}`}
                  onClick={() => update({ s: h.session_id, m: String(h.idx) })}
                >
                  <div className="row-top">
                    <span className={`badge ${h.source}`}>{h.source}</span>
                    <span className="row-title">{h.title ?? "(untitled)"}</span>
                  </div>
                  <div className="snippet">
                    <Highlighted text={h.snippet} />
                  </div>
                  <div className="row-meta">
                    {h.role} · {shortProject(h.project)} · {relativeTime(h.updated_at)}
                  </div>
                </button>
              ))
            : sessions.map((s) => (
                <button
                  key={s.id}
                  className={`row ${selected === s.id ? "active" : ""}`}
                  onClick={() => update({ s: s.id, m: null })}
                >
                  <div className="row-top">
                    <span className={`badge ${s.source}`}>{s.source}</span>
                    <span className="row-title">{s.title ?? "(untitled)"}</span>
                  </div>
                  <div className="row-meta">
                    {shortProject(s.project)} · {s.message_count} msgs ·{" "}
                    {compactNumber(s.input_tokens + s.output_tokens)} tok · {relativeTime(s.updated_at)}
                    {!s.available && <span title="The agent deleted this log; Receipts kept a copy."> · archived</span>}
                  </div>
                </button>
              ))}
          {hits?.length === 0 && <div className="empty">No matches.</div>}
          {!hits && sessions.length === 0 && (
            <div className="empty">
              {status?.indexing ? "Indexing your sessions…" : "No sessions found in ~/.claude or ~/.codex yet."}
            </div>
          )}
        </div>
      </section>

      <main className="detail">
        {selected ? (
          <Transcript id={selected} focusIdx={focusIdx ? Number(focusIdx) : null} version={version} />
        ) : (
          <div className="empty center">Select a session to review it.</div>
        )}
      </main>
    </div>
  );
}

function FacetButton(props: {
  label: string;
  count?: number;
  active: boolean;
  onClick: () => void;
  title?: string;
  badge?: string;
}) {
  return (
    <button className={`facet ${props.active ? "active" : ""}`} onClick={props.onClick} title={props.title}>
      {props.badge && <span className={`dot ${props.badge}`} />}
      <span className="facet-label">{props.label}</span>
      {props.count !== undefined && <span className="count">{props.count}</span>}
    </button>
  );
}

/**
 * Shows "Updated just now" for 4 seconds after a scan. After that it counts seconds, so a late
 * or failing scan is easy to spot. The component re-renders every second by itself, so the rest
 * of the page does not.
 */
function LastUpdated({ at }: { at: number | null }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  if (!at) return <span className="updated">Updating…</span>;
  const secs = Math.max(0, Math.floor((now - at) / 1000));
  const ago = secs < 5 ? "just now" : secs < 60 ? `${secs}s ago` : relativeTime(new Date(at).toISOString());
  return <span className="updated">Updated {ago}</span>;
}

/**
 * The icon turns while a refresh runs. When the refresh ends, the icon finishes its current turn
 * and stops at its start position, so it never snaps back. A fast refresh still shows a full turn.
 */
function RefreshButton({ refreshing, onClick }: { refreshing: boolean; onClick: () => void }) {
  const [turning, setTurning] = useState(false);
  useEffect(() => {
    if (refreshing) setTurning(true);
  }, [refreshing]);
  return (
    <button
      className={`refresh ${turning ? "spinning" : ""}`}
      onClick={onClick}
      onAnimationIteration={() => {
        if (!refreshing) setTurning(false);
      }}
      title="Refresh now"
      aria-label="Refresh now"
    >
      <RefreshIcon />
    </button>
  );
}

/** A clockwise arrow (the Lucide "rotate-cw" icon). */
function RefreshIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8" />
      <path d="M21 3v5h-5" />
    </svg>
  );
}

/** Renders server snippets where matches are delimited by \u0002 … \u0003. */
function Highlighted({ text }: { text: string }) {
  const parts = text.split(/\u0002|\u0003/);
  return <>{parts.map((p, i) => (i % 2 ? <mark key={i}>{p}</mark> : <span key={i}>{p}</span>))}</>;
}
