import { PromptInputBox } from "./ui/ai-prompt-box";

export default function HeroPrompt() {
  const handleSend = (message: string, files?: File[]) => {
    console.log("User prompt:", message, files);
    // TODO: wire to CreateProject + SendMessage API
  };

  return (
    <PromptInputBox
      placeholder="Ask Stem Cell to create a landing page for my..."
      onSend={handleSend}
    />
  );
}
