import { Component, createMemo, createSignal, onCleanup } from "solid-js";
import {
  ZOOM_MAX,
  ZOOM_MIN,
  ZOOM_STEP,
  getZoomState,
  onZoomChange,
  setUiZoom,
} from "../../shared/zoom";

const ZoomStepper: Component = () => {
  const [zoom, setZoom] = createSignal(getZoomState().zoom);
  const unsubscribe = onZoomChange((state) => setZoom(state.zoom));
  onCleanup(unsubscribe);

  const percent = createMemo(() => `${Math.round(zoom() * 100)}%`);
  const canZoomOut = createMemo(() => zoom() > ZOOM_MIN);
  const canZoomIn = createMemo(() => zoom() < ZOOM_MAX);

  const changeZoom = (delta: number) => {
    void setUiZoom(zoom() + delta).catch((err) => {
      console.error("Failed to change zoom:", err);
    });
  };

  return (
    <div class="titlebar-zoom-stepper" role="group" aria-label="UI zoom" data-ac-testid="titlebar.zoom">
      <button
        class="titlebar-zoom-btn"
        type="button"
        title="Zoom out"
        aria-label="Zoom out"
        disabled={!canZoomOut()}
        onClick={() => changeZoom(-ZOOM_STEP)}
        data-ac-testid="titlebar.zoom.out"
      >
        -
      </button>
      <span class="titlebar-zoom-value" aria-live="polite" data-ac-testid="titlebar.zoom.value">
        {percent()}
      </span>
      <button
        class="titlebar-zoom-btn"
        type="button"
        title="Zoom in"
        aria-label="Zoom in"
        disabled={!canZoomIn()}
        onClick={() => changeZoom(ZOOM_STEP)}
        data-ac-testid="titlebar.zoom.in"
      >
        +
      </button>
    </div>
  );
};

export default ZoomStepper;
