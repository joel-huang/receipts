import { useCallback, useEffect, useState } from "react";
import { api, type Facet, type SearchHit, type Session, type Status } from "./api";
import { compactNumber, relativeTime, shortProject } from "./format";
import { Transcript } from "./Transcript";

/** App state lives in the URL hash so every view is linkable and survives reloads. */
function useHashParams(): [URLSearchParams, (patch: Record<string, string | null>) => void] {
  const read = () => new URLSearchParams(window.location.hash.replace(/^#\/?\??/, ""));
  const [params, setParams] = useState(read);
  useEffect(() => {
    const onHash = () => setParams(read());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);
  const update = useCallback((patch: Record<string, string | null>) => {
    const next = read();
    for (const [k, v] of Object.entries(patch)) (v ? next.set(k, v) : next.delete(k));
    window.location.hash = `/?${next}`;
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

  // Poll status while the background indexer runs; bump `version` when data changes.
  const [version, setVersion] = useState(0);
  useEffect(() => {
    let last = -1;
    let timer: number;
    const tick = async () => {
      try {
        const s = await api.status();
        setStatus(s);
        if (s.sessions !== last) {
          last = s.sessions;
          setVersion((v) => v + 1);
        }
        timer = window.setTimeout(tick, s.indexing ? 1500 : 15000);
      } catch (e) {
        setError(String(e));
        timer = window.setTimeout(tick, 5000);
      }
    };
    tick();
    return () => clearTimeout(timer);
  }, []);

  useEffect(() => {
    api.facets().then(setFacets).catch((e) => setError(String(e)));
  }, [version]);

  useEffect(() => {
    api.sessions(source, project).then(setSessions).catch((e) => setError(String(e)));
  }, [source, project, version]);

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
          {status?.indexing ? (
            <span className="pulse">Indexing… {status.sessions}</span>
          ) : (
            <button className="link" onClick={() => api.reindex().then(() => setTimeout(() => setVersion((v) => v + 1), 500))}>
              Rescan
            </button>
          )}
          <span className="muted">v{status?.version}</span>
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

/** Renders server snippets where matches are delimited by \u0002 … \u0003. */
function Highlighted({ text }: { text: string }) {
  const parts = text.split(/\u0002|\u0003/);
  return <>{parts.map((p, i) => (i % 2 ? <mark key={i}>{p}</mark> : <span key={i}>{p}</span>))}</>;
}
