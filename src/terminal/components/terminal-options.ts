import type { ITerminalOptions } from "@xterm/xterm";
import { requestExternalLinkConfirmation } from "../../shared/external-links";

export const createTerminalOptions = (useExternalLinkConfirmation: boolean): ITerminalOptions => ({
  fontFamily: "'Cascadia Code', 'JetBrains Mono', 'Fira Code', monospace",
  fontSize: 14,
  lineHeight: 1.2,
  cursorBlink: true,
  cursorStyle: "block",
  scrollback: 10000,
  theme: {
    background: "#0a0a0f",
    foreground: "#e8e8e8",
    cursor: "#00d4ff",
    selectionBackground: "rgba(0, 212, 255, 0.25)",
    black: "#1a1a2e",
    red: "#ff3b5c",
    green: "#33ff99",
    yellow: "#ffcc33",
    blue: "#3399ff",
    magenta: "#ff33cc",
    cyan: "#33ccff",
    white: "#e8e8e8",
    brightBlack: "#4a4a5e",
    brightRed: "#ff6699",
    brightGreen: "#66ffbb",
    brightYellow: "#ffdd66",
    brightBlue: "#66bbff",
    brightMagenta: "#ff66dd",
    brightCyan: "#66ddff",
    brightWhite: "#ffffff",
    // #2579: slider matches dark --sidebar-border (sidebar/styles/variables.css:9);
    // xterm parses colors, not var().
    scrollbarSliderBackground: "rgba(255, 255, 255, 0.06)",
    scrollbarSliderHoverBackground: "rgba(255, 255, 255, 0.12)",
    scrollbarSliderActiveBackground: "rgba(255, 255, 255, 0.18)",
  },
  allowTransparency: false,
  linkHandler: useExternalLinkConfirmation
    ? {
        allowNonHttpProtocols: false,
        activate(event, text) {
          event.preventDefault();
          event.stopPropagation();
          event.stopImmediatePropagation();
          requestExternalLinkConfirmation(text);
        },
      }
    : undefined,
});
