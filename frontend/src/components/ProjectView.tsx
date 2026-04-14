import React, { useState, useEffect, useRef, useCallback } from "react";
import { PromptInputBox } from "./ui/ai-prompt-box";

const DEFAULT_ORG_ID = "00000000-0000-0000-0000-000000000001";
const DEFAULT_USER_ID = "00000000-0000-0000-0000-000000000001";

const POLL_INTERVAL_MS = 2_000;
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

// ── Preview panel ───────────────────────────────────────────────────────

function PreviewPanel({
  deploymentId,
  status,
}: {
  deploymentId: string | null;
  status: string | null;
}) {
  if (status === "succeeded" && deploymentId) {
    return (
      <div className="flex h-full flex-col">
        <div className="flex items-center justify-between border-b border-neutral-800 px-4 py-2">
          <div className="flex items-center gap-2">
            <span className="h-2 w-2 rounded-full bg-emerald-400" />
            <span className="text-xs font-medium text-neutral-400">Live preview</span>
          </div>
          <a
            href={`/env/${deploymentId}/`}
            target="_blank"
            rel="noopener noreferrer"
            className="text-xs text-neutral-500 hover:text-neutral-300 transition-colors"
          >
            Open ↗
          </a>
        </div>
        <iframe
          src={`/env/${deploymentId}/`}
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
          <span className="text-red-400 truncate max-w-xs">{job.error_message}</span>
        </>
      )}
    </div>
  );
}

// ── Main component ──────────────────────────────────────────────────────

export default function ProjectView({ projectId }: { projectId: string }) {
  const [tab, setTab] = useState<LeftTab>("chat");
  const [messages, setMessages] = useState<Message[]>([]);
  const [job, setJob] = useState<BuildJob | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);

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

  useEffect(() => stopPolling, [stopPolling]);

  // On mount: if job_id is in URL params, start polling immediately.
  // Otherwise, fetch the latest job for this project.
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
        const res = await fetch(`/api/build_jobs?sort=created_at&order=desc&limit=1&project_id=${projectId}`);
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
        const res = await fetch(`/api/messages?sort=created_at&order=asc&limit=100&conversation_id=${projectId}`);
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
