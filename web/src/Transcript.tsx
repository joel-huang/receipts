import { useEffect, useRef, useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { api, type Message, type Session } from "./api";
import { compactNumber, toolSummary } from "./format";

type Filters = { tools: boolean; thinking: boolean; system: boolean };

export function Transcript({ id, focusIdx, version }: { id: string; focusIdx: number | null; version: number }) {
  const [data, setData] = useState<{ session: Session; messages: Message[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filters, setFilters] = useState<Filters>(() => {
    const saved = localStorage.getItem("receipts.filters");
    return saved ? JSON.parse(saved) : { tools: true, thinking: true, system: false };
  });
  const focusRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    localStorage.setItem("receipts.filters", JSON.stringify(filters));
  }, [filters]);

  useEffect(() => {
    setError(null);
    api.session(id).then(setData).catch((e) => setError(String(e)));
  }, [id, version]);

  useEffect(() => {
    focusRef.current?.scrollIntoView({ block: "center" });
  }, [data, focusIdx]);

  if (error) return <div className="error">{error}</div>;
  if (!data || data.session.id !== id) return <div className="empty center">Loading…</div>;

  const { session, messages } = data;
  const visible = (m: Message, i: number) =>
    i === focusIdx ||
    ((filters.tools || (m.kind !== "tool_use" && m.kind !== "tool_result")) &&
      (filters.thinking || m.kind !== "thinking") &&
      (filters.system || m.role !== "system"));

  return (
    <div className="transcript">
      <header className="transcript-header">
        <h1>{session.title ?? "(untitled)"}</h1>
        <div className="meta">
          <span className={`badge ${session.source}`}>{session.source}</span>
          {session.project && <span title="Working directory">{session.project}</span>}
          {session.git_branch && <span>⎇ {session.git_branch}</span>}
          {session.model && <span>{session.model}</span>}
          <span>
            {compactNumber(session.input_tokens)} in / {compactNumber(session.output_tokens)} out
          </span>
          {session.started_at && <span>{new Date(session.started_at).toLocaleString()}</span>}
        </div>
        <div className="toggles">
          {(Object.keys(filters) as (keyof Filters)[]).map((k) => (
            <label key={k}>
              <input type="checkbox" checked={filters[k]} onChange={(e) => setFilters({ ...filters, [k]: e.target.checked })} />
              {k}
            </label>
          ))}
          <button className="link" onClick={() => navigator.clipboard.writeText(session.path)} title={session.path}>
            Copy log path
          </button>
          <button className="link" onClick={() => download(session, messages)}>
            Export Markdown
          </button>
        </div>
      </header>
      <div className="messages">
        {messages.map((m, i) =>
          visible(m, i) ? (
            <div key={i} ref={i === focusIdx ? focusRef : undefined} className={i === focusIdx ? "focused" : undefined}>
              <MessageView m={m} />
            </div>
          ) : null,
        )}
      </div>
    </div>
  );
}

function MessageView({ m }: { m: Message }) {
  if (m.kind === "tool_use") {
    const summary = toolSummary(m.tool_name, m.tool_input);
    return (
      <details className="tool">
        <summary>
          <span className="tool-name">{m.tool_name}</span> <code className="tool-summary">{summary}</code>
        </summary>
        <pre>{typeof m.tool_input === "string" ? m.tool_input : JSON.stringify(m.tool_input, null, 2)}</pre>
      </details>
    );
  }
  if (m.kind === "tool_result") {
    const firstLine = m.text.split("\n").find((l) => l.trim()) ?? "(empty)";
    return (
      <details className={`tool result ${m.is_error ? "error-result" : ""}`}>
        <summary>
          <span className="tool-name">{m.is_error ? "error" : "result"}</span>{" "}
          <code className="tool-summary">{firstLine.slice(0, 160)}</code>
        </summary>
        <pre>{m.text}</pre>
      </details>
    );
  }
  if (m.kind === "thinking") {
    return (
      <details className="thinking">
        <summary>Thinking</summary>
        <div className="prose">
          <Markdown remarkPlugins={[remarkGfm]}>{m.text}</Markdown>
        </div>
      </details>
    );
  }
  return (
    <div className={`bubble ${m.role}`}>
      <div className="bubble-head">
        <span className="role">{m.role}</span>
        {m.timestamp && <time>{new Date(m.timestamp).toLocaleTimeString()}</time>}
      </div>
      <div className="prose">
        {m.role === "assistant" ? <Markdown remarkPlugins={[remarkGfm]}>{m.text}</Markdown> : <p className="plain">{m.text}</p>}
      </div>
    </div>
  );
}

function download(session: Session, messages: Message[]) {
  const lines = [`# ${session.title ?? "Session"}`, "", `- Agent: ${session.source}`, `- Project: ${session.project ?? ""}`, ""];
  for (const m of messages) {
    if (m.kind === "text" && m.role !== "system") lines.push(`## ${m.role}`, "", m.text, "");
    else if (m.kind === "tool_use") lines.push(`> **${m.tool_name}** \`${toolSummary(m.tool_name, m.tool_input)}\``, "");
  }
  const blob = new Blob([lines.join("\n")], { type: "text/markdown" });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = `${session.id.replace(/[^\w.-]+/g, "_")}.md`;
  a.click();
  URL.revokeObjectURL(a.href);
}
