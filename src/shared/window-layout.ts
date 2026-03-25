import { getCurrentWindow, currentMonitor } from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { LogicalPosition, LogicalSize } from "@tauri-apps/api/dpi";

const SIDEBAR_WIDTH_RATIO = 0.3;

export type LayoutSide = "left" | "right";

export async function applyWindowLayout(side: LayoutSide): Promise<void> {
  const sidebarWin = getCurrentWindow();
  const monitor = await currentMonitor();
  if (!monitor) return;

  const sf = monitor.scaleFactor;
  const { size: workSize, position: workPos } = monitor.workArea;
  const logicalW = workSize.width / sf;
  const logicalH = workSize.height / sf;
  const logicalX = workPos.x / sf;
  const logicalY = workPos.y / sf;

  const sidebarW = Math.round(logicalW * SIDEBAR_WIDTH_RATIO);
  const terminalW = logicalW - sidebarW;

  const sidebarX = side === "left" ? logicalX : logicalX + terminalW;
  const terminalX = side === "left" ? logicalX + sidebarW : logicalX;

  await sidebarWin.setSize(new LogicalSize(sidebarW, logicalH));
  await sidebarWin.setPosition(new LogicalPosition(sidebarX, logicalY));

  const terminalWin = await WebviewWindow.getByLabel("terminal");
  if (terminalWin) {
    await terminalWin.setSize(new LogicalSize(terminalW, logicalH));
    await terminalWin.setPosition(new LogicalPosition(terminalX, logicalY));
  }
}
