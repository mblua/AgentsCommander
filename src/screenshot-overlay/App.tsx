import { Component, onCleanup, onMount } from "solid-js";
import { ScreenshotAPI } from "../shared/ipc";
import type { ScreenshotOverlayState } from "../shared/types";
import {
  clampSelectionToBounds,
  clientToImagePoint,
  normalizeSelection,
  type Point,
  type Rect,
} from "./selection";
import { drawOverlay } from "./render";
import "./styles/screenshot-overlay.css";

const MIN_SELECTION_PX = 2;

const ScreenshotOverlayApp: Component = () => {
  let canvas: HTMLCanvasElement | undefined;

  onMount(() => {
    const params = new URLSearchParams(window.location.search);
    const captureId = params.get("captureId") ?? "";
    const monitorId = Number(params.get("monitorId"));

    let overlay: ScreenshotOverlayState | null = null;
    let image: HTMLImageElement | null = null;
    let ctx: CanvasRenderingContext2D | null = null;
    let dragStart: Point | null = null;
    let hover: Point | null = null;
    let settled = false; // confirm resolved OR cancel already fired.
    let rafId = 0;
    let disposed = false;
    let activePointerId: number | null = null;

    const cancelOnce = (reason: string): void => {
      if (settled) return;
      settled = true;
      void ScreenshotAPI.cancel(captureId).catch((err) => {
        console.error(`[screenshot-overlay] cancel (${reason}) failed:`, err);
      });
    };

    const focusOverlay = (): void => {
      try {
        window.focus();
      } catch {
      }
    };

    const scheduleDraw = (): void => {
      if (rafId !== 0) return;
      rafId = requestAnimationFrame(() => {
        rafId = 0;
        draw();
      });
    };

    const physicalPerCss = (): number => {
      if (!canvas || !overlay) return 1;
      const rect = canvas.getBoundingClientRect();
      return rect.width > 0 ? overlay.width / rect.width : 1;
    };

    const draw = (): void => {
      if (!ctx || !overlay || !image) return;
      const { width, height } = overlay;
      const selection =
        dragStart && hover
          ? clampSelectionToBounds(
              normalizeSelection(dragStart, hover),
              width,
              height
            )
          : null;
      drawOverlay(
        ctx,
        image,
        { width, height, physicalPerCss: physicalPerCss() },
        selection,
        hover
      );
    };

    const toImagePoint = (e: PointerEvent): Point => {
      if (!canvas || !overlay) return { x: 0, y: 0 };
      const rect = canvas.getBoundingClientRect();
      return clientToImagePoint(
        { x: e.clientX, y: e.clientY },
        rect,
        overlay.width,
        overlay.height
      );
    };

    const onPointerDown = (e: PointerEvent): void => {
      if (settled || !overlay) return;
      focusOverlay();
      dragStart = toImagePoint(e);
      hover = dragStart;
      activePointerId = e.pointerId;
      try {
        canvas?.setPointerCapture(e.pointerId);
      } catch {
      }
      scheduleDraw();
    };

    const onPointerMove = (e: PointerEvent): void => {
      if (settled || !overlay) return;
      hover = toImagePoint(e);
      scheduleDraw();
    };

    const onPointerEnter = (): void => {
      if (!settled) focusOverlay();
    };

    const onPointerUp = (e: PointerEvent): void => {
      if (settled || !overlay || !dragStart) return;
      const end = toImagePoint(e);
      try {
        if (activePointerId !== null) canvas?.releasePointerCapture(activePointerId);
      } catch {
      }
      activePointerId = null;

      const selection: Rect = clampSelectionToBounds(
        normalizeSelection(dragStart, end),
        overlay.width,
        overlay.height
      );

      if (selection.width < MIN_SELECTION_PX || selection.height < MIN_SELECTION_PX) {
        dragStart = null;
        scheduleDraw();
        return;
      }

      settled = true;
      void ScreenshotAPI.confirmSelection({
        captureId,
        monitorId,
        x: selection.x,
        y: selection.y,
        width: selection.width,
        height: selection.height,
      }).catch((err) => {
        console.error("[screenshot-overlay] confirmSelection failed:", err);
      });
    };

    const onPointerLeave = (): void => {
      if (dragStart) return; // keep the magnifier while actively dragging
      hover = null;
      scheduleDraw();
    };

    const onKeyDown = (e: KeyboardEvent): void => {
      if (e.key === "Escape") {
        e.preventDefault();
        cancelOnce("escape");
      }
    };

    onCleanup(() => {
      disposed = true;
      if (rafId !== 0) cancelAnimationFrame(rafId);
      window.removeEventListener("keydown", onKeyDown);
      if (canvas) {
        canvas.removeEventListener("pointerdown", onPointerDown);
        canvas.removeEventListener("pointermove", onPointerMove);
        canvas.removeEventListener("pointerup", onPointerUp);
        canvas.removeEventListener("pointerenter", onPointerEnter);
        canvas.removeEventListener("pointerleave", onPointerLeave);
      }
      cancelOnce("unmount");
    });

    void (async () => {
      let state: ScreenshotOverlayState;
      try {
        state = await ScreenshotAPI.getOverlayState(captureId, monitorId);
      } catch (err) {
        console.error("[screenshot-overlay] getOverlayState failed:", err);
        return;
      }
      if (disposed || !canvas) return;
      overlay = state;

      const img = new Image();
      img.src = state.imageDataUrl;
      try {
        await img.decode();
      } catch {
      }
      if (disposed || !canvas) return;
      image = img;

      canvas.width = state.width;
      canvas.height = state.height;
      ctx = canvas.getContext("2d");

      canvas.addEventListener("pointerdown", onPointerDown);
      canvas.addEventListener("pointermove", onPointerMove);
      canvas.addEventListener("pointerup", onPointerUp);
      canvas.addEventListener("pointerenter", onPointerEnter);
      canvas.addEventListener("pointerleave", onPointerLeave);
      window.addEventListener("keydown", onKeyDown);
      focusOverlay();
      draw();
    })();
  });

  return (
    <div class="screenshot-overlay" data-ac-testid="screenshotOverlay.root">
      <canvas ref={canvas} data-ac-testid="screenshotOverlay.canvas" />
    </div>
  );
};

export default ScreenshotOverlayApp;
