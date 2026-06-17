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

  it("strips a BEL-terminated OSC-4 colour reply that precedes typed text (#533)", () => {
    // OpenCode answers an OSC-4 palette query on the input channel; previously
    // only the leading ESC was dropped and "]4;0;rgb:..." leaked into the prompt.
    const oscReply = "\x1b]4;0;rgb:1a1a/1a1a/2e2e\x07";
    expect(
      applyChunks([`${oscReply}Mandale un saludo a tu team`, "\r"]),
    ).toEqual({
      buffer: "",
      submittedPrompt: "Mandale un saludo a tu team",
    });
  });

  it("strips an OSC sequence terminated by ST (ESC \\) between typed text", () => {
    const osc = "\x1b]0;window title\x1b\\";
    expect(applyChunks([`before ${osc}after`, "\r"])).toEqual({
      buffer: "",
      submittedPrompt: "before after",
    });
  });

  it("strips a DCS sequence terminated by ST", () => {
    const dcs = "\x1bP1;2|payload\x1b\\";
    expect(applyChunks([`x${dcs}y`, "\r"])).toEqual({
      buffer: "",
      submittedPrompt: "xy",
    });
  });

  it("does not let a BEL inside a DCS payload terminate it early (DCS ends only at ST)", () => {
    // Only OSC accepts BEL; a literal BEL in a DCS payload must be consumed, not
    // treated as a terminator, or the tail would leak into the captured prompt.
    const dcs = "\x1bPdata\x07more\x1b\\";
    expect(applyChunks([`x${dcs}y`, "\r"])).toEqual({
      buffer: "",
      submittedPrompt: "xy",
    });
  });

  it("strips an 8-bit OSC introducer (\\x9d) terminated by 8-bit ST (\\x9c)", () => {
    expect(applyChunks(["a\x9d4;0;rgb:1/2/3\x9cb", "\r"])).toEqual({
      buffer: "",
      submittedPrompt: "ab",
    });
  });

  it("keeps literal brackets that are not part of an OSC sequence", () => {
    // A bare "]" (not preceded by ESC) is ordinary prompt text, not a control byte.
    expect(applyChunks(["arr[0] = list[ ]", "\r"])).toEqual({
      buffer: "",
      submittedPrompt: "arr[0] = list[ ]",
    });
  });
});
