import React, { useState, useEffect, useRef, useCallback } from "react";
import { PromptInputBox } from "./ui/ai-prompt-box";

const DEFAULT_ORG_ID = "00000000-0000-0000-0000-000000000001";
const DEFAULT_USER_ID = "00000000-0000-0000-0000-000000000001";

function getHostParam(key: string): string | null {
  return new URLSearchParams(window.location.search).get(key);
}

function setHostParam(key: string, value: string | null) {
  const url = new URL(window.location.href);
  if (value) url.searchParams.set(key, value);
  else url.searchParams.delete(key);
  window.history.replaceState(null, "", url.toString());
}

const POLL_INTERVAL_MS = 5_000;
const TERMINAL_STATUSES = new Set(["succeeded", "failed", "stopped"]);

interface BuildJob {
  id: string;
  status: string;
  error_message: string;
  duration_ms: number;
  logs: string;
  deployment_id: string | null;
  project_id: string;
}

interface Message {
  id: string;
  role: string;
  content: string;
  created_at: string;
}

// ── Tabs ────────────────────────────────────────────────────────────────

type LeftTab = "chat" | "logs";

function TabBar({
  active,
  onChange,
  hasLogs,
}: {
  active: LeftTab;
  onChange: (t: LeftTab) => void;
  hasLogs: boolean;
}) {
  return (
    <div className="flex border-b border-neutral-800">
      <button
        onClick={() => onChange("chat")}
        className={`px-4 py-2.5 text-xs font-medium transition ${
          active === "chat"
            ? "border-b-2 border-indigo-500 text-neutral-100"
            : "text-neutral-500 hover:text-neutral-300"
        }`}
      >
        Chat
      </button>
      <button
        onClick={() => onChange("logs")}
        className={`relative px-4 py-2.5 text-xs font-medium transition ${
          active === "logs"
            ? "border-b-2 border-indigo-500 text-neutral-100"
            : "text-neutral-500 hover:text-neutral-300"
        }`}
      >
        Logs
        {hasLogs && active !== "logs" && (
          <span className="absolute top-2 right-2 h-1.5 w-1.5 rounded-full bg-emerald-400 animate-pulse" />
        )}
      </button>
    </div>
  );
}

// ── Log viewer ──────────────────────────────────────────────────────────

function LogViewer({ logs }: { logs: string }) {
  const ref = useRef<HTMLPreElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [logs]);

  if (!logs) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-neutral-600">
        No logs yet — submit a prompt to start a build.
      </div>
    );
  }

  return (
    <pre
      ref={ref}
      className="h-full overflow-y-auto p-4 font-mono text-[11px] leading-relaxed text-green-400 scrollbar-thin scrollbar-thumb-neutral-700 scrollbar-track-transparent"
    >
      {logs}
    </pre>
  );
}

// ── Chat panel ──────────────────────────────────────────────────────────

function ChatPanel({
  messages,
  onSend,
  isLoading,
}: {
  messages: Message[];
  onSend: (msg: string) => void;
  isLoading: boolean;
}) {
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages.length]);

  return (
    <div className="flex h-full flex-col">
      <div className="flex-1 overflow-y-auto p-4 space-y-4 scrollbar-thin scrollbar-thumb-neutral-700 scrollbar-track-transparent">
        {messages.length === 0 && (
          <div className="flex h-full items-center justify-center text-sm text-neutral-600">
            Describe what you want to build.
          </div>
        )}
        {messages.map((m) => (
          <div
            key={m.id}
            className={`flex ${m.role === "user" ? "justify-end" : "justify-start"}`}
          >
            <div
              className={`max-w-[85%] rounded-xl px-4 py-2.5 text-sm leading-relaxed ${
                m.role === "user"
                  ? "bg-indigo-600/20 text-neutral-200"
                  : "bg-neutral-800/60 text-neutral-300"
              }`}
            >
              {m.content}
            </div>
          </div>
        ))}
        <div ref={bottomRef} />
      </div>
      <div className="border-t border-neutral-800 p-3">
        <PromptInputBox
          placeholder="Send a message…"
          onSend={(msg) => onSend(msg)}
          isLoading={isLoading}
        />
      </div>
    </div>
  );
}

// ── URL bar ─────────────────────────────────────────────────────────────

function UrlBar({
  url,
  onNavigate,
  onRefresh,
  onBack,
  onForward,
  canGoBack,
  canGoForward,
}: {
  url: string;
  onNavigate: (url: string) => void;
  onRefresh: () => void;
  onBack: () => void;
  onForward: () => void;
  canGoBack: boolean;
  canGoForward: boolean;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(url);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!editing) setDraft(url);
  }, [url, editing]);

  const commit = () => {
    setEditing(false);
    const trimmed = draft.trim();
    if (trimmed && trimmed !== url) onNavigate(trimmed);
  };

  const navBtnClass = (enabled: boolean) =>
    `flex h-7 w-7 items-center justify-center rounded-md transition ${
      enabled
        ? "text-neutral-400 hover:bg-neutral-800 hover:text-neutral-200"
        : "text-neutral-700 cursor-default"
    }`;

  return (
    <div className="flex items-center gap-1.5 border-b border-neutral-800 bg-neutral-950/80 px-2 py-1.5">
      <button
        onClick={onBack}
        disabled={!canGoBack}
        className={navBtnClass(canGoBack)}
        title="Back"
      >
        <svg
          width="14"
          height="14"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <path d="M10 3L5 8l5 5" />
        </svg>
      </button>
      <button
        onClick={onForward}
        disabled={!canGoForward}
        className={navBtnClass(canGoForward)}
        title="Forward"
      >
        <svg
          width="14"
          height="14"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <path d="M6 3l5 5-5 5" />
        </svg>
      </button>
      <button
        onClick={onRefresh}
        className="flex h-7 w-7 items-center justify-center rounded-md text-neutral-400 hover:bg-neutral-800 hover:text-neutral-200 transition"
        title="Refresh"
      >
        <svg
          width="14"
          height="14"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <path d="M1.5 1.5v4h4" />
          <path d="M2.3 9.5a6 6 0 1 0 .8-4L1.5 5.5" />
        </svg>
      </button>

      <div className="relative flex flex-1 items-center">
        <div className="pointer-events-none absolute left-2.5 text-neutral-600">
          <svg
            width="12"
            height="12"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
          >
            <circle cx="8" cy="8" r="5.5" />
            <path d="M8 5v0M8 7v4" />
          </svg>
        </div>
        <input
          ref={inputRef}
          type="text"
          value={editing ? draft : url}
          onChange={(e) => setDraft(e.target.value)}
          onFocus={() => {
            setEditing(true);
            setTimeout(() => inputRef.current?.select(), 0);
          }}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit();
            if (e.key === "Escape") {
              setDraft(url);
              setEditing(false);
              inputRef.current?.blur();
            }
          }}
          className="h-7 w-full rounded-md border border-neutral-800 bg-neutral-900 pl-8 pr-2 text-xs text-neutral-300 outline-none transition focus:border-neutral-600 focus:bg-neutral-900/80 placeholder:text-neutral-700"
          spellCheck={false}
        />
      </div>

      <a
        href={url}
        target="_blank"
        rel="noopener noreferrer"
        className="flex h-7 w-7 items-center justify-center rounded-md text-neutral-400 hover:bg-neutral-800 hover:text-neutral-200 transition"
        title="Open in new tab"
      >
        <svg
          width="14"
          height="14"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <path d="M9 2h5v5" />
          <path d="M14 2L7 9" />
          <path d="M13 9v4a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h4" />
        </svg>
      </a>
    </div>
  );
}

// ── Preview panel ───────────────────────────────────────────────────────

function PreviewPanel({
  deploymentId,
  status,
}: {
  deploymentId: string | null;
  status: string | null;
}) {
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const [currentUrl, setCurrentUrl] = useState("");
  const [history, setHistory] = useState<string[]>([]);
  const [historyIdx, setHistoryIdx] = useState(-1);

  const baseUrl = deploymentId ? `/env/${deploymentId}/` : "";

  // Initialise when deployment becomes available; restore from ?preview= if present
  useEffect(() => {
    if (!baseUrl) return;
    const restored = getHostParam("preview");
    const initial =
      restored && restored.startsWith(baseUrl) ? restored : baseUrl;
    setCurrentUrl(initial);
    setHistory([initial]);
    setHistoryIdx(0);
  }, [baseUrl]);

  // Persist preview path in host URL whenever it changes
  useEffect(() => {
    if (currentUrl && baseUrl) {
      setHostParam("preview", currentUrl === baseUrl ? null : currentUrl);
    }
  }, [currentUrl, baseUrl]);

  const navigateTo = useCallback(
    (url: string) => {
      const next = history.slice(0, historyIdx + 1);
      next.push(url);
      setHistory(next);
      setHistoryIdx(next.length - 1);
      setCurrentUrl(url);
      if (iframeRef.current) iframeRef.current.src = url;
    },
    [history, historyIdx],
  );

  const handleRefresh = useCallback(() => {
    if (iframeRef.current) {
      iframeRef.current.src = currentUrl;
    }
  }, [currentUrl]);

  const handleBack = useCallback(() => {
    if (historyIdx > 0) {
      const prev = historyIdx - 1;
      setHistoryIdx(prev);
      setCurrentUrl(history[prev]);
      if (iframeRef.current) iframeRef.current.src = history[prev];
    }
  }, [history, historyIdx]);

  const handleForward = useCallback(() => {
    if (historyIdx < history.length - 1) {
      const next = historyIdx + 1;
      setHistoryIdx(next);
      setCurrentUrl(history[next]);
      if (iframeRef.current) iframeRef.current.src = history[next];
    }
  }, [history, historyIdx]);

  // Track same-origin iframe navigation via load events
  const handleIframeLoad = useCallback(() => {
    try {
      const loc = iframeRef.current?.contentWindow?.location.pathname;
      if (loc && loc !== currentUrl) {
        const next = history.slice(0, historyIdx + 1);
        next.push(loc);
        setHistory(next);
        setHistoryIdx(next.length - 1);
        setCurrentUrl(loc);
      }
    } catch {
      // cross-origin — ignore
    }
  }, [currentUrl, history, historyIdx]);

  if (status === "succeeded" && deploymentId) {
    return (
      <div className="flex h-full flex-col">
        <UrlBar
          url={currentUrl}
          onNavigate={navigateTo}
          onRefresh={handleRefresh}
          onBack={handleBack}
          onForward={handleForward}
          canGoBack={historyIdx > 0}
          canGoForward={historyIdx < history.length - 1}
        />
        <iframe
          ref={iframeRef}
          src={currentUrl || baseUrl}
          onLoad={handleIframeLoad}
          className="flex-1 bg-white"
          title="Live preview"
        />
      </div>
    );
  }

  const label =
    status === "running"
      ? "Building…"
      : status === "failed"
        ? "Build failed"
        : "Waiting for build";

  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 text-neutral-600">
      {status === "running" && (
        <div className="h-8 w-8 animate-spin rounded-full border-2 border-neutral-700 border-t-indigo-500" />
      )}
      <p className="text-sm">{label}</p>
    </div>
  );
}

// ── Status bar ──────────────────────────────────────────────────────────

function StatusBar({ job }: { job: BuildJob | null }) {
  if (!job) return null;

  const color =
    job.status === "succeeded"
      ? "bg-emerald-400"
      : job.status === "failed"
        ? "bg-red-400"
        : "bg-indigo-400 animate-pulse";

  return (
    <div className="flex items-center gap-3 border-t border-neutral-800 px-4 py-1.5 text-[11px] text-neutral-500">
      <span className={`h-1.5 w-1.5 rounded-full ${color}`} />
      <span className="capitalize">{job.status}</span>
      <span className="text-neutral-700">|</span>
      <span className="font-mono">{job.id.slice(0, 8)}</span>
      {job.duration_ms > 0 && (
        <>
          <span className="text-neutral-700">|</span>
          <span>{(job.duration_ms / 1000).toFixed(1)}s</span>
        </>
      )}
      {job.error_message && (
        <>
          <span className="text-neutral-700">|</span>
          <span className="text-red-400 truncate max-w-xs">
            {job.error_message}
          </span>
        </>
      )}
    </div>
  );
}

// ── SSE event types ──────────────────────────────────────────────────────

interface BuildStatusEvent {
  event: "build_status";
  job_id: string;
  status: string;
}

interface MessageChunkEvent {
  event: "message_chunk";
  job_id: string;
  text: string;
}

interface ToolCallEvent {
  event: "tool_call";
  job_id: string;
  tool: string;
  args?: unknown;
}

interface BuildCompleteEvent {
  event: "build_complete";
  job_id: string;
  status: string;
  artifacts_count: number;
  tokens_used: number;
}

interface BuildErrorEvent {
  event: "build_error";
  job_id: string;
  error: string;
}

type BuildEvent =
  | BuildStatusEvent
  | MessageChunkEvent
  | ToolCallEvent
  | BuildCompleteEvent
  | BuildErrorEvent;

// ── Main component ──────────────────────────────────────────────────────

export default function ProjectView({ projectId }: { projectId: string }) {
  const [tab, setTab] = useState<LeftTab>("chat");
  const [messages, setMessages] = useState<Message[]>([]);
  const [job, setJob] = useState<BuildJob | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [streamingText, setStreamingText] = useState("");
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const eventSourceRef = useRef<EventSource | null>(null);
  const streamingMsgIdRef = useRef<string | null>(null);

  const stopPolling = useCallback(() => {
    if (timerRef.current) {
      clearInterval(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  const pollJob = useCallback(
    async (jobId: string) => {
      try {
        const res = await fetch(`/api/build_jobs/${jobId}`);
        if (!res.ok) return;
        const data: BuildJob = await res.json();
        setJob(data);
        if (TERMINAL_STATUSES.has(data.status)) {
          stopPolling();
          setIsLoading(false);
        }
      } catch {
        /* keep polling */
      }
    },
    [stopPolling],
  );

  const startPolling = useCallback(
    (jobId: string) => {
      stopPolling();
      pollJob(jobId);
      timerRef.current = setInterval(() => pollJob(jobId), POLL_INTERVAL_MS);
    },
    [pollJob, stopPolling],
  );

  // ── SSE connection ────────────────────────────────────────
  useEffect(() => {
    const es = new EventSource(`/api/projects/${projectId}/events`);
    eventSourceRef.current = es;

    es.addEventListener("build.status", (e) => {
      try {
        const data: BuildStatusEvent = JSON.parse(e.data);
        setJob((prev) =>
          prev ? { ...prev, status: data.status } : prev,
        );
        if (data.status === "running") {
          setIsLoading(true);
        }
      } catch { /* ignore parse errors */ }
    });

    es.addEventListener("message.chunk", (e) => {
      try {
        const data: MessageChunkEvent = JSON.parse(e.data);
        setStreamingText((prev) => prev + data.text);

        // Append streaming text to logs too
        setJob((prev) =>
          prev ? { ...prev, logs: (prev.logs || "") + data.text } : prev,
        );
      } catch { /* ignore */ }
    });

    es.addEventListener("tool.call", (e) => {
      try {
        const data: ToolCallEvent = JSON.parse(e.data);
        const logLine = `[tool] ${data.tool}\n`;
        setJob((prev) =>
          prev ? { ...prev, logs: (prev.logs || "") + logLine } : prev,
        );
      } catch { /* ignore */ }
    });

    es.addEventListener("build.complete", (e) => {
      try {
        const data: BuildCompleteEvent = JSON.parse(e.data);

        // Finalize the streaming assistant message
        setStreamingText((current) => {
          if (current) {
            const finalMsg: Message = {
              id: crypto.randomUUID(),
              role: "assistant",
              content: current,
              created_at: new Date().toISOString(),
            };
            setMessages((prev) => [...prev, finalMsg]);
          }
          return "";
        });
        streamingMsgIdRef.current = null;

        setJob((prev) =>
          prev
            ? {
                ...prev,
                status: data.status,
              }
            : prev,
        );
        setIsLoading(false);

        // Refresh full job data from API for deployment_id, etc.
        if (data.job_id) {
          fetch(`/api/build_jobs/${data.job_id}`)
            .then((r) => r.json())
            .then((j: BuildJob) => setJob(j))
            .catch(() => {});
        }
      } catch { /* ignore */ }
    });

    es.addEventListener("build.error", (e) => {
      try {
        const data: BuildErrorEvent = JSON.parse(e.data);
        setJob((prev) =>
          prev
            ? { ...prev, status: "failed", error_message: data.error }
            : prev,
        );
        setIsLoading(false);
        setStreamingText("");
        streamingMsgIdRef.current = null;
      } catch { /* ignore */ }
    });

    es.onerror = () => {
      // EventSource auto-reconnects; no action needed
    };

    return () => {
      es.close();
      eventSourceRef.current = null;
    };
  }, [projectId]);

  useEffect(() => stopPolling, [stopPolling]);

  // On mount: fetch the latest job for this project (fallback)
  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const jobIdParam = params.get("job");

    if (jobIdParam) {
      setIsLoading(true);
      setTab("logs");
      startPolling(jobIdParam);
      return;
    }

    (async () => {
      try {
        const res = await fetch(
          `/api/build_jobs?sort=created_at&order=desc&limit=1&project_id=${projectId}`,
        );
        if (!res.ok) return;
        const jobs: BuildJob[] = await res.json();
        if (jobs.length > 0) {
          const latest = jobs[0];
          setJob(latest);
          if (!TERMINAL_STATUSES.has(latest.status)) {
            setIsLoading(true);
            startPolling(latest.id);
          }
        }
      } catch {
        /* ignore */
      }
    })();
  }, [projectId, startPolling]);

  // Load existing messages for this project
  useEffect(() => {
    (async () => {
      try {
        const res = await fetch(
          `/api/messages?sort=created_at&order=asc&limit=100&conversation_id=${projectId}`,
        );
        if (!res.ok) return;
        const msgs: Message[] = await res.json();
        if (msgs.length > 0) setMessages(msgs);
      } catch {
        /* ignore */
      }
    })();
  }, [projectId]);

  const handleSend = async (content: string) => {
    const optimistic: Message = {
      id: crypto.randomUUID(),
      role: "user",
      content,
      created_at: new Date().toISOString(),
    };
    setMessages((prev) => [...prev, optimistic]);
    setIsLoading(true);
    setStreamingText("");
    setTab("logs");

    try {
      const res = await fetch("/api/systems/spawn_environment", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          org_id: DEFAULT_ORG_ID,
          user_id: DEFAULT_USER_ID,
          prompt: content,
        }),
      });
      if (!res.ok) throw new Error(await res.text());
      const { job_id } = await res.json();
      // Start polling as fallback — SSE is the primary update channel
      startPolling(job_id);
    } catch (err) {
      setIsLoading(false);
    }
  };

  // Combine persisted messages with the live streaming message
  const displayMessages = streamingText
    ? [
        ...messages,
        {
          id: "streaming",
          role: "assistant",
          content: streamingText,
          created_at: new Date().toISOString(),
        },
      ]
    : messages;

  return (
    <div className="flex h-full flex-col">
      {/* Top bar */}
      <header className="flex items-center justify-between border-b border-neutral-800 bg-neutral-950/80 px-4 py-2 backdrop-blur">
        <div className="flex items-center gap-3">
          <a
            href="/"
            className="text-sm font-bold tracking-tight text-neutral-100 hover:text-indigo-400 transition"
          >
            Stem Cell
          </a>
          <span className="text-neutral-700">/</span>
          <span className="text-xs font-mono text-neutral-500">
            {projectId.slice(0, 8)}
          </span>
        </div>
        {job?.status === "succeeded" && job.deployment_id && (
          <div className="flex items-center gap-2">
            <span className="h-1.5 w-1.5 rounded-full bg-emerald-400" />
            <span className="text-xs text-emerald-400">Live</span>
          </div>
        )}
      </header>

      {/* Main area: left panel + right preview */}
      <div className="flex flex-1 overflow-hidden">
        {/* Left panel */}
        <div className="flex w-[420px] min-w-[320px] flex-col border-r border-neutral-800 bg-neutral-950">
          <TabBar active={tab} onChange={setTab} hasLogs={!!job?.logs} />
          <div className="flex-1 overflow-hidden">
            {tab === "chat" ? (
              <ChatPanel
                messages={displayMessages}
                onSend={handleSend}
                isLoading={isLoading}
              />
            ) : (
              <LogViewer logs={job?.logs ?? ""} />
            )}
          </div>
        </div>

        {/* Right panel: preview */}
        <div className="flex-1 bg-neutral-900">
          <PreviewPanel
            deploymentId={job?.deployment_id ?? null}
            status={job?.status ?? null}
          />
        </div>
      </div>

      {/* Bottom status bar */}
      <StatusBar job={job} />
    </div>
  );
}
