import React, { useState } from "react";
import { PromptInputBox } from "./ui/ai-prompt-box";

const DEFAULT_ORG_ID = "00000000-0000-0000-0000-000000000001";
const DEFAULT_USER_ID = "00000000-0000-0000-0000-000000000001";

interface SpawnResult {
  project_id: string;
  job_id: string;
  status: string;
}

export default function HeroPrompt() {
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSend = async (message: string, _files?: File[]) => {
    setIsLoading(true);
    setError(null);

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
      window.location.href = `/project/${data.project_id}?job=${data.job_id}`;
    } catch (err) {
      setError(err instanceof Error ? err.message : "Unknown error");
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

      {error && (
        <div className="rounded-lg border border-red-700/30 bg-red-950/20 px-4 py-3 text-sm text-red-300">
          <p className="font-medium">Failed to spawn</p>
          <p className="mt-1 text-xs text-red-400/70">{error}</p>
        </div>
      )}
    </div>
  );
}
