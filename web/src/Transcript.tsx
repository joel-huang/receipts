import { type RefObject, useEffect, useLayoutEffect, useRef, useState } from "react";
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
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    localStorage.setItem("receipts.filters", JSON.stringify(filters));
  }, [filters]);

  useEffect(() => {
    setError(null);
    api.session(id).then(setData).catch((e) => setError(String(e)));
  }, [id, version]);

  // Scroll to a search hit once. Background refreshes reload `data`, and they must not scroll the chat.
  const scrolledTo = useRef<string | null>(null);
  useEffect(() => {
    const key = `${id}:${focusIdx}`;
    if (!data || data.session.id !== id || focusIdx === null || scrolledTo.current === key) return;
    focusRef.current?.scrollIntoView({ block: "center" });
    scrolledTo.current = key;
  }, [data, id, focusIdx]);

  // Track whether the chat is scrolled to the bottom. The ref holds the value from before the
  // latest render, so the layout effect below can tell where the reader was before new messages.
  const messagesRef = useRef<HTMLDivElement>(null);
  const atBottomRef = useRef(true);
  const [atBottom, setAtBottom] = useState(true);
  useEffect(() => {
    const scroller = scrollRef.current;
    const content = messagesRef.current;
    if (!scroller || !content) return;
    const check = () => {
      const bottom = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 40;
      atBottomRef.current = bottom;
      setAtBottom(bottom);
    };
    check();
    scroller.addEventListener("scroll", check, { passive: true });
    // Content height changes when messages arrive or when a filter hides blocks.
    const resize = new ResizeObserver(check);
    resize.observe(content);
    return () => {
      scroller.removeEventListener("scroll", check);
      resize.disconnect();
    };
  }, [data?.session.id]);

  // When a refresh adds messages: stay pinned if the reader was at the bottom, else wiggle the button.
  const seenCount = useRef<{ id: string; count: number } | null>(null);
  const [wiggle, setWiggle] = useState(0);
  useLayoutEffect(() => {
    if (!data) return;
    const prev = seenCount.current;
    seenCount.current = { id: data.session.id, count: data.messages.length };
    if (!prev || prev.id !== data.session.id || data.messages.length <= prev.count) return;
    const scroller = scrollRef.current;
    if (atBottomRef.current && scroller) scroller.scrollTop = scroller.scrollHeight;
    else setWiggle((w) => w + 1);
  }, [data]);

  const scrollToBottom = () => {
    const scroller = scrollRef.current;
    if (scroller) scroller.scrollTop = scroller.scrollHeight;
  };

  if (error) return <div className="error">{error}</div>;
  if (!data || data.session.id !== id) return <div className="empty center">Loading…</div>;

  const { session, messages } = data;
  const visible = (m: Message, i: number) =>
    i === focusIdx ||
    ((filters.tools || (m.kind !== "tool_use" && m.kind !== "tool_result")) &&
      (filters.thinking || m.kind !== "thinking") &&
      (filters.system || m.role !== "system"));

  return (
    <div className="transcript-layout">
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
      <div className="transcript-body">
        <ChatNav messages={messages} scrollRef={scrollRef} />
        <div className="transcript-main">
          <div className="transcript-scroll" ref={scrollRef}>
            <div className="messages" ref={messagesRef}>
              {messages.map((m, i) =>
                visible(m, i) ? (
                  <div
                    key={i}
                    id={`msg-${i}`}
                    ref={i === focusIdx ? focusRef : undefined}
                    className={i === focusIdx ? "focused" : undefined}
                  >
                    <MessageView m={m} />
                  </div>
                ) : null,
              )}
            </div>
          </div>
          {!atBottom && (
            <button
              // A new key remounts the button, which replays the wiggle animation.
              key={wiggle}
              className={`scroll-down ${wiggle ? "wiggle" : ""}`}
              onClick={scrollToBottom}
              title="Scroll to the latest message"
              aria-label="Scroll to the latest message"
            >
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M12 5v14" />
                <path d="m19 12-7 7-7-7" />
              </svg>
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

/** The panel lists user prompts. Claude Code writes slash commands as "<command-name>…", so the panel skips those. */
function isPrompt(m: Message) {
  return m.role === "user" && m.kind === "text" && !m.text.trimStart().startsWith("<");
}

/** Formats a timestamp as local 24-hour HH:MM. Some messages have no timestamp, and they get an empty string. */
function clockTime(iso: string | null) {
  if (!iso) return "";
  return new Date(iso).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", hourCycle: "h23" });
}

function ChatNav({ messages, scrollRef }: { messages: Message[]; scrollRef: RefObject<HTMLDivElement | null> }) {
  const prompts = messages.flatMap((m, i) => (isPrompt(m) ? [{ idx: i, text: m.text, time: clockTime(m.timestamp) }] : []));
  const [active, setActive] = useState<number | null>(prompts[0]?.idx ?? null);
  const navRef = useRef<HTMLElement>(null);

  // Highlight the last prompt that has scrolled past the top of the messages area.
  useEffect(() => {
    const scroller = scrollRef.current;
    if (!scroller) return;
    let frame = 0;
    const update = () => {
      frame = 0;
      const top = scroller.getBoundingClientRect().top + 24;
      let current = prompts[0]?.idx ?? null;
      for (const p of prompts) {
        const el = document.getElementById(`msg-${p.idx}`);
        if (el && el.getBoundingClientRect().top <= top) current = p.idx;
      }
      setActive(current);
    };
    const onScroll = () => {
      if (!frame) frame = requestAnimationFrame(update);
    };
    update();
    scroller.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      scroller.removeEventListener("scroll", onScroll);
      cancelAnimationFrame(frame);
    };
    // React rebuilds `prompts` on every render, so the effect depends on `messages` instead.
  }, [messages, scrollRef]);

  // Keep the highlighted item visible when a long chat scrolls it out of the panel.
  useEffect(() => {
    navRef.current?.querySelector(".nav-item.active")?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const jump = (idx: number) => {
    const scroller = scrollRef.current;
    const el = document.getElementById(`msg-${idx}`);
    if (!scroller || !el) return;
    scroller.scrollTo({ top: el.offsetTop - 12 });
  };

  return (
    <nav className="chat-nav" ref={navRef}>
      {prompts.map((p) => (
        <button
          key={p.idx}
          className={`nav-item ${active === p.idx ? "active" : ""}`}
          onClick={() => jump(p.idx)}
          title={p.text.slice(0, 500)}
        >
          <span className="nav-time">{p.time}</span>
          <span className="nav-text">{p.text}</span>
        </button>
      ))}
      {prompts.length === 0 && <div className="muted nav-empty">No prompts in this chat.</div>}
    </nav>
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
