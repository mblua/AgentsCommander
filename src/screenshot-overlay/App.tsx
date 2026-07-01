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

// #714 Selection must be at least this many physical px on each side to count as
// a real drag; a smaller release is treated as a mis-click and cancels locally
// (the backend also rejects width/height < 2). Mirrors the plan's 2px floor.
const MIN_SELECTION_PX = 2;

/**
 * #714 Per-monitor screenshot selection overlay. One instance renders in each
 * `?window=screenshot-overlay&captureId=<dashed-uuid>&monitorId=<n>` window. It
 * draws the frozen monitor image the backend captured, lets the user rubber-band
 * a rectangle with a pixel magnifier, and sends the physical crop back. The Rust
 * side owns ALL lifecycle/cleanup (confirm/cancel/WindowEvent::Destroyed); the
 * frontend cancel-on-unmount is best-effort only.
 */
const ScreenshotOverlayApp: Component = () => {
  let canvas: HTMLCanvasElement | undefined;

  onMount(() => {
    const params = new URLSearchParams(window.location.search);
    const captureId = params.get("captureId") ?? "";
    const monitorId = Number(params.get("monitorId"));

    // Mutable interaction state (plain locals, not signals: drawing is fully
    // imperative and runs off pointer events + rAF, so reactivity would only add
    // overhead).
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
      // Fire-and-forget: the backend destroys every overlay window; a stale id is
      // a harmless no-op there.
      void ScreenshotAPI.cancel(captureId).catch((err) => {
        console.error(`[screenshot-overlay] cancel (${reason}) failed:`, err);
      });
    };

    // Grab focus so the ESC keydown reaches THIS overlay even though the hotkey
    // fired while another app was focused. Best-effort (jsdom/OS may no-op).
    const focusOverlay = (): void => {
      try {
        window.focus();
      } catch {
        /* focus is best-effort */
      }
    };

    const scheduleDraw = (): void => {
      if (rafId !== 0) return;
      rafId = requestAnimationFrame(() => {
        rafId = 0;
        draw();
      });
    };

    // Physical-px-per-CSS-px ratio, so on-screen sizes (border, magnifier) stay
    // constant regardless of monitor DPI (the canvas backing store is physical
    // px, displayed at the monitor's logical size).
    const physicalPerCss = (): number => {
      if (!canvas || !overlay) return 1;
      const rect = canvas.getBoundingClientRect();
      return rect.width > 0 ? overlay.width / rect.width : 1;
    };

    const draw = (): void => {
      if (!ctx || !overlay || !image) return;
      const { width, height } = overlay;
      // The drag endpoint IS the hover point while dragging (both are set from
      // the same pointer move), so `hover` doubles as the live drag corner.
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
        /* pointer capture is best-effort */
      }
      scheduleDraw();
    };

    const onPointerMove = (e: PointerEvent): void => {
      if (settled || !overlay) return;
      hover = toImagePoint(e);
      scheduleDraw();
    };

    // Focus on ENTER (not on every move) so ESC reaches the overlay the cursor is
    // over, without an OS window call on each pointermove during a drag.
    const onPointerEnter = (): void => {
      if (!settled) focusOverlay();
    };

    const onPointerUp = (e: PointerEvent): void => {
      if (settled || !overlay || !dragStart) return;
      const end = toImagePoint(e);
      try {
        if (activePointerId !== null) canvas?.releasePointerCapture(activePointerId);
      } catch {
        /* best-effort */
      }
      activePointerId = null;

      const selection: Rect = clampSelectionToBounds(
        normalizeSelection(dragStart, end),
        overlay.width,
        overlay.height
      );

      if (selection.width < MIN_SELECTION_PX || selection.height < MIN_SELECTION_PX) {
        // Treat a tiny drag as a mis-click: reset and keep the overlay open.
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
        // Backend owns teardown + a failure event; just log locally.
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

    // Register teardown synchronously so it binds to this owner even though the
    // async setup below resolves later.
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
      // Best-effort only — Rust's WindowEvent::Destroyed is the authority.
      cancelOnce("unmount");
    });

    void (async () => {
      let state: ScreenshotOverlayState;
      try {
        state = await ScreenshotAPI.getOverlayState(captureId, monitorId);
      } catch (err) {
        // Stale/starting capture: nothing to render. Rust cleans up on
        // cancel/destroy, so just leave the (transparent) window.
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
        // decode() can reject in non-browser test envs (or for odd encodings);
        // fall through and draw best-effort so interaction still works.
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
