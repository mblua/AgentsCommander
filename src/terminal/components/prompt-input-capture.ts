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

function skipControlSequence(data: string, index: number) {
  const next = data[index + 1];
  if (data[index] === "\x1b" && next === "[") {
    for (let i = index + 2; i < data.length; i += 1) {
      const code = data.charCodeAt(i);
      if (code >= 0x40 && code <= 0x7e) {
        return i + 1;
      }
    }
    return data.length;
  }

  if (data[index] === "\x9b") {
    for (let i = index + 1; i < data.length; i += 1) {
      const code = data.charCodeAt(i);
      if (code >= 0x40 && code <= 0x7e) {
        return i + 1;
      }
    }
    return data.length;
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
    if (char === "\x1b" || char === "\x9b") {
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
