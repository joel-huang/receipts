import { type RefObject, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { api, type Message, type Session } from "./api";
import { compactNumber, toolSummary } from "./format";
import { RemoteIcon } from "./RemoteIcon";

type Filters = { tools: boolean; thinking: boolean; system: boolean };

export function Transcript({
  id,
  focusIdx,
  version,
  remote,
}: {
  id: string;
  focusIdx: number | null;
  version: number;
  /** SSH target when the chats come from another machine. */
  remote?: string | null;
}) {
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
    // Empty thinking blocks only mark time for the timeline. They have nothing to read.
    (!(m.kind === "thinking" && !m.text.trim()) &&
      (filters.tools || (m.kind !== "tool_use" && m.kind !== "tool_result")) &&
      (filters.thinking || m.kind !== "thinking") &&
      (filters.system || m.role !== "system"));

  return (
    <div className="transcript-layout">
      <header className="transcript-header">
        <h1>{session.title ?? "(untitled)"}</h1>
        <div className="meta">
          <span className={`badge ${session.source}`}>{session.source}</span>
          {session.project && (
            <span title="Working directory">
              <RemoteIcon remote={remote} />
              {session.project}
            </span>
          )}
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
        <Timeline messages={messages} source={session.source} onJump={(idx) => scrollToMessage(scrollRef.current, idx)} />
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

/** Scrolls the chat so that message `idx` sits just below the top of the messages area. */
function scrollToMessage(scroller: HTMLElement | null, idx: number) {
  const el = document.getElementById(`msg-${idx}`);
  if (scroller && el) scroller.scrollTo({ top: el.offsetTop - 12 });
}

/**
 * Tags that Claude Code writes into user messages itself, such as slash commands and background
 * task notifications. A user message that starts with one of these is not a typed prompt. Other
 * tags, such as <pasted_content>, come from the user.
 */
const AGENT_TAGS = [
  "bash-input",
  "bash-stderr",
  "bash-stdout",
  "command-args",
  "command-message",
  "command-name",
  "local-command-caveat",
  "local-command-stderr",
  "local-command-stdout",
  "persisted-output",
  "system-reminder",
  "task-notification",
  "tool_use_error",
  "user-memory-input",
];

/** Returns the agent tag that a message starts with, if any. */
function agentTag(text: string): string | null {
  const match = /^<([a-z_-]+)/.exec(text.trimStart());
  return match && AGENT_TAGS.includes(match[1]) ? match[1] : null;
}

/** The navigation column lists prompts that the user typed. */
function isPrompt(m: Message) {
  return m.role === "user" && m.kind === "text" && agentTag(m.text) === null;
}

/**
 * A turn of agent work starts at any user text. Besides typed prompts, that includes messages
 * such as background task notifications, which also wake the agent.
 */
function isTurnStart(m: Message) {
  return m.role === "user" && m.kind === "text";
}

/** Tooltip label for the message that started a turn. */
function turnLabel(m: Message) {
  const tag = agentTag(m.text);
  if (tag === null) return m.text;
  if (tag === "task-notification") return "background task notification";
  if (tag.startsWith("command-")) return /<command-name>(.*?)<\/command-name>/.exec(m.text)?.[1] ?? "slash command";
  return tag.replace(/[-_]/g, " ");
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

  const jump = (idx: number) => scrollToMessage(scrollRef.current, idx);

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

type PartKind = "think" | "reply" | "tool";
type Part = { kind: PartKind; start: number; end: number };
type Turn = { idx: number; prompt: string; start: number; end: number; parts: Part[] };

const PART_LABELS: Record<PartKind, string> = { think: "thinking", reply: "response", tool: "tool calls" };

/** Logs record when a message was written, so the time before a message counts as that message's kind. */
function partKind(m: Message): PartKind | null {
  if (m.kind === "thinking") return "think";
  if (m.kind === "tool_use" || m.kind === "tool_result") return "tool";
  if (m.role === "assistant") return "reply";
  return null;
}

/**
 * Splits a chat into agent turns. A turn starts at a user prompt and ends at the last message
 * before the next prompt. The time between a turn's end and the next prompt is idle time.
 * Each turn is further split into thinking, response and tool call parts.
 */
function agentTurns(messages: Message[]): { turns: Turn[]; start: number; end: number } | null {
  const turns: Turn[] = [];
  let start = Infinity;
  let end = -Infinity;
  let current: Turn | null = null;
  messages.forEach((m, idx) => {
    const t = m.timestamp ? Date.parse(m.timestamp) : NaN;
    if (Number.isNaN(t)) return;
    start = Math.min(start, t);
    end = Math.max(end, t);
    if (isTurnStart(m)) {
      if (current) turns.push(current);
      current = { idx, prompt: turnLabel(m), start: t, end: t, parts: [] };
      return;
    }
    if (!current || t <= current.end) return;
    const last = current.parts[current.parts.length - 1];
    // Other messages, such as a slash command, extend the part before them.
    const kind = partKind(m) ?? last?.kind;
    if (kind && last?.kind === kind) last.end = t;
    else if (kind) current.parts.push({ kind, start: current.end, end: t });
    current.end = t;
  });
  if (current) turns.push(current);
  if (!Number.isFinite(start) || end <= start) return null;
  // A prompt without any agent message has no work to show.
  return { turns: turns.filter((t) => t.end > t.start), start, end };
}

/** Total time for each part kind, in the fixed legend order. */
function partTotals(turns: Turn[]) {
  const totals: Record<PartKind, number> = { think: 0, reply: 0, tool: 0 };
  for (const t of turns) for (const p of t.parts) totals[p.kind] += p.end - p.start;
  return totals;
}

function formatDuration(ms: number) {
  const secs = Math.round(ms / 1000);
  if (secs < 60) return `${secs}s`;
  const mins = Math.round(secs / 60);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  return mins % 60 ? `${hours}h ${mins % 60}m` : `${hours}h`;
}

const toIso = (ms: number) => new Date(ms).toISOString();

/** Pills narrower than this grow to it, so short turns stay visible and clickable. */
const MIN_PILL_PX = 6;
/** Pills closer than this merge, so they never overlap and separate pills keep a visible gap. */
const PILL_GAP_PX = 2;

type Pill = { turns: Turn[]; start: number; end: number; left: number; right: number };

function mainKind(totals: Record<PartKind, number>): PartKind {
  return (Object.entries(totals) as [PartKind, number][]).reduce((a, b) => (b[1] > a[1] ? b : a))[0];
}

/**
 * Wall-clock timeline of the chat. Filled pills are agent turns in shades of the agent's color;
 * the gaps are idle time. Each pill shows a tooltip on hover or focus, and a click jumps to its
 * first prompt.
 */
function Timeline({ messages, source, onJump }: { messages: Message[]; source: string; onJump: (idx: number) => void }) {
  const [hover, setHover] = useState<number | null>(null);
  // Measure the bar whenever it appears. A new chat may have only a prompt, and then the bar
  // does not render yet, so a measurement at first render would find nothing.
  const [width, setWidth] = useState(0);
  const observer = useRef<ResizeObserver | null>(null);
  const trackRef = useCallback((track: HTMLDivElement | null) => {
    observer.current?.disconnect();
    observer.current = null;
    if (!track) return;
    observer.current = new ResizeObserver(() => setWidth(track.clientWidth));
    observer.current.observe(track);
  }, []);

  const data = agentTurns(messages);
  if (!data) return null;
  const { turns, start, end } = data;
  const span = end - start;
  const px = (t: number) => ((t - start) / span) * width;
  const working = turns.reduce((sum, t) => sum + (t.end - t.start), 0);

  // Merge turns whose pills would touch or overlap at the current width.
  const pills: Pill[] = [];
  for (const t of turns) {
    const left = px(t.start);
    const right = Math.max(px(t.end), left + MIN_PILL_PX);
    const last = pills[pills.length - 1];
    if (last && left < last.right + PILL_GAP_PX) {
      last.turns.push(t);
      last.end = t.end;
      last.right = Math.max(last.right, right);
    } else {
      pills.push({ turns: [t], start: t.start, end: t.end, left, right });
    }
  }
  const shown = hover === null ? null : pills[hover];
  const shownWork = shown ? shown.turns.reduce((sum, t) => sum + (t.end - t.start), 0) : 0;

  return (
    <div className="timeline" style={{ ["--agent" as string]: `var(--${source}, var(--muted))` }}>
      <div className="timeline-row">
        <span className="timeline-time">{clockTime(toIso(start))}</span>
        <div className="timeline-track" ref={trackRef} onPointerLeave={() => setHover(null)}>
          {width > 0 &&
            pills.map((pill, i) => (
              <button
                key={pill.turns[0].idx}
                className="timeline-hit"
                style={{ left: pill.left, width: pill.right - pill.left }}
                onPointerEnter={() => setHover(i)}
                onFocus={() => setHover(i)}
                onBlur={() => setHover(null)}
                onClick={() => onJump(pill.turns[0].idx)}
                aria-label={`Agent worked ${formatDuration(pill.turns.reduce((sum, t) => sum + (t.end - t.start), 0))}, ${clockTime(toIso(pill.start))} to ${clockTime(toIso(pill.end))}`}
              >
                <span className={`timeline-seg part-${mainKind(partTotals(pill.turns))} ${hover === i ? "active" : ""}`}>
                  {pill.turns.flatMap((t) =>
                    t.parts.map((p) => (
                      <span
                        key={`${t.idx}:${p.start}`}
                        className={`timeline-part part-${p.kind}`}
                        style={{ left: px(p.start) - pill.left, width: px(p.end) - px(p.start) }}
                      />
                    )),
                  )}
                </span>
              </button>
            ))}
          {shown && (
            <div className="timeline-tip" style={{ left: `${Math.min(88, Math.max(12, ((shown.left + shown.right) / 2 / width) * 100))}%` }}>
              <strong>{formatDuration(shownWork)}</strong>
              <span>
                {clockTime(toIso(shown.start))}–{clockTime(toIso(shown.end))}
              </span>
              <span className="timeline-tip-prompt">{shown.turns[0].prompt}</span>
              {shown.turns.length > 1 && <span>and {shown.turns.length - 1} more prompts</span>}
              {(Object.entries(partTotals(shown.turns)) as [PartKind, number][])
                // Hide kinds that would show as "0s".
                .filter(([, ms]) => ms >= 500)
                .map(([kind, ms]) => (
                  <span key={kind} className="timeline-tip-row">
                    <i className={`key-line part-${kind}`} />
                    <b>{formatDuration(ms)}</b> {PART_LABELS[kind]}
                  </span>
                ))}
            </div>
          )}
        </div>
        <span className="timeline-time">
          {new Date(start).toDateString() === new Date(end).toDateString()
            ? clockTime(toIso(end))
            : `${new Date(end).toLocaleDateString(undefined, { month: "short", day: "numeric" })} ${clockTime(toIso(end))}`}
        </span>
      </div>
      <div className="timeline-row timeline-foot">
        <span className="timeline-legend">
          {(Object.entries(partTotals(turns)) as [PartKind, number][]).map(([kind, ms]) => (
            <span key={kind}>
              <i className={`key-rect part-${kind}`} />
              {PART_LABELS[kind]} {formatDuration(ms)}
            </span>
          ))}
        </span>
        <span className="timeline-total">
          {formatDuration(working)} working of {formatDuration(span)}
        </span>
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
