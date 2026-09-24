import { useVirtualizer } from "@tanstack/react-virtual";
import { memo, type ReactNode, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { api, ApiError, type Message, type Outline, type Session } from "./api";
import { compactNumber, toolSummary } from "./format";
import { RemoteIcon } from "./RemoteIcon";

type Filters = { tools: boolean; thinking: boolean; system: boolean };

/** Tooltip for a toggle whose kind the chat lacks. */
const ABSENT: Record<keyof Filters, string> = {
  tools: "This chat has no tool calls",
  thinking: "This chat has no readable thinking. Codex, for example, stores its reasoning encrypted.",
  system: "This chat has no system messages",
};

/** Messages per request when the chat view loads the messages near the view. */
const PAGE = 100;

/** Height guess for a message that has not rendered yet. The list corrects it after rendering. */
function estimateHeight(m: Message | undefined) {
  if (!m || m.kind !== "text") return 46;
  return m.role === "user" ? 100 : 140;
}

/** Turns an outline item into a message with only the fields that the outline has. */
function fromOutline([timestamp, role, kind, text]: Outline["items"][number]): Message {
  return { role, kind, text: text ?? "", tool_name: null, tool_input: null, is_error: false, timestamp };
}

export function Transcript({
  id,
  focusIdx,
  version,
  remote,
  onBack,
}: {
  id: string;
  focusIdx: number | null;
  version: number;
  /** SSH target when the chats come from another machine. */
  remote?: string | null;
  /** Returns to the chat list. The phone layout shows a back button for it. */
  onBack?: () => void;
}) {
  // The outline covers every message in brief. It feeds the timeline, the navigation column and
  // the list layout. Full messages load only near the view, so a large chat opens at once.
  const [outline, setOutline] = useState<{ session: Session; items: Message[] } | null>(null);
  const [loaded, setLoaded] = useState<(Message | undefined)[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [filters, setFilters] = useState<Filters>(() => {
    const saved = localStorage.getItem("receipts.filters");
    return saved ? JSON.parse(saved) : { tools: true, thinking: true, system: false };
  });
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    localStorage.setItem("receipts.filters", JSON.stringify(filters));
  }, [filters]);

  // Load the outline once, then only the part after what is already loaded.
  const outlineCount = useRef<{ id: string; count: number } | null>(null);
  // Servers before the outline endpoint, such as an older release on a remote machine, only send
  // whole chats. Then the app loads the whole chat on every refresh.
  const wholeChats = useRef(false);
  useEffect(() => {
    let cancelled = false;
    const from = outlineCount.current?.id === id ? outlineCount.current.count : 0;
    const loadWhole = async () => {
      const all = await api.session(id);
      if (cancelled) return;
      setError(null);
      setLoaded(all.messages);
      outlineCount.current = { id, count: all.messages.length };
      setOutline({ session: all.session, items: all.messages });
    };
    (async () => {
      if (wholeChats.current) return loadWhole();
      let page;
      try {
        page = await api.outline(id, from);
      } catch (e) {
        if (!(e instanceof ApiError && e.status === 404)) throw e;
        wholeChats.current = true;
        return loadWhole();
      }
      // The chat got shorter than what is loaded, so load it again from the start.
      if (page.total < from) page = await api.outline(id, 0);
      if (cancelled) return;
      setError(null);
      if (page.offset === 0) setLoaded([]);
      setOutline((prev) => {
        const keep = prev && prev.session.id === id && page.offset > 0 ? prev.items.slice(0, page.offset) : [];
        const items = [...keep, ...page.items.map(fromOutline)];
        outlineCount.current = { id, count: items.length };
        return { session: page.session, items };
      });
    })().catch((e) => !cancelled && setError(String(e)));
    return () => {
      cancelled = true;
    };
  }, [id, version]);

  const items = useMemo(() => (outline && outline.session.id === id ? outline.items : []), [outline, id]);
  // Positions of the messages that the list shows, in order.
  const rows = useMemo(
    () =>
      items.flatMap((m, i) => {
        const shown =
          i === focusIdx ||
          // Empty thinking blocks only mark time for the timeline. They have nothing to read.
          (!(m.kind === "thinking" && !m.text.trim()) &&
            (filters.tools || (m.kind !== "tool_use" && m.kind !== "tool_result")) &&
            (filters.thinking || m.kind !== "thinking") &&
            (filters.system || m.role !== "system"));
        return shown ? [i] : [];
      }),
    [items, filters, focusIdx],
  );

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (r) => estimateHeight(loaded[rows[r]] ?? items[rows[r]]),
    getItemKey: (r) => rows[r],
    overscan: 8,
    paddingStart: 16,
    paddingEnd: 64,
    scrollPaddingStart: 12,
  });
  const virtualRows = virtualizer.getVirtualItems();

  // Load the messages near the view that have not loaded yet, in pages.
  const pending = useRef(new Set<string>());
  const missing = virtualRows.map((v) => rows[v.index]).filter((i) => i !== undefined && !loaded[i]);
  const first = missing.length ? Math.min(...missing) : -1;
  const last = missing.length ? Math.max(...missing) : -1;
  useEffect(() => {
    if (first < 0) return;
    for (let from = first - (first % PAGE); from <= last; from += PAGE) {
      const key = `${id}:${from}`;
      if (pending.current.has(key)) continue;
      pending.current.add(key);
      api
        .session(id, from, PAGE)
        .then((page) =>
          setLoaded((prev) => {
            const next = prev.slice();
            page.messages.forEach((m, k) => {
              // Keep a message that already loaded its full text.
              if (!next[page.offset + k] || next[page.offset + k]?.truncated) next[page.offset + k] = m;
            });
            return next;
          }),
        )
        .catch((e) => setError(String(e)))
        .finally(() => pending.current.delete(key));
    }
  }, [id, first, last]);

  // Replace a shortened message with its full text when its block opens.
  const expand = useCallback(
    (idx: number) => {
      api
        .message(id, idx)
        .then((full) =>
          setLoaded((prev) => {
            const next = prev.slice();
            next[idx] = full;
            return next;
          }),
        )
        .catch((e) => setError(String(e)));
    },
    [id],
  );

  /** Scrolls the list so that message `idx`, or the next shown message after it, is at the top. */
  const jumpTo = useCallback(
    (idx: number, align: "start" | "center" = "start") => {
      const row = rows.findIndex((i) => i >= idx);
      if (row >= 0) virtualizer.scrollToIndex(row, { align });
    },
    [rows, virtualizer],
  );
  // Messages near the end may still be loading, and the list keeps the view pinned to the bottom
  // as they grow (see below).
  const scrollToBottom = useCallback(() => {
    if (rows.length) virtualizer.scrollToIndex(rows.length - 1, { align: "end" });
  }, [rows, virtualizer]);

  // Scroll to a search hit once. Refreshes must not scroll the chat again.
  const scrolledTo = useRef<string | null>(null);
  useEffect(() => {
    const key = `${id}:${focusIdx}`;
    if (!items.length || focusIdx === null || scrolledTo.current === key) return;
    jumpTo(focusIdx, "center");
    scrolledTo.current = key;
  }, [items.length, id, focusIdx, jumpTo]);

  // Track whether the chat is scrolled to the bottom. The ref holds the value from before the
  // latest render, so the layout effect below can tell where the reader was before new messages.
  const atBottomRef = useRef(true);
  const [atBottom, setAtBottom] = useState(true);
  const hasOutline = outline?.session.id === id;
  const listRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const scroller = scrollRef.current;
    const list = listRef.current;
    if (!scroller || !list) return;
    // Messages that load or render late make the list taller, and the list may move the scroll
    // position for them. If the view was at the bottom before the list grew, keep it there.
    let lastHeight = scroller.scrollHeight;
    const check = () => {
      const grew = scroller.scrollHeight > lastHeight;
      lastHeight = scroller.scrollHeight;
      if (grew && atBottomRef.current) {
        scroller.scrollTop = scroller.scrollHeight;
        return;
      }
      const bottom = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 40;
      atBottomRef.current = bottom;
      setAtBottom(bottom);
    };
    check();
    scroller.addEventListener("scroll", check, { passive: true });
    const resize = new ResizeObserver(check);
    resize.observe(list);
    return () => {
      scroller.removeEventListener("scroll", check);
      resize.disconnect();
    };
  }, [hasOutline]);

  // When a refresh adds messages: stay pinned if the reader was at the bottom, else wiggle the button.
  const seenCount = useRef<{ id: string; count: number } | null>(null);
  const [wiggle, setWiggle] = useState(0);
  useLayoutEffect(() => {
    if (!items.length) return;
    const prev = seenCount.current;
    seenCount.current = { id, count: items.length };
    if (!prev || prev.id !== id || items.length <= prev.count) return;
    if (atBottomRef.current) scrollToBottom();
    else setWiggle((w) => w + 1);
  }, [items.length, id, scrollToBottom]);

  // The first message whose bottom is below the top of the view, for the navigation column.
  const scrollTop = virtualizer.scrollOffset ?? 0;
  const topRow = virtualRows.find((v) => v.end > scrollTop + 24);
  const topIdx = topRow ? rows[topRow.index] : null;

  // Which kinds this chat has. A toggle for a kind that the chat lacks would do nothing, so it is
  // disabled. Codex, for example, often stores its reasoning encrypted, with no text to show.
  const present = useMemo(
    () => ({
      tools: items.some((m) => m.kind === "tool_use" || m.kind === "tool_result"),
      thinking: items.some((m) => m.kind === "thinking" && m.text.trim() !== ""),
      system: items.some((m) => m.role === "system"),
    }),
    [items],
  );

  const exportMarkdown = async () => {
    if (!outline) return;
    try {
      const all = await api.session(id);
      download(outline.session, all.messages);
    } catch (e) {
      setError(String(e));
    }
  };

  if (error) return <div className="error">{error}</div>;
  if (!outline || !hasOutline) return <div className="empty center">Loading…</div>;
  const { session } = outline;

  return (
    <div className="transcript-layout">
      <header className="transcript-header">
        <div className="title-row">
          {onBack && (
            <button className="icon-button phone-only" onClick={onBack} aria-label="Back to the chat list">
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="m15 18-6-6 6-6" />
              </svg>
            </button>
          )}
          <h1>{session.title ?? "(untitled)"}</h1>
        </div>
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
            <label key={k} className={present[k] ? undefined : "disabled"} title={present[k] ? undefined : ABSENT[k]}>
              <input
                type="checkbox"
                checked={filters[k]}
                disabled={!present[k]}
                onChange={(e) => setFilters({ ...filters, [k]: e.target.checked })}
              />
              {k}
            </label>
          ))}
          <button className="link" onClick={() => navigator.clipboard.writeText(session.path)} title={session.path}>
            Copy log path
          </button>
          <button className="link" onClick={exportMarkdown}>
            Export Markdown
          </button>
        </div>
        <Timeline messages={items} source={session.source} onJump={jumpTo} />
      </header>
      <div className="transcript-body">
        <ChatNav messages={items} topIdx={topIdx} onJump={jumpTo} />
        <div className="transcript-main">
          <div className="transcript-scroll" ref={scrollRef}>
            <div className="messages" ref={listRef} style={{ height: virtualizer.getTotalSize() }}>
              {virtualRows.map((v) => {
                const idx = rows[v.index];
                const m = loaded[idx];
                return (
                  <div
                    key={v.key}
                    data-index={v.index}
                    ref={virtualizer.measureElement}
                    id={`msg-${idx}`}
                    className="message-row"
                    style={{ transform: `translateY(${v.start}px)` }}
                  >
                    <div className={idx === focusIdx ? "focused" : undefined}>
                      {m ? (
                        <MessageView m={m} idx={idx} onExpand={expand} />
                      ) : (
                        <div className="placeholder" style={{ height: estimateHeight(items[idx]) - 10 }} />
                      )}
                    </div>
                  </div>
                );
              })}
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

/** Local calendar day of a timestamp, such as "2026-09-24". Messages without a time get "". */
function dayKey(iso: string | null) {
  if (!iso) return "";
  const d = new Date(iso);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

/** Header text for a day: "Today", "Yesterday", or a date such as "Tue, Sep 22". */
function dayLabel(key: string) {
  const [y, m, d] = key.split("-").map(Number);
  const date = new Date(y, m - 1, d);
  const today = new Date();
  const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
  if (key === dayKey(today.toISOString())) return "Today";
  if (key === dayKey(yesterday.toISOString())) return "Yesterday";
  return date.toLocaleDateString(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
    year: y === today.getFullYear() ? undefined : "numeric",
  });
}

function ChatNav({
  messages,
  topIdx,
  onJump,
}: {
  messages: Message[];
  /** First message at the top of the chat view. */
  topIdx: number | null;
  onJump: (idx: number) => void;
}) {
  const prompts = messages.flatMap((m, i) =>
    isPrompt(m) ? [{ idx: i, text: m.text, time: clockTime(m.timestamp), day: dayKey(m.timestamp) }] : [],
  );
  // Prompts grouped by local day, in order. Each day gets a header that sticks while its prompts scroll.
  const days: { day: string; prompts: typeof prompts }[] = [];
  for (const p of prompts) {
    const last = days[days.length - 1];
    if (last && last.day === p.day) last.prompts.push(p);
    else days.push({ day: p.day, prompts: [p] });
  }
  // Highlight the last prompt at or above the top of the chat view.
  let active = prompts[0]?.idx ?? null;
  if (topIdx !== null) for (const p of prompts) if (p.idx <= topIdx) active = p.idx;
  const navRef = useRef<HTMLElement>(null);

  // Keep the highlighted item visible when a long chat scrolls it out of the panel.
  useEffect(() => {
    navRef.current?.querySelector(".nav-item.active")?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const jump = onJump;

  return (
    <nav className="chat-nav" ref={navRef}>
      {days.map((d) => (
        <section key={d.prompts[0].idx} className="nav-day">
          {d.day && <div className="nav-day-header">{dayLabel(d.day)}</div>}
          {d.prompts.map((p) => (
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
        </section>
      ))}
      {prompts.length === 0 && <div className="muted nav-empty">No prompts in this chat.</div>}
    </nav>
  );
}

type PartKind = "think" | "reply" | "tool";
type Part = { kind: PartKind; start: number; end: number };

/** One agent response with the tool calls after it. Each one is a pill on the timeline. */
type Response = {
  /** The user message that the response answers, and its position for a jump. */
  prompt: string;
  promptIdx: number;
  /** Start of the assistant's reply, and its position. Empty if the agent has not replied yet. */
  reply: string;
  replyIdx: number | null;
  start: number;
  end: number;
  parts: Part[];
};

const PART_LABELS: Record<PartKind, string> = { think: "thinking", reply: "response", tool: "tool calls" };

/** Logs record when a message was written, so the time before a message counts as that message's kind. */
function partKind(m: Message): PartKind | null {
  if (m.kind === "thinking") return "think";
  if (m.kind === "tool_use" || m.kind === "tool_result") return "tool";
  if (m.role === "assistant") return "reply";
  return null;
}

/**
 * Splits a chat into agent responses. Work starts at a user message. A response holds the
 * thinking before an assistant reply, the reply, and the tool calls after it. The next response
 * starts where the agent resumes after those tool calls. The time between the last response and
 * the next user message is idle time.
 */
function agentResponses(messages: Message[]): { responses: Response[]; start: number; end: number } | null {
  const responses: Response[] = [];
  let start = Infinity;
  let end = -Infinity;
  let current: Response | null = null;
  messages.forEach((m, idx) => {
    const t = m.timestamp ? Date.parse(m.timestamp) : NaN;
    if (Number.isNaN(t)) return;
    start = Math.min(start, t);
    end = Math.max(end, t);
    if (isTurnStart(m)) {
      if (current) responses.push(current);
      current = { prompt: turnLabel(m), promptIdx: idx, reply: "", replyIdx: null, start: t, end: t, parts: [] };
      return;
    }
    if (!current || t <= current.end) return;
    const last = current.parts[current.parts.length - 1];
    // Other messages, such as a slash command, extend the part before them.
    const kind = partKind(m) ?? last?.kind;
    if (kind && last?.kind === kind) last.end = t;
    else if (kind) current.parts.push({ kind, start: current.end, end: t });
    current.end = t;
    if (m.role !== "assistant" || m.kind !== "text") return;
    if (current.replyIdx === null) {
      current.reply = m.text;
      current.replyIdx = idx;
      return;
    }
    // A second reply starts a new response where the agent resumed after its last tool call.
    const tools = current.parts.filter((p) => p.kind === "tool");
    const split = tools.length ? tools[tools.length - 1].end : current.parts[current.parts.length - 1].start;
    const next: Response = {
      prompt: current.prompt,
      promptIdx: current.promptIdx,
      reply: m.text,
      replyIdx: idx,
      start: split,
      end: t,
      parts: current.parts.filter((p) => p.start >= split),
    };
    current.parts = current.parts.filter((p) => p.start < split);
    current.end = split;
    responses.push(current);
    current = next;
  });
  if (current) responses.push(current);
  if (!Number.isFinite(start) || end <= start) return null;
  // A user message without any agent work has nothing to show.
  return { responses: responses.filter((r) => r.end > r.start), start, end };
}

/** Total time for each part kind, in the fixed legend order. */
function partTotals(responses: Response[]) {
  const totals: Record<PartKind, number> = { think: 0, reply: 0, tool: 0 };
  for (const r of responses) for (const p of r.parts) totals[p.kind] += p.end - p.start;
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

/** Pills narrower than this grow to it, so short responses stay visible and clickable. */
const MIN_PILL_PX = 6;
/** Space between pills. Responses that touch in time get this gap cut from the earlier pill. */
const PILL_GAP_PX = 2;

type Pill = { responses: Response[]; start: number; end: number; left: number; right: number };

/**
 * Wall-clock timeline of the chat. Each pill is an agent response in the agent's color; the gaps
 * are idle time. The tooltip names the user message and the reply, and shows how the response's
 * time splits into thinking, response and tool calls. A click jumps to the reply.
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

  const data = agentResponses(messages);
  if (!data) return null;
  const { responses, start, end } = data;
  const span = end - start;
  const px = (t: number) => ((t - start) / span) * width;
  const working = responses.reduce((sum, r) => sum + (r.end - r.start), 0);

  // Responses that touch get a gap cut from the earlier pill. A response too narrow for that
  // merges into the pill before it.
  const pills: Pill[] = [];
  for (const r of responses) {
    const left = px(r.start);
    const right = Math.max(px(r.end), left + MIN_PILL_PX);
    const last = pills[pills.length - 1];
    if (last && left < last.right + PILL_GAP_PX) {
      const roomToSplit = left - PILL_GAP_PX - last.left >= MIN_PILL_PX && px(r.end) - left >= MIN_PILL_PX;
      if (!roomToSplit) {
        last.responses.push(r);
        last.end = r.end;
        last.right = Math.max(last.right, right);
        continue;
      }
      last.right = left - PILL_GAP_PX;
    }
    pills.push({ responses: [r], start: r.start, end: r.end, left, right });
  }
  const shown = hover === null ? null : pills[hover];
  const first = shown?.responses[0];
  const shownWork = shown ? shown.responses.reduce((sum, r) => sum + (r.end - r.start), 0) : 0;
  const shownTotals = shown ? partTotals(shown.responses) : null;

  return (
    <div className="timeline" style={{ ["--agent" as string]: `var(--${source}, var(--muted))` }}>
      <div className="timeline-row">
        <span className="timeline-time">{clockTime(toIso(start))}</span>
        <div className="timeline-track" ref={trackRef} onPointerLeave={() => setHover(null)}>
          {width > 0 &&
            pills.map((pill, i) => {
              const r = pill.responses[0];
              return (
                <button
                  key={`${r.promptIdx}:${r.start}`}
                  className="timeline-hit"
                  style={{ left: pill.left, width: pill.right - pill.left }}
                  onPointerEnter={() => setHover(i)}
                  onFocus={() => setHover(i)}
                  onBlur={() => setHover(null)}
                  onClick={() => onJump(r.replyIdx ?? r.promptIdx)}
                  aria-label={`Agent response, ${formatDuration(pill.end - pill.start)}, ${clockTime(toIso(pill.start))} to ${clockTime(toIso(pill.end))}`}
                >
                  <span className={`timeline-seg ${hover === i ? "active" : ""}`} />
                </button>
              );
            })}
          {shown && first && shownTotals && (
            <div className="timeline-tip" style={{ left: `${Math.min(88, Math.max(12, ((shown.left + shown.right) / 2 / width) * 100))}%` }}>
              <span className="timeline-tip-head">
                <strong>{formatDuration(shownWork)}</strong> {clockTime(toIso(shown.start))}–{clockTime(toIso(shown.end))}
              </span>
              <span className="timeline-tip-label">User</span>
              <span className="timeline-tip-text">{first.prompt}</span>
              {first.reply && (
                <>
                  <span className="timeline-tip-label">Assistant</span>
                  <span className="timeline-tip-text">{first.reply}</span>
                </>
              )}
              {shown.responses.length > 1 && <span>and {shown.responses.length - 1} more responses</span>}
              {/* The time of the response split by kind, in the shades of the agent's color. */}
              <span className="timeline-tip-bands">
                {(Object.entries(shownTotals) as [PartKind, number][])
                  .filter(([, ms]) => ms > 0)
                  .map(([kind, ms]) => (
                    <i key={kind} className={`part-${kind}`} style={{ flexGrow: ms }} />
                  ))}
              </span>
              {(Object.entries(shownTotals) as [PartKind, number][])
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
          {(Object.entries(partTotals(responses)) as [PartKind, number][]).map(([kind, ms]) => (
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

/**
 * A collapsed block that renders its contents only while open, so a chat with thousands of
 * blocks does not render them all. Opening a shortened block loads its full text.
 */
function Collapsible(props: {
  className: string;
  summary: ReactNode;
  truncated: boolean | undefined;
  onOpen: () => void;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  return (
    <details
      className={props.className}
      onToggle={(e) => {
        const isOpen = e.currentTarget.open;
        setOpen(isOpen);
        if (isOpen && props.truncated) props.onOpen();
      }}
    >
      <summary>{props.summary}</summary>
      {open && props.children}
      {open && props.truncated && <div className="loading-full">Loading the full text…</div>}
    </details>
  );
}

/** Memoized, so that appending new messages does not render the existing ones again. */
const MessageView = memo(function MessageView({
  m,
  idx,
  onExpand,
}: {
  m: Message;
  idx: number;
  onExpand: (idx: number) => void;
}) {
  const open = () => onExpand(idx);
  if (m.kind === "tool_use") {
    const summary = toolSummary(m.tool_name, m.tool_input);
    return (
      <Collapsible
        className="tool"
        truncated={m.truncated}
        onOpen={open}
        summary={
          <>
            <span className="tool-name">{m.tool_name}</span> <code className="tool-summary">{summary}</code>
          </>
        }
      >
        <pre>{typeof m.tool_input === "string" ? m.tool_input : JSON.stringify(m.tool_input, null, 2)}</pre>
      </Collapsible>
    );
  }
  if (m.kind === "tool_result") {
    const firstLine = m.text.split("\n").find((l) => l.trim()) ?? "(empty)";
    return (
      <Collapsible
        className={`tool result ${m.is_error ? "error-result" : ""}`}
        truncated={m.truncated}
        onOpen={open}
        summary={
          <>
            <span className="tool-name">{m.is_error ? "error" : "result"}</span>{" "}
            <code className="tool-summary">{firstLine.slice(0, 160)}</code>
          </>
        }
      >
        <pre>{m.text}</pre>
      </Collapsible>
    );
  }
  if (m.kind === "thinking") {
    return (
      <Collapsible className="thinking" truncated={m.truncated} onOpen={open} summary="Thinking">
        <div className="prose">
          <Markdown remarkPlugins={[remarkGfm]}>{m.text}</Markdown>
        </div>
      </Collapsible>
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
});

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
