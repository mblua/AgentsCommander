// #4785 - the one sidebar regex filter control, rendered in the Temporary
// Rooms header. Markup moved from ProjectPanel's per-project filter row.
import { Show, type Component } from "solid-js";
import { focusOnMount } from "../../shared/focus-on-mount";
import { sidebarFilter } from "../stores/sidebar-filter";

const SidebarRegexFilter: Component = () => {
  let inputEl: HTMLInputElement | undefined;
  const focus = () => {
    if (inputEl) focusOnMount(inputEl, { select: true });
  };
  const toggle = () => {
    sidebarFilter.toggle();
    if (sidebarFilter.open()) focus();
  };
  const clear = () => {
    sidebarFilter.setPattern("");
    focus();
  };
  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    e.stopPropagation();
    if (sidebarFilter.escape() === "cleared") focus();
  };
  const toggleLabel = () =>
    sidebarFilter.open() ? "Hide regex filter" : "Filter sidebar (regex)";

  return (
    <div
      class="project-filter-row"
      classList={{
        open: sidebarFilter.open(),
        active: sidebarFilter.active(),
        invalid: !!sidebarFilter.error(),
      }}
      data-ac-testid="sidebar.regexFilter.row"
    >
      <div class="project-filter-field" classList={{ open: sidebarFilter.open() }}>
        <input
          ref={inputEl}
          class="project-filter-input"
          value={sidebarFilter.pattern()}
          placeholder="room-2.*"
          aria-label="Sidebar regex filter"
          aria-invalid={!!sidebarFilter.error()}
          data-ac-testid="sidebar.regexFilter.input"
          onInput={(e) => sidebarFilter.setPattern(e.currentTarget.value)}
          onKeyDown={handleKeyDown}
        />
        <Show when={sidebarFilter.pattern()}>
          <button
            type="button"
            class="project-filter-clear"
            title="Clear regex filter"
            aria-label="Clear regex filter"
            data-ac-testid="sidebar.regexFilter.clear"
            onClick={clear}
          >
            &#x2715;
          </button>
        </Show>
      </div>
      <button
        type="button"
        class="project-filter-toggle"
        title={toggleLabel()}
        aria-label={toggleLabel()}
        aria-expanded={sidebarFilter.open()}
        data-ac-testid="sidebar.regexFilter.toggle"
        onClick={toggle}
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <circle cx="11" cy="11" r="7" />
          <path d="m20 20-4.2-4.2" />
        </svg>
      </button>
    </div>
  );
};

export const SidebarRegexFilterError: Component = () => (
  <Show when={sidebarFilter.error()}>
    {(error) => (
      <div class="project-filter-error" role="alert" data-ac-testid="sidebar.regexFilter.error">
        {error()}
      </div>
    )}
  </Show>
);

export default SidebarRegexFilter;
