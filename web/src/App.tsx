import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { api, setMachine, type Facet, type MachineInfo, type SearchHit, type Session, type Status } from "./api";
import { compactNumber, relativeTime, shortProject } from "./format";
import { RemoteIcon } from "./RemoteIcon";
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

/** Name of this computer in the machine selector. */
const LOCAL = "This computer";

/**
 * Picks the machine whose chats the page shows. Another machine connects over SSH when it is
 * picked, and its chats show once the connection is up.
 */
export function App() {
  const [params, update] = useHashParams();
  const host = params.get("host");
  const [machines, setMachines] = useState<MachineInfo[]>([]);
  const [listed, setListed] = useState(false);
  const current = machines.find((m) => m.name === host);

  // Check the machines every 30 seconds, and every second while the picked one connects.
  const connecting = current?.link === "connecting";
  const loadMachines = useCallback(() => {
    api
      .machines()
      .then((list) => {
        setMachines(list);
        setListed(true);
      })
      .catch(() => setListed(true));
  }, []);
  useEffect(() => {
    loadMachines();
    const timer = setInterval(loadMachines, connecting ? 1000 : 30_000);
    return () => clearInterval(timer);
  }, [connecting, loadMachines]);

  const connect = useCallback((name: string) => {
    api
      .connect(name)
      .then((m) => setMachines((list) => list.map((x) => (x.name === m.name ? m : x))))
      .catch(() => {});
  }, []);

  // Connect when a machine is picked. A failed connection waits for Retry.
  useEffect(() => {
    if (host && current?.link === "idle") connect(host);
  }, [host, current?.link, connect]);

  const ready = !host || current?.link === "connected";
  // This server's version, for the switcher's footer.
  const [version, setVersion] = useState<string>();
  useEffect(() => {
    fetch("/api/status")
      .then((r) => r.json())
      .then((s: Status) => setVersion(s.version))
      .catch(() => {});
  }, []);
  // Children read the base path while they render, so set it first.
  setMachine(ready ? host : null);

  const selector = (
    <MachineSelector
      host={host}
      machines={machines}
      version={version}
      onSelect={(name) => update({ host: name, s: null, m: null, q: null, source: null, project: null })}
      onOpen={loadMachines}
    />
  );
  if (!ready) {
    return (
      <div className="app">
        <aside className="sidebar">{selector}</aside>
        <section className="list" />
        <main className="detail machine-status">
          {!listed ? (
            <div className="empty center">Loading machines…</div>
          ) : !current ? (
            <div className="empty center">{host} is not in ~/.ssh/config.</div>
          ) : current.link === "failed" ? (
            <div className="empty center">
              <div>
                <p className="error-text">{current.message}</p>
                <button className="link" onClick={() => connect(current.name)}>
                  Retry
                </button>
              </div>
            </div>
          ) : (
            <div className="empty center pulse">{current.message ?? `Connecting to ${host}`}…</div>
          )}
        </main>
      </div>
    );
  }
  return <Workspace key={host ?? LOCAL} selector={selector} />;
}

/**
 * Machine switcher at the top of the sidebar, like a workspace switcher. It lists this computer
 * and the machines whose SSH server answers.
 */
function MachineSelector(props: {
  host: string | null;
  machines: MachineInfo[];
  version: string | undefined;
  onSelect: (name: string | null) => void;
  onOpen: () => void;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const escape = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", escape);
    };
  }, [open]);
  const pick = (name: string | null) => {
    setOpen(false);
    if (name !== props.host) props.onSelect(name);
  };
  // Keep the picked and connected machines even if a check misses them.
  const available = props.machines.filter((m) => m.reachable === true || m.link === "connected" || m.name === props.host);
  const item = (name: string | null, detail: string) => (
    <button key={name ?? LOCAL} className="machine-item" role="menuitem" onClick={() => pick(name)}>
      <MachineAvatar name={name} />
      <span className="machine-name">{name ?? LOCAL}</span>
      <span className="machine-detail">{detail}</span>
      <span className="machine-check">{props.host === name ? "✓" : ""}</span>
    </button>
  );
  return (
    <div className="machine-selector" ref={ref}>
      <button
        className="machine-current"
        onClick={() => {
          // Check the list again, so the menu shows the machines that answer now.
          if (!open) props.onOpen();
          setOpen(!open);
        }}
        aria-expanded={open}
        aria-haspopup="menu"
      >
        <MachineAvatar name={props.host} />
        <span className="machine-name">{props.host ?? LOCAL}</span>
        <svg className="caret" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="m6 9 6 6 6-6" />
        </svg>
      </button>
      {open && (
        <div className="machine-menu" role="menu">
          <div className="machine-menu-label">Machines</div>
          {item(null, "")}
          {available.map((m) => item(m.name, m.link === "connected" ? "connected" : osName(m.detail)))}
          {props.version && <div className="machine-menu-footer">Receipts v{props.version}</div>}
        </div>
      )}
    </div>
  );
}

/** Square with the machine's first letter, in a color that stays the same for each name. */
function MachineAvatar({ name }: { name: string | null }) {
  const hue = name ? [...name].reduce((h, c) => (h * 31 + c.charCodeAt(0)) % 360, 7) : null;
  return (
    <span className="machine-avatar" style={hue === null ? undefined : { background: `oklch(0.6 0.12 ${hue})` }} aria-hidden="true">
      {(name ?? LOCAL)[0].toUpperCase()}
    </span>
  );
}

/** Short OS name from an SSH greeting, such as "Windows" from "OpenSSH_for_Windows_9.5". */
function osName(detail: string | null) {
  if (!detail) return "";
  for (const os of ["Windows", "Ubuntu", "Debian", "FreeBSD", "Raspbian"]) if (detail.includes(os)) return os;
  return detail.startsWith("OpenSSH") ? "" : detail;
}

function Workspace({ selector }: { selector: ReactNode }) {
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
        {selector}
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
                icon={<RemoteIcon remote={status?.remote} />}
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
                    {h.role} · <RemoteIcon remote={status?.remote} />
                    {shortProject(h.project)} · {relativeTime(h.updated_at)}
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
                    <RemoteIcon remote={status?.remote} />
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
          <Transcript
            id={selected}
            focusIdx={focusIdx ? Number(focusIdx) : null}
            version={version}
            remote={status?.remote}
          />
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
  icon?: React.ReactNode;
}) {
  return (
    <button className={`facet ${props.active ? "active" : ""}`} onClick={props.onClick} title={props.title}>
      {props.badge && <span className={`dot ${props.badge}`} />}
      <span className="facet-label">
        {props.icon}
        {props.label}
      </span>
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
