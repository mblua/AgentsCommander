
import { Component, createEffect, onMount, onCleanup, createSignal } from "solid-js";
import { specBoardStore, setSpecBoardStore } from "../stores/spec-board";

const MermaidPreview: Component = () => {
  let containerRef!: HTMLDivElement;
  let renderId = 0;
  const [mermaidAPI, setMermaidAPI] = createSignal<any>(null);

  onMount(async () => {
    try {
      const m = (await import("mermaid")).default;
      m.initialize({ startOnLoad: false, securityLevel: "strict" });
      setMermaidAPI(m);
    } catch (e) {
      console.error("Failed to load mermaid", e);
      setSpecBoardStore("renderError", "Failed to load Mermaid parser.");
    }
  });

  createEffect(() => {
    const source = specBoardStore.diagramSource;
    const m = mermaidAPI();
    if (!source || !containerRef || !m) return;
    
    const currentRenderId = ++renderId;
    
    m.render(`mermaid-${Date.now()}`, source)
      .then(({ svg }: any) => {
        if (currentRenderId === renderId) {
          containerRef.innerHTML = svg;
          setSpecBoardStore("renderError", null);
        }
      })
      .catch((e: any) => {
        if (currentRenderId === renderId) {
          setSpecBoardStore("renderError", e.message || "Parse error");
          // Keep old SVG visible
        }
      });
  });

  let isDragging = false;
  let startX = 0;
  let startY = 0;

  const onWheel = (e: WheelEvent) => {
    e.preventDefault();
    if (e.deltaY < 0) {
      setSpecBoardStore("previewZoom", z => z + 0.1);
    } else {
      setSpecBoardStore("previewZoom", z => Math.max(0.1, z - 0.1));
    }
  };

  const onPointerDown = (e: PointerEvent) => {
    if (e.button !== 0) return; // Only left click
    isDragging = true;
    startX = e.clientX - specBoardStore.previewOffset.x;
    startY = e.clientY - specBoardStore.previewOffset.y;
    (e.currentTarget as HTMLElement)?.setPointerCapture(e.pointerId);
  };

  const onPointerMove = (e: PointerEvent) => {
    if (!isDragging) return;
    setSpecBoardStore("previewOffset", {
      x: e.clientX - startX,
      y: e.clientY - startY
    });
  };

  const onPointerUp = (e: PointerEvent) => {
    isDragging = false;
    (e.currentTarget as HTMLElement)?.releasePointerCapture(e.pointerId);
  };

  return (
    <div 
      class="spec-board-preview"
      onWheel={onWheel}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
    >
      <div 
        class="spec-board-preview-inner"
        ref={containerRef}
        style={{
          transform: `translate(${specBoardStore.previewOffset.x}px, ${specBoardStore.previewOffset.y}px) scale(${specBoardStore.previewZoom})`
        }}
      />
    </div>
  );
};

export default MermaidPreview;
