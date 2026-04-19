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

function formatToolLabel(tool: string, args?: unknown): string {
  const name = tool
    .replace(/^tool\./, "")
    .replace(/_/g, " ")
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .toLowerCase();

  if (args && typeof args === "object") {
    const a = args as Record<string, unknown>;
    if (typeof a.path === "string") return `${name}: ${a.path}`;
    if (typeof a.file === "string") return `${name}: ${a.file}`;
    if (typeof a.command === "string")
      return `${name}: ${a.command.toString().slice(0, 60)}`;
    if (typeof a.query === "string")
      return `${name}: "${a.query.toString().slice(0, 40)}"`;
  }
  return name;
}

interface BuildJob {
  id: string;
  status: string;
  error_message: string;
  duration_ms: number;
  logs: string;
  deployment_id: string | null;
  project_id: string;
}

/** OpenCode jobs may omit deployment_id in older rows; keep preview when refetching. */
function mergeBuildJob(prev: BuildJob | null, next: BuildJob): BuildJob {
  const prevLogs = prev?.logs ?? "";
  const nextLogs = next.logs ?? "";
  // Avoid wiping streamed / longer client logs if a refetch races an empty/partial DB row.
  const logs =
    nextLogs.length >= prevLogs.length ? nextLogs : prevLogs || nextLogs;

  return {
    ...next,
    deployment_id: next.deployment_id ?? prev?.deployment_id ?? null,
    logs,
  };
}

/** Several refetches catch races: logs/status updated on disk after build.complete SSE. */
function scheduleBuildJobRefetches(
  jobId: string,
  setJobFn: React.Dispatch<React.SetStateAction<BuildJob | null>>,
) {
  const delaysMs = [0, 500, 1500, 3500];
  for (const delay of delaysMs) {
    window.setTimeout(async () => {
      try {
        const r = await fetch(`/api/build_jobs/${jobId}`);
        if (!r.ok) return;
        const j: BuildJob = await r.json();
        setJobFn((p) => mergeBuildJob(p, j));
      } catch {
        /* ignore */
      }
    }, delay);
  }
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

// ── Thinking step type ──────────────────────────────────────────────────

interface ThinkingStep {
  id: string;
  kind: "tool" | "status";
  label: string;
  timestamp: number;
}

// ── Thinking bubble ────────────────────────────────────────────────────

function ThinkingBubble({
  steps,
  streamingText,
}: {
  steps: ThinkingStep[];
  streamingText: string;
}) {
  const [expanded, setExpanded] = useState(true);
  const isActive = !streamingText && steps.length > 0;

  if (steps.length === 0 && !streamingText) return null;

  return (
    <div className="flex justify-start">
      <div className="max-w-[85%] w-full space-y-2">
        {/* Thinking header + steps */}
        {steps.length > 0 && (
          <div className="rounded-xl bg-neutral-800/40 border border-neutral-800/60 overflow-hidden">
            <button
              onClick={() => setExpanded((v) => !v)}
              className="flex w-full items-center gap-2 px-3 py-2 text-xs text-neutral-500 hover:text-neutral-400 transition"
            >
              {isActive && (
                <span className="flex gap-0.5">
                  <span className="h-1 w-1 rounded-full bg-indigo-400 animate-bounce [animation-delay:0ms]" />
                  <span className="h-1 w-1 rounded-full bg-indigo-400 animate-bounce [animation-delay:150ms]" />
                  <span className="h-1 w-1 rounded-full bg-indigo-400 animate-bounce [animation-delay:300ms]" />
                </span>
              )}
              {!isActive && (
                <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
              )}
              <span className="font-medium">
                {isActive ? "Thinking…" : "Thought process"}
              </span>
              <span className="ml-auto text-neutral-600">
                {steps.length} step{steps.length !== 1 ? "s" : ""}
              </span>
              <svg
                width="12"
                height="12"
                viewBox="0 0 16 16"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                className={`transition-transform ${expanded ? "rotate-180" : ""}`}
              >
                <path d="M4 6l4 4 4-4" />
              </svg>
            </button>
            {expanded && (
              <div className="border-t border-neutral-800/60 px-3 py-2 space-y-1">
                {steps.map((step) => (
                  <div
                    key={step.id}
                    className="flex items-center gap-2 text-xs text-neutral-500"
                  >
                    {step.kind === "tool" ? (
                      <svg
                        width="11"
                        height="11"
                        viewBox="0 0 16 16"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="2"
                        className="shrink-0 text-amber-500/70"
                      >
                        <path d="M14.7 6.3a1 1 0 0 0 0-1.4l-1.6-1.6a1 1 0 0 0-1.4 0l-2 2L6 2 2 6l3.3 3.7-4 4a1 1 0 0 0 0 1.4l.6.6a1 1 0 0 0 1.4 0l4-4L11 15l4-4-3.3-2.7z" />
                      </svg>
                    ) : (
                      <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-indigo-400/60" />
                    )}
                    <span className="truncate">{step.label}</span>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}

        {/* Streaming response text */}
        {streamingText && (
          <div className="rounded-xl bg-neutral-800/60 px-4 py-2.5 text-sm leading-relaxed text-neutral-300 whitespace-pre-wrap">
            {streamingText}
            <span className="inline-block ml-0.5 w-1.5 h-4 bg-indigo-400/80 animate-pulse rounded-sm" />
          </div>
        )}
      </div>
    </div>
  );
}

// ── Chat panel ──────────────────────────────────────────────────────────

function DemoLimitBanner() {
  return (
    <div className="border-t border-amber-900/40 bg-amber-950/30 px-4 py-3">
      <div className="flex items-center gap-2 text-amber-400/90">
        <svg
          width="16"
          height="16"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          strokeLinejoin="round"
          className="shrink-0"
        >
          <path d="M8 1.5l6.5 12H1.5z" />
          <path d="M8 6v3" />
          <circle cx="8" cy="11.5" r="0.5" fill="currentColor" />
        </svg>
        <span className="text-xs font-medium">
          Free demo limit reached — upgrade to keep building.
        </span>
      </div>
    </div>
  );
}

function ChatPanel({
  messages,
  onSend,
  isLoading,
  thinkingSteps,
  streamingText,
  demoLimitReached,
}: {
  messages: Message[];
  onSend: (msg: string) => void;
  isLoading: boolean;
  thinkingSteps: ThinkingStep[];
  streamingText: string;
  demoLimitReached: boolean;
}) {
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages.length, thinkingSteps.length, streamingText]);

  return (
    <div className="flex h-full flex-col">
      <div className="flex-1 overflow-y-auto p-4 space-y-4 scrollbar-thin scrollbar-thumb-neutral-700 scrollbar-track-transparent">
        {messages.length === 0 &&
          thinkingSteps.length === 0 &&
          !streamingText && (
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
              className={`max-w-[85%] rounded-xl px-4 py-2.5 text-sm leading-relaxed whitespace-pre-wrap ${
                m.role === "user"
                  ? "bg-indigo-600/20 text-neutral-200"
                  : "bg-neutral-800/60 text-neutral-300"
              }`}
            >
              {m.content}
            </div>
          </div>
        ))}
        {(thinkingSteps.length > 0 || streamingText) && (
          <ThinkingBubble steps={thinkingSteps} streamingText={streamingText} />
        )}
        <div ref={bottomRef} />
      </div>
      {demoLimitReached ? (
        <DemoLimitBanner />
      ) : (
        <div className="border-t border-neutral-800 p-3">
          <PromptInputBox
            placeholder="Send a message…"
            onSend={(msg) => onSend(msg)}
            isLoading={isLoading}
          />
        </div>
      )}
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
  statusDetail,
  reloadNonce,
  isRestarting,
  onPreviewReady,
}: {
  deploymentId: string | null;
  status: string | null;
  /** Latest deploy / setup message while the container or dev server is coming up */
  statusDetail: string | null;
  /** Bumped by parent on `restart_healthy` to force a post-restart iframe reload.
   *  Must NOT be bumped on `build.complete` — Vite is still alive at that
   *  point and swapping the src then triggers a chunk-load against a
   *  dev server that's about to be killed. */
  reloadNonce: number;
  /** While true, Vite is being torn down + restarted. Covers the iframe with
   *  a friendly overlay so the user never sees the proxy's raw 502 response. */
  isRestarting: boolean;
  /** Called after a reload-triggered navigation finishes loading in the
   *  iframe. Lets the parent drop the restart overlay at exactly the moment
   *  the fresh document is visible — no flash of half-loaded content. */
  onPreviewReady: () => void;
}) {
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const [currentUrl, setCurrentUrl] = useState("");
  const [history, setHistory] = useState<string[]>([]);
  const [historyIdx, setHistoryIdx] = useState(-1);
  const lastNonceRef = useRef(reloadNonce);
  // True between a nonce bump and the next iframe `onLoad`. Used to scope
  // `onPreviewReady` to reload-triggered loads only, so normal in-iframe
  // navigation (user clicking links) doesn't accidentally drop the overlay.
  const pendingReloadRef = useRef(false);

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

  // When the parent signals a fresh build (nonce bump), force the iframe to
  // re-fetch. Vite HMR doesn't work through the proxy (no WebSocket upgrade),
  // so after the dev server restart the iframe would otherwise show stale HTML
  // (often still referencing assets from the killed Vite process).
  //
  // Important subtleties:
  // 1. Only bump `lastNonceRef` once we actually schedule a reload — otherwise
  //    if `currentUrl` is not yet set when the nonce arrives, the effect will
  //    return early, mark the nonce as consumed, and never retry when
  //    `currentUrl` populates a render later.
  // 2. Don't trust the health probe — it only proves Vite is listening, not
  //    that Astro finished compiling the first request. Poll the proxied
  //    `baseUrl` until it returns 200 before swapping the iframe's src.
  useEffect(() => {
    if (reloadNonce === lastNonceRef.current) return;
    if (!currentUrl || !baseUrl) return;
    lastNonceRef.current = reloadNonce;

    let cancelled = false;
    let attempt = 0;
    const maxAttempts = 25;

    const swapSrc = () => {
      const el = iframeRef.current;
      if (!el) return;
      // Flag the upcoming load as reload-triggered so `handleIframeLoad`
      // knows to call `onPreviewReady` (which drops the restart overlay).
      pendingReloadRef.current = true;
      const sep = currentUrl.includes("?") ? "&" : "?";
      el.src = `${currentUrl}${sep}__sc_reload=${reloadNonce}`;
    };

    const ping = () => {
      if (cancelled) return;
      attempt += 1;
      fetch(baseUrl, { cache: "no-store", credentials: "same-origin" })
        .then((r) => {
          if (cancelled) return;
          if (r.ok) {
            swapSrc();
            return;
          }
          if (attempt < maxAttempts) {
            window.setTimeout(ping, 800);
          } else {
            swapSrc();
          }
        })
        .catch(() => {
          if (cancelled) return;
          if (attempt < maxAttempts) {
            window.setTimeout(ping, 800);
          } else {
            swapSrc();
          }
        });
    };

    const t = window.setTimeout(ping, 300);
    return () => {
      cancelled = true;
      window.clearTimeout(t);
    };
  }, [reloadNonce, currentUrl, baseUrl]);

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
    // If this load was triggered by a reload-after-restart, the new
    // document is now painted — tell the parent to drop the overlay.
    // (Unconditional: a failed navigation would still fire `onLoad` with
    // a 502 body in the iframe, but covering that with an overlay
    // indefinitely is worse than showing the error.)
    if (pendingReloadRef.current) {
      pendingReloadRef.current = false;
      onPreviewReady();
    }
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
  }, [currentUrl, history, historyIdx, onPreviewReady]);

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
        <div className="relative flex-1">
          <iframe
            // key remounts the iframe on each build so a stale document from
            // before a dev-server restart never lingers. Without this, React
            // reuses the same element and the browser may serve a cached
            // response on repeat navigations to the same URL.
            key={reloadNonce}
            ref={iframeRef}
            src={currentUrl || baseUrl}
            onLoad={handleIframeLoad}
            className="h-full w-full bg-white"
            title="Live preview"
          />
          {isRestarting ? (
            // Intentionally NOT `pointer-events-none`: while Vite is down,
            // letting clicks pass through to the iframe would send them to
            // a dead dev server and queue up more 502s. Swallowing them
            // here is the correct behavior.
            <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 bg-neutral-950/90 text-neutral-200 backdrop-blur-sm">
              <div className="h-8 w-8 animate-spin rounded-full border-2 border-neutral-600 border-t-indigo-400" />
              <p className="text-sm font-medium">Applying changes…</p>
              <p className="max-w-xs text-center text-xs leading-relaxed text-neutral-400">
                {statusDetail ?? "Preview is restarting. It will refresh automatically."}
              </p>
            </div>
          ) : null}
        </div>
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
    <div className="flex h-full flex-col items-center justify-center gap-3 px-6 text-center text-neutral-600">
      {status === "running" && (
        <div className="h-8 w-8 animate-spin rounded-full border-2 border-neutral-700 border-t-indigo-500" />
      )}
      <p className="text-sm">{label}</p>
      {status === "running" && statusDetail ? (
        <p className="max-w-md text-xs leading-relaxed text-neutral-500">
          {statusDetail}
        </p>
      ) : null}
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

const LATEST_JOB_POLL_MS = 4_000;

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

interface DeployStatusEvent {
  event: "deploy_status";
  job_id: string;
  project_id: string;
  phase: string;
  message: string;
}

type BuildEvent =
  | BuildStatusEvent
  | MessageChunkEvent
  | ToolCallEvent
  | BuildCompleteEvent
  | BuildErrorEvent
  | DeployStatusEvent;

// ── Main component ──────────────────────────────────────────────────────

export default function ProjectView({ projectId }: { projectId: string }) {
  const [tab, setTab] = useState<LeftTab>("chat");
  const [messages, setMessages] = useState<Message[]>([]);
  const [job, setJob] = useState<BuildJob | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [streamingText, setStreamingText] = useState("");
  const [thinkingSteps, setThinkingSteps] = useState<ThinkingStep[]>([]);
  const [previewStatusDetail, setPreviewStatusDetail] = useState<string | null>(
    null,
  );
  const [previewReloadNonce, setPreviewReloadNonce] = useState(0);
  // Flips true while the deploy is being restarted (Vite killed → spawn →
  // health). During this window the proxy returns 502s because the upstream
  // dev server is genuinely down; surfacing those raw errors in the iframe
  // makes the app look broken even though the restart is proceeding
  // normally. `PreviewPanel` uses this to show an overlay instead.
  const [isPreviewRestarting, setIsPreviewRestarting] = useState(false);
  const [projectScope, setProjectScope] = useState<string>("frontend");
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const eventSourceRef = useRef<EventSource | null>(null);
  const streamingMsgIdRef = useRef<string | null>(null);

  const demoLimitReached = projectScope === "free";

  const fetchProjectScope = useCallback(async () => {
    try {
      const res = await fetch(`/api/projects/${projectId}`);
      if (!res.ok) return;
      const data: { scope?: string } = await res.json();
      if (data.scope) setProjectScope(data.scope);
    } catch {
      /* ignore */
    }
  }, [projectId]);

  useEffect(() => {
    fetchProjectScope();
  }, [fetchProjectScope]);

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
        setJob((prev) => {
          const merged = mergeBuildJob(prev, data);
          // Fallback path: SSE dropped, poller is the one seeing success.
          // Bump the preview nonce on the running→succeeded transition only,
          // so we don't thrash the iframe on every poll after completion.
          if (
            merged.status === "succeeded" &&
            prev?.status !== "succeeded" &&
            merged.deployment_id
          ) {
            setPreviewReloadNonce((n) => n + 1);
          }
          return merged;
        });
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
          prev ? { ...prev, id: data.job_id, status: data.status } : prev,
        );
        if (data.status === "running") {
          setIsLoading(true);
          setPreviewStatusDetail(null);
          setTab("chat");
          setThinkingSteps((steps) =>
            steps.length === 0
              ? [
                  {
                    id: crypto.randomUUID(),
                    kind: "status",
                    label: "Build started — setting up environment",
                    timestamp: Date.now(),
                  },
                ]
              : steps,
          );
          startPolling(data.job_id);
        }
      } catch {
        /* ignore parse errors */
      }
    });

    es.addEventListener("message.chunk", (e) => {
      try {
        const data: MessageChunkEvent = JSON.parse(e.data);
        setStreamingText((prev) => prev + data.text);

        // Append streaming text to logs too
        setJob((prev) =>
          prev ? { ...prev, logs: (prev.logs || "") + data.text } : prev,
        );
      } catch {
        /* ignore */
      }
    });

    es.addEventListener("tool.call", (e) => {
      try {
        const data: ToolCallEvent = JSON.parse(e.data);
        const logLine = `[tool] ${data.tool}\n`;
        setJob((prev) =>
          prev ? { ...prev, logs: (prev.logs || "") + logLine } : prev,
        );
        setThinkingSteps((prev) => [
          ...prev,
          {
            id: crypto.randomUUID(),
            kind: "tool" as const,
            label: formatToolLabel(data.tool, data.args),
            timestamp: Date.now(),
          },
        ]);
      } catch {
        /* ignore */
      }
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
        setThinkingSteps([]);
        setPreviewStatusDetail(null);

        setJob((prev) =>
          prev
            ? {
                ...prev,
                status: data.status,
              }
            : prev,
        );
        setIsLoading(false);

        fetchProjectScope();

        // Don't touch `previewReloadNonce` here: `build.complete` fires
        // *before* `restart_deployment_after_opencode_build` tears Vite
        // down (see run_build.rs: publish_event(BuildComplete) → await
        // restart). Bumping the nonce now would kick off the iframe
        // reload against the still-healthy old Vite, then Vite gets
        // killed mid-load and the iframe renders the proxy's raw
        // "upstream: error sending request …" 502 body. The reload is
        // instead triggered by `restart_healthy` below, so the iframe
        // only refetches once the new dev server is confirmed up.
        //
        // We intentionally do NOT clear `isPreviewRestarting` here either
        // — that's owned by the `deploy.status` handler (see below).

        if (data.job_id) {
          scheduleBuildJobRefetches(data.job_id, setJob);
          stopPolling();
        }
      } catch {
        /* ignore */
      }
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
        setThinkingSteps([]);
        streamingMsgIdRef.current = null;
        setPreviewStatusDetail(null);
        setIsPreviewRestarting(false);
      } catch {
        /* ignore */
      }
    });

    es.addEventListener("deploy.status", (e) => {
      try {
        const data: DeployStatusEvent = JSON.parse(e.data);
        setThinkingSteps((steps) => [
          ...steps,
          {
            id: crypto.randomUUID(),
            kind: "status" as const,
            label: data.message,
            timestamp: Date.now(),
          },
        ]);
        setPreviewStatusDetail(data.message);
        // Overlay + iframe-reload lifecycle.
        //
        // - restart_started: Vite is about to be killed. Pin the overlay
        //   over the iframe so the user never sees the in-flight 502s
        //   (and especially not the proxy's literal "upstream: …" body
        //   rendered as plaintext in the iframe).
        //
        // - restart_healthy: new Vite is up. NOW trigger the iframe
        //   reload by bumping `previewReloadNonce`. We keep the overlay
        //   up through the reload itself; PreviewPanel clears it via
        //   `onPreviewReady` once the iframe's onLoad fires, so there's
        //   no flash of a half-loaded document.
        //
        // - restart_failed: Vite never came back. Drop the overlay so
        //   the user isn't stuck staring at a spinner forever; the
        //   underlying 502 is the honest state and a later build can
        //   recover.
        //
        // - soft_reload: page edit that Vite's file watcher already
        //   picked up. Vite is still alive, so we just bump the nonce
        //   to refetch the iframe document. Crucially we do NOT flip
        //   `isPreviewRestarting`; there's no 502 window, and showing
        //   the heavy overlay for a sub-second reload looks broken.
        if (data.phase === "restart_started") {
          setIsPreviewRestarting(true);
        } else if (data.phase === "restart_healthy") {
          setPreviewReloadNonce((n) => n + 1);
        } else if (data.phase === "restart_failed") {
          setIsPreviewRestarting(false);
        } else if (data.phase === "soft_reload") {
          setPreviewReloadNonce((n) => n + 1);
        }
        setIsLoading(true);
      } catch {
        /* ignore */
      }
    });

    es.onerror = () => {
      // EventSource auto-reconnects; no action needed
    };

    return () => {
      es.close();
      eventSourceRef.current = null;
    };
  }, [projectId, startPolling, stopPolling, fetchProjectScope]);

  useEffect(() => stopPolling, [stopPolling]);

  // SSE can lag or drop under burst; merge latest row periodically (skip ?job= deep-link).
  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    if (params.get("job")) return;

    const t = window.setInterval(async () => {
      try {
        const r = await fetch(
          `/api/build_jobs?sort=created_at&order=desc&limit=1&project_id=${projectId}`,
        );
        if (!r.ok) return;
        const jobs: BuildJob[] = await r.json();
        if (jobs.length === 0) return;
        setJob((p) => mergeBuildJob(p, jobs[0]));
      } catch {
        /* ignore */
      }
    }, LATEST_JOB_POLL_MS);
    return () => clearInterval(t);
  }, [projectId]);

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

  // Load existing messages for this project.
  // First resolve the project's conversation, then fetch its messages.
  useEffect(() => {
    (async () => {
      try {
        // Find the conversation for this project
        const convRes = await fetch(
          `/api/conversations?project_id=${projectId}&limit=1&sort=created_at&order=desc`,
        );
        if (!convRes.ok) return;
        const convs: { id: string }[] = await convRes.json();
        if (convs.length === 0) return;

        const conversationId = convs[0].id;

        const res = await fetch(
          `/api/messages?sort=created_at&order=asc&limit=100&conversation_id=${conversationId}`,
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
    if (demoLimitReached) return;
    const optimistic: Message = {
      id: crypto.randomUUID(),
      role: "user",
      content,
      created_at: new Date().toISOString(),
    };
    setMessages((prev) => [...prev, optimistic]);
    setIsLoading(true);
    setStreamingText("");
    setThinkingSteps([]);
    setPreviewStatusDetail(null);
    setTab("chat");

    try {
      const res = await fetch("/api/systems/spawn_environment", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          org_id: DEFAULT_ORG_ID,
          user_id: DEFAULT_USER_ID,
          prompt: content,
          // Reuse checkout, deployment, and OpenCode session — do not spawn a new project.
          project_id: projectId,
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
                messages={messages}
                onSend={handleSend}
                isLoading={isLoading}
                thinkingSteps={thinkingSteps}
                streamingText={streamingText}
                demoLimitReached={demoLimitReached}
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
            statusDetail={previewStatusDetail}
            reloadNonce={previewReloadNonce}
            isRestarting={isPreviewRestarting}
            onPreviewReady={() => setIsPreviewRestarting(false)}
          />
        </div>
      </div>

      {/* Bottom status bar */}
      <StatusBar job={job} />
    </div>
  );
}
