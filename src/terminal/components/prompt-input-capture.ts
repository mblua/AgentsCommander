export interface PromptCaptureResult {
  buffer: string;
  submittedPrompt: string | null;
}

const PASTE_START_7BIT = "\x1b[200~";
const PASTE_END_7BIT = "\x1b[201~";
const PASTE_START_8BIT = "\x9b200~";
const PASTE_END_8BIT = "\x9b201~";

function findPasteEnd(data: string, fromIndex: number) {
  const end7 = data.indexOf(PASTE_END_7BIT, fromIndex);
  const end8 = data.indexOf(PASTE_END_8BIT, fromIndex);

  if (end7 === -1) return end8 === -1 ? null : { index: end8, marker: PASTE_END_8BIT };
  if (end8 === -1) return { index: end7, marker: PASTE_END_7BIT };
  return end7 < end8
    ? { index: end7, marker: PASTE_END_7BIT }
    : { index: end8, marker: PASTE_END_8BIT };
}

// 8-bit C1 OSC introducer. OSC (7-bit `ESC ]`) is handled apart from the other
// string controls because it alone accepts BEL as a terminator (see below).
const OSC_INTRO_8BIT = "\x9d";

// The remaining ECMA-48 "string" controls, which terminate ONLY at ST: DCS, SOS,
// PM, APC. 7-bit finals after ESC: P (DCS), X (SOS), ^ (PM), _ (APC).
const STRING_SEQ_INTRO_7BIT = new Set(["P", "X", "^", "_"]);
// 8-bit C1 equivalents: \x90 (DCS), \x98 (SOS), \x9e (PM), \x9f (APC).
const STRING_SEQ_INTRO_8BIT = new Set(["\x90", "\x98", "\x9e", "\x9f"]);

// Bytes that begin a control sequence we must skip rather than capture as prompt
// text: ESC (every 7-bit sequence), 8-bit CSI (\x9b, also the bracketed-paste
// prefix handled before this check), and the 8-bit string introducers (OSC + the
// ST-only family).
function isControlIntroducer(char: string): boolean {
  return (
    char === "\x1b" ||
    char === "\x9b" ||
    char === OSC_INTRO_8BIT ||
    STRING_SEQ_INTRO_8BIT.has(char)
  );
}

// CSI: consume up to and including the final byte (0x40–0x7e).
function skipCsi(data: string, bodyStart: number): number {
  for (let i = bodyStart; i < data.length; i += 1) {
    const code = data.charCodeAt(i);
    if (code >= 0x40 && code <= 0x7e) {
      return i + 1;
    }
  }
  return data.length;
}

// OSC/DCS/SOS/PM/APC: consume the whole "string" sequence up to its terminator.
// All end at ST (`ESC \` or 8-bit \x9c); OSC additionally accepts BEL (\x07) —
// the xterm convention OpenCode uses for its OSC-4 colour-palette reply (#533).
// For DCS/SOS/PM/APC, `belTerminates` is false, so a stray BEL inside the payload
// is consumed rather than mistaken for a terminator.
function skipStringSequence(
  data: string,
  bodyStart: number,
  belTerminates: boolean,
): number {
  for (let i = bodyStart; i < data.length; i += 1) {
    const code = data.charCodeAt(i);
    if (code === 0x9c || (belTerminates && code === 0x07)) {
      return i + 1; // 8-bit ST, or BEL for OSC
    }
    if (code === 0x1b && data[i + 1] === "\\") {
      return i + 2; // 7-bit ST: ESC \
    }
  }
  return data.length;
}

function skipControlSequence(data: string, index: number): number {
  const char = data[index];

  if (char === "\x1b") {
    const next = data[index + 1];
    if (next === "[") {
      return skipCsi(data, index + 2); // CSI: ESC [
    }
    if (next === "]") {
      return skipStringSequence(data, index + 2, true); // OSC: ST or BEL
    }
    if (next !== undefined && STRING_SEQ_INTRO_7BIT.has(next)) {
      return skipStringSequence(data, index + 2, false); // DCS/SOS/PM/APC: ST only
    }
    // Lone trailing ESC or a short escape this helper does not model — drop just
    // the ESC, preserving prior behaviour for those cases.
    return index + 1;
  }

  if (char === "\x9b") {
    return skipCsi(data, index + 1); // 8-bit CSI
  }

  if (char === OSC_INTRO_8BIT) {
    return skipStringSequence(data, index + 1, true); // 8-bit OSC: ST or BEL
  }

  if (STRING_SEQ_INTRO_8BIT.has(char)) {
    return skipStringSequence(data, index + 1, false); // 8-bit DCS/SOS/PM/APC: ST only
  }

  return index + 1;
}

function promptTextFromInputChunk(data: string) {
  let text = "";
  let index = 0;

  while (index < data.length) {
    if (data.startsWith(PASTE_START_7BIT, index) || data.startsWith(PASTE_START_8BIT, index)) {
      const startMarker = data.startsWith(PASTE_START_7BIT, index)
        ? PASTE_START_7BIT
        : PASTE_START_8BIT;
      const payloadStart = index + startMarker.length;
      const end = findPasteEnd(data, payloadStart);

      if (!end) {
        text += data.slice(payloadStart);
        break;
      }

      text += data.slice(payloadStart, end.index);
      index = end.index + end.marker.length;
      continue;
    }

    const char = data[index];
    if (isControlIntroducer(char)) {
      index = skipControlSequence(data, index);
      continue;
    }

    if (char >= " ") {
      text += char;
    }
    index += 1;
  }

  return text;
}

export function updatePromptCapture(buffer: string, data: string): PromptCaptureResult {
  if (data === "\r") {
    const submittedPrompt = buffer.trim();
    return {
      buffer: "",
      submittedPrompt: submittedPrompt || null,
    };
  }

  if (data === "\x7f") {
    return {
      buffer: buffer.slice(0, -1),
      submittedPrompt: null,
    };
  }

  return {
    buffer: buffer + promptTextFromInputChunk(data),
    submittedPrompt: null,
  };
}
