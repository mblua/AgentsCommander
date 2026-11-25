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
    <fieldset
      class="titlebar-zoom-stepper"
      aria-label="UI zoom"
      data-ac-testid="titlebar.zoom"
      data-ac-role="group"
      style={{ border: "0", padding: "0", "min-width": "0" }}
    >
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
      <span
        class="titlebar-zoom-value"
        aria-live="polite"
        data-ac-testid="titlebar.zoom.value"
        data-ac-role="status"
      >
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
    </fieldset>
  );
};

export default ZoomStepper;
