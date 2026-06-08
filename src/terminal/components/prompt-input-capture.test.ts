import { describe, expect, it } from "vitest";
import { updatePromptCapture } from "./prompt-input-capture";

function applyChunks(chunks: string[]) {
  let buffer = "";
  let submittedPrompt: string | null = null;

  for (const chunk of chunks) {
    const result = updatePromptCapture(buffer, chunk);
    buffer = result.buffer;
    submittedPrompt = result.submittedPrompt;
  }

  return { buffer, submittedPrompt };
}

describe("updatePromptCapture", () => {
  it("preserves typed text around a bracketed-paste Windows path", () => {
    const path = "C:\\Users\\maria\\0_mmb\\0_AC\\agentscommander_standalone_wg-1_project";

    expect(applyChunks([" 1- ", `\x1b[200~${path}\x1b[201~`, " 2- ok", "\r"])).toEqual({
      buffer: "",
      submittedPrompt: `1- ${path} 2- ok`,
    });
  });

  it("preserves a whole prompt pasted as one bracketed-paste chunk", () => {
    const prompt = "1- C:\\Users\\maria\\project 2- ok";

    expect(applyChunks([`\x1b[200~${prompt}\x1b[201~`, "\r"])).toEqual({
      buffer: "",
      submittedPrompt: prompt,
    });
  });

  it("ignores arrow and control escape sequences", () => {
    expect(applyChunks(["abc", "\x1b[A", "\x1b[B", "\x1b[1;5D", "\x9bC", "d", "\r"])).toEqual({
      buffer: "",
      submittedPrompt: "abcd",
    });
  });

  it("keeps single-character typing, backspace, and enter behavior intact", () => {
    expect(applyChunks(["a", "b", "c", "\x7f", "d", "\r"])).toEqual({
      buffer: "",
      submittedPrompt: "abd",
    });
  });
});
