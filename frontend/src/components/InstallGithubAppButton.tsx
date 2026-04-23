import React, { useEffect, useMemo, useState } from "react";

/**
 * Button that kicks off the GitHub App install flow. On mount it asks the
 * backend at `/github/app/info` whether the App is configured (i.e. the ops
 * team has wired GITHUB_APP_ID / PRIVATE_KEY / WEBHOOK_SECRET) and which
 * slug to point to — so a frontend build never has the App identity baked
 * in and we can swap Apps just by flipping env vars.
 *
 * Caller passes any context it wants round-tripped back after the install
 * completes (project id, a custom return_to, etc.). We base64url-encode it
 * into GitHub's `state` query param; the backend Setup URL handler decodes
 * it and uses it to pick the redirect target.
 *
 * Unlike OAuth repo tokens, installing the App grants scoped, revocable
 * access that the backend mints installation tokens from on demand. The
 * user is the only party authorized to grant that access, which is why this
 * must be a real top-level navigation (not an XHR).
 */
export interface InstallGithubAppButtonProps {
  /** Project id to return to after install completes. */
  projectId?: string;
  /** Stem Cell org id to attach the installation to. */
  orgId?: string;
  /** Stem Cell user id to record as the installer. */
  userId?: string;
  /**
   * Relative URL to redirect to after the Setup URL handler runs.
   * Overrides the default (`/project/<id>` or `/github/install`).
   * Must start with `/` — external URLs are stripped server-side.
   */
  returnTo?: string;
  /** Override button label. */
  label?: string;
  /** Override button className. */
  className?: string;
}

interface AppInfo {
  configured: boolean;
  slug: string | null;
}

function encodeState(
  state: Record<string, string | undefined>,
): string | undefined {
  // Drop undefined entries so the JSON payload stays compact.
  const cleaned = Object.fromEntries(
    Object.entries(state).filter(([, v]) => v !== undefined && v !== ""),
  );
  if (Object.keys(cleaned).length === 0) return undefined;
  const json = JSON.stringify(cleaned);
  // base64url, no padding — matches URL_SAFE_NO_PAD on the Rust side.
  const b64 = btoa(json)
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/, "");
  return b64;
}

export default function InstallGithubAppButton({
  projectId,
  orgId,
  userId,
  returnTo,
  label = "Install on GitHub",
  className,
}: InstallGithubAppButtonProps) {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const res = await fetch("/github/app/info", {
          credentials: "same-origin",
        });
        if (!res.ok) {
          if (!cancelled) setError(`HTTP ${res.status}`);
          return;
        }
        const data: AppInfo = await res.json();
        if (!cancelled) setInfo(data);
      } catch (e) {
        if (!cancelled)
          setError(e instanceof Error ? e.message : "network error");
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const installUrl = useMemo(() => {
    if (!info?.configured || !info.slug) return null;
    const state = encodeState({
      project_id: projectId,
      org_id: orgId,
      user_id: userId,
      return_to: returnTo,
    });
    const params = new URLSearchParams();
    if (state) params.set("state", state);
    const qs = params.toString();
    return `https://github.com/apps/${info.slug}/installations/new${
      qs ? `?${qs}` : ""
    }`;
  }, [info, projectId, orgId, userId, returnTo]);

  const baseClass =
    className ??
    "inline-flex items-center justify-center gap-2 rounded-lg border border-neutral-700 bg-neutral-900 px-4 py-2.5 text-sm font-semibold text-neutral-100 transition hover:border-neutral-600 hover:bg-neutral-800 disabled:cursor-not-allowed disabled:opacity-50";

  // Loading: show a subtle placeholder so the CTA height is stable.
  if (info === null && error === null) {
    return (
      <button type="button" className={baseClass} disabled>
        <GithubGlyph />
        <span className="text-neutral-400">Checking GitHub…</span>
      </button>
    );
  }

  if (error) {
    return (
      <div className="text-xs text-red-400">
        Could not check GitHub App status: {error}
      </div>
    );
  }

  if (!info || !info.configured || !info.slug) {
    return (
      <div className="rounded-lg border border-amber-900/50 bg-amber-950/30 px-3 py-2 text-xs text-amber-300">
        GitHub App is not configured on this server. Ask an admin to set
        <code className="mx-1 rounded bg-black/40 px-1">GITHUB_APP_ID</code>,
        private key, webhook secret, and slug.
      </div>
    );
  }

  return (
    <a href={installUrl ?? "#"} className={baseClass}>
      <GithubGlyph />
      {label}
    </a>
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
