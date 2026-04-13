import React, { useState } from "react";
import { PromptInputBox } from "./ui/ai-prompt-box";

// MVP: hardcoded seed-data IDs — replace with auth context later
const DEFAULT_ORG_ID = "00000000-0000-0000-0000-000000000001";
const DEFAULT_USER_ID = "00000000-0000-0000-0000-000000000001";

interface SpawnResult {
  project_id: string;
  job_id: string;
  status: string;
}

export default function HeroPrompt() {
  const [isLoading, setIsLoading] = useState(false);
  const [result, setResult] = useState<SpawnResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  const handleSend = async (message: string, _files?: File[]) => {
    setIsLoading(true);
    setError(null);
    setResult(null);

    try {
      const res = await fetch("/api/systems/spawn_environment", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          org_id: DEFAULT_ORG_ID,
          user_id: DEFAULT_USER_ID,
          prompt: message,
        }),
      });

      if (!res.ok) {
        const body = await res.text();
        throw new Error(body || `HTTP ${res.status}`);
      }

      const data: SpawnResult = await res.json();
      setResult(data);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Unknown error");
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <div className="w-full space-y-3">
      <PromptInputBox
        placeholder="Ask Stem Cell to create a landing page for my..."
        onSend={handleSend}
        isLoading={isLoading}
      />

      {result && (
        <div className="rounded-lg border border-green-700/30 bg-green-950/20 px-4 py-3 text-sm text-green-300">
          <p className="font-medium">Environment spawning</p>
          <p className="mt-1 font-mono text-xs text-green-400/70">
            project: {result.project_id} &middot; job: {result.job_id} &middot;{" "}
            {result.status}
          </p>
        </div>
      )}

      {error && (
        <div className="rounded-lg border border-red-700/30 bg-red-950/20 px-4 py-3 text-sm text-red-300">
          <p className="font-medium">Failed to spawn</p>
          <p className="mt-1 text-xs text-red-400/70">{error}</p>
        </div>
      )}
    </div>
  );
}
