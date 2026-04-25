import React, { useEffect, useMemo, useState } from "react";

export interface InstallGithubAppButtonProps {
  projectId?: string;
  orgId?: string;
  userId?: string;
  returnTo?: string;
  label?: string;
  className?: string;
}

interface InstallationSummary {
  id: string;
  account_login: string;
  target_type: string;
  status: string;
  active: boolean;
}

interface AppStatus {
  configured: boolean;
  slug: string | null;
  authenticated: boolean;
  github_login: string | null;
  installations: InstallationSummary[];
}

interface SyncResponse {
  synced: boolean;
  github_login: string | null;
  installation: InstallationSummary | null;
  message: string;
}

type Phase = "loading" | "ready" | "syncing" | "error";

const PRIMARY_ACTION_CLASS =
  "inline-flex items-center justify-center rounded-2xl bg-emerald-300 px-5 py-3 text-sm font-semibold text-zinc-950 shadow-lg shadow-emerald-950/30 transition hover:bg-emerald-200";
const SECONDARY_ACTION_CLASS =
  "inline-flex items-center justify-center rounded-2xl border border-zinc-700 bg-zinc-900 px-5 py-3 text-sm font-semibold text-zinc-100 transition hover:border-zinc-500 hover:bg-zinc-800";

function encodeState(
  state: Record<string, string | undefined>,
): string | undefined {
  const cleaned = Object.fromEntries(
    Object.entries(state).filter(([, v]) => v !== undefined && v !== ""),
  );
  if (Object.keys(cleaned).length === 0) return undefined;
  return btoa(JSON.stringify(cleaned))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/, "");
}

function currentReturnTo(projectId?: string, returnTo?: string) {
  if (returnTo) return returnTo;
  if (projectId) return `/project/${projectId}`;
  return "/settings";
}

export default function InstallGithubAppButton({
  projectId,
  orgId,
  userId,
  returnTo,
  label = "Install GitHub App",
  className,
}: InstallGithubAppButtonProps) {
  const [phase, setPhase] = useState<Phase>("loading");
  const [status, setStatus] = useState<AppStatus | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const refreshStatus = async () => {
    const res = await fetch("/github/app/status", { credentials: "same-origin" });
    if (!res.ok) throw new Error(`status HTTP ${res.status}`);
    const data: AppStatus = await res.json();
    setStatus(data);
    setPhase("ready");
  };

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const res = await fetch("/github/app/status", {
          credentials: "same-origin",
        });
        if (!res.ok) throw new Error(`status HTTP ${res.status}`);
        const data: AppStatus = await res.json();
        if (!cancelled) {
          setStatus(data);
          setPhase("ready");
        }
      } catch (e) {
        if (!cancelled) {
          setNotice(e instanceof Error ? e.message : "network error");
          setPhase("error");
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const installUrl = useMemo(() => {
    if (!status?.configured || !status.slug) return null;
    const state = encodeState({
      project_id: projectId,
      org_id: orgId,
      user_id: userId,
      return_to: currentReturnTo(projectId, returnTo),
    });
    const params = new URLSearchParams();
    if (state) params.set("state", state);
    const qs = params.toString();
    return `https://github.com/apps/${status.slug}/installations/new${
      qs ? `?${qs}` : ""
    }`;
  }, [status, projectId, orgId, userId, returnTo]);

  const githubLoginUrl = useMemo(() => {
    const params = new URLSearchParams({
      return_to: currentReturnTo(projectId, returnTo),
    });
    return `/auth/oauth/github?${params.toString()}`;
  }, [projectId, returnTo]);

  const connected = status?.installations.find((i) => i.active);

  const syncExisting = async () => {
    setPhase("syncing");
    setNotice(null);
    try {
      const res = await fetch("/github/app/sync", {
        method: "POST",
        credentials: "same-origin",
      });
      const data: SyncResponse = await res.json().catch(() => ({
        synced: false,
        github_login: null,
        installation: null,
        message: `Sync failed with HTTP ${res.status}`,
      }));
      setNotice(data.message);
      await refreshStatus();
    } catch (e) {
      setNotice(e instanceof Error ? e.message : "sync failed");
      setPhase("error");
    }
  };

  const shellClass =
    className ??
    "relative overflow-hidden rounded-[2rem] border border-zinc-800 bg-[#08090b] p-6 text-left shadow-2xl shadow-black/40";

  return (
    <section className={shellClass}>
      <div className="pointer-events-none absolute -right-20 -top-20 h-52 w-52 rounded-full bg-emerald-500/10 blur-3xl" />
      <div className="pointer-events-none absolute -bottom-24 left-1/3 h-56 w-56 rounded-full bg-sky-500/10 blur-3xl" />

      <div className="relative space-y-5">
        <div className="flex items-start gap-4">
          <div className="grid h-12 w-12 shrink-0 place-items-center rounded-2xl border border-zinc-700 bg-zinc-950 text-zinc-100">
            <GithubGlyph />
          </div>
          <div className="min-w-0">
            <p className="text-xs font-semibold uppercase tracking-[0.28em] text-emerald-300">
              GitHub storage
            </p>
            <h2 className="mt-1 text-xl font-semibold tracking-tight text-zinc-50">
              Sync first. Install only if needed.
            </h2>
            <p className="mt-2 text-sm leading-6 text-zinc-400">
              Stem Cell uses your GitHub sign-in to find an existing App
              installation, then uses the App for short-lived repo access.
            </p>
          </div>
        </div>

        {phase === "loading" && <StatePill tone="muted">Checking connection...</StatePill>}

        {phase === "error" && (
          <StatePill tone="danger">
            Could not read GitHub connection state: {notice}
          </StatePill>
        )}

        {status && !status.configured && (
          <StatePill tone="warn">
            Server is missing GitHub App credentials. Set App ID, slug, private
            key, and webhook secret before connecting storage.
          </StatePill>
        )}

        {status?.configured && !status.authenticated && (
          <ActionBlock
            eyebrow="Step 1"
            title="Sign in with GitHub"
            body="We need your GitHub login only to match you to an existing App installation. Repo operations still use the GitHub App."
          >
            <a className={PRIMARY_ACTION_CLASS} href={githubLoginUrl}>
              Continue with GitHub
            </a>
          </ActionBlock>
        )}

        {status?.configured && status.authenticated && !status.github_login && (
          <ActionBlock
            eyebrow="Step 1"
            title="Link a GitHub identity"
            body="You are signed in, but this account does not have a GitHub OAuth link yet."
          >
            <a className={PRIMARY_ACTION_CLASS} href={githubLoginUrl}>
              Link GitHub account
            </a>
          </ActionBlock>
        )}

        {status?.configured && status.authenticated && status.github_login && connected && (
          <ActionBlock
            eyebrow="Connected"
            title={`${connected.account_login} is ready`}
            body="Successful project edits will create or update a GitHub branch and open a pull request automatically."
          >
            <div className="rounded-2xl border border-emerald-400/20 bg-emerald-400/10 px-4 py-3 text-sm text-emerald-200">
              Active {connected.target_type.toLowerCase()} installation ·{" "}
              {connected.status}
            </div>
          </ActionBlock>
        )}

        {status?.configured &&
          status.authenticated &&
          status.github_login &&
          !connected && (
            <ActionBlock
              eyebrow="Step 2"
              title={`Find the App install for ${status.github_login}`}
              body="If Taller DIY is already installed on your personal account, sync it here and skip GitHub's install settings trap."
            >
              <div className="flex flex-col gap-3 sm:flex-row">
                <button
                  type="button"
                  className={`${PRIMARY_ACTION_CLASS} disabled:cursor-wait disabled:opacity-60`}
                  disabled={phase === "syncing"}
                  onClick={syncExisting}
                >
                  {phase === "syncing" ? "Syncing..." : "Sync existing install"}
                </button>
                {installUrl && (
                  <a className={SECONDARY_ACTION_CLASS} href={installUrl}>
                    {label}
                  </a>
                )}
              </div>
            </ActionBlock>
          )}

        {notice && phase !== "error" && (
          <p className="rounded-2xl border border-zinc-800 bg-zinc-950/70 px-4 py-3 text-sm text-zinc-300">
            {notice}
          </p>
        )}

        <div className="grid gap-3 border-t border-zinc-800 pt-5 text-xs text-zinc-500 sm:grid-cols-3">
          <Step label="OAuth" value={status?.github_login ?? "identity"} />
          <Step label="App" value={connected ? "connected" : "scoped access"} />
          <Step label="Repo" value="one project, one repo" />
        </div>
      </div>
    </section>
  );
}

function ActionBlock({
  eyebrow,
  title,
  body,
  children,
}: {
  eyebrow: string;
  title: string;
  body: string;
  children: React.ReactNode;
}) {
  return (
    <div className="rounded-3xl border border-zinc-800 bg-zinc-950/70 p-5">
      <p className="text-xs font-semibold uppercase tracking-[0.22em] text-zinc-500">
        {eyebrow}
      </p>
      <h3 className="mt-2 text-lg font-semibold text-zinc-100">{title}</h3>
      <p className="mt-2 max-w-xl text-sm leading-6 text-zinc-400">{body}</p>
      <div className="mt-4">{children}</div>
    </div>
  );
}

function StatePill({
  tone,
  children,
}: {
  tone: "muted" | "warn" | "danger";
  children: React.ReactNode;
}) {
  const cls =
    tone === "danger"
      ? "border-red-400/20 bg-red-500/10 text-red-200"
      : tone === "warn"
        ? "border-amber-400/20 bg-amber-500/10 text-amber-200"
        : "border-zinc-700 bg-zinc-900 text-zinc-300";
  return (
    <div className={`rounded-2xl border px-4 py-3 text-sm ${cls}`}>
      {children}
    </div>
  );
}

function Step({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <p className="uppercase tracking-[0.2em]">{label}</p>
      <p className="mt-1 font-medium text-zinc-300">{value}</p>
    </div>
  );
}

function GithubGlyph() {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 24 24"
      fill="currentColor"
      className="h-5 w-5"
    >
      <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12" />
    </svg>
  );
}
