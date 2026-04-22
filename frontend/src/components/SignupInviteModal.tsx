import React, { useEffect, useRef } from "react";

export type InviteMode = "soft" | "hard";

interface Props {
  open: boolean;
  mode: InviteMode;
  /**
   * Relative path the OAuth callback should redirect back to. Validated
   * server-side (same-origin, must start with `/`) so a crafted link can't
   * turn the flow into an open-redirect. We encode it into `return_to` and
   * trust the server to ignore anything it doesn't like.
   */
  returnTo?: string;
  onDismiss: () => void;
}

// The invite modal has two personas controlled by `mode`:
//
//  - soft: shown once, right after the user's first build finishes. Reads
//    like a celebratory "nice, save your progress" prompt and can be
//    dismissed by a "Maybe later" button or Esc.
//
//  - hard: shown when the anonymous user tries to send a second message.
//    No dismiss affordance — the chat input behind it is disabled, so the
//    only path forward is sign-in. Esc is a no-op; clicking the backdrop
//    is a no-op. Intentional friction.
export default function SignupInviteModal({
  open,
  mode,
  returnTo,
  onDismiss,
}: Props) {
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    dialogRef.current?.focus();

    if (mode !== "soft") return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onDismiss();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [open, mode, onDismiss]);

  if (!open) return null;

  const githubHref = returnTo
    ? `/auth/oauth/github?return_to=${encodeURIComponent(returnTo)}`
    : "/auth/oauth/github";

  const isHard = mode === "hard";

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="signup-invite-title"
      className="fixed inset-0 z-50 flex items-center justify-center p-4"
    >
      <button
        type="button"
        aria-label="Close"
        onClick={isHard ? undefined : onDismiss}
        tabIndex={-1}
        className={`absolute inset-0 bg-neutral-950/80 backdrop-blur-sm ${
          isHard ? "cursor-default" : "cursor-pointer"
        }`}
      />

      <div
        ref={dialogRef}
        tabIndex={-1}
        className="relative w-full max-w-md rounded-2xl border border-neutral-800 bg-neutral-950 p-8 shadow-2xl shadow-black/50 outline-none"
      >
        <div className="mb-5 flex h-12 w-12 items-center justify-center rounded-xl bg-linear-to-br from-indigo-500/20 to-purple-500/10 text-indigo-400">
          <svg
            width="22"
            height="22"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true"
          >
            <path d="M12 3l2.286 6.857L21 12l-5.714 2.143L13 21l-2.286-6.857L5 12l5.714-2.143L13 3z" />
          </svg>
        </div>

        <h2
          id="signup-invite-title"
          className="text-2xl font-bold tracking-tight text-neutral-100"
        >
          {isHard ? "Sign in to keep building" : "Save your progress"}
        </h2>

        <p className="mt-3 text-sm leading-relaxed text-neutral-400">
          {isHard
            ? "You've used your free anonymous message. Sign in with GitHub to keep iterating on this app and unlock unlimited builds."
            : "Nice — your first build is ready. Sign in with GitHub so we can save this project, your history, and let you come back to it later."}
        </p>

        <a
          href={githubHref}
          className="mt-7 flex w-full items-center justify-center gap-2.5 rounded-xl bg-white px-4 py-3 text-sm font-semibold text-neutral-900 transition hover:bg-neutral-200"
        >
          <svg
            className="h-5 w-5"
            viewBox="0 0 24 24"
            fill="currentColor"
            aria-hidden="true"
          >
            <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12" />
          </svg>
          Continue with GitHub
        </a>

        {isHard ? (
          <p className="mt-4 text-center text-xs text-neutral-600">
            We only request your public profile and primary email.
          </p>
        ) : (
          <button
            type="button"
            onClick={onDismiss}
            className="mt-3 w-full rounded-xl px-4 py-2.5 text-sm font-medium text-neutral-500 transition hover:text-neutral-300"
          >
            Maybe later
          </button>
        )}
      </div>
    </div>
  );
}
