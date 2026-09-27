import { Component } from "solid-js";

/**
 * #2568 - the amber stripe that tells the user a stored coding-agent reference
 * was not found and another profile was adopted. Shared by SessionItem and the
 * ProjectPanel replica rows so the two cannot drift. Presentational only: the
 * caller supplies the text and the dismiss action. stopPropagation is handled
 * here so the dismiss click never selects or switches the row; the stripe body
 * adds no handler, so a click on the text behaves like a click on the row.
 */
const OrphanNotice: Component<{
  text: string;
  onDismiss: () => void;
  testId?: string;
}> = (props) => {
  return (
    <div class="orphan-notice" data-ac-testid={props.testId}>
      <span class="orphan-notice-text">{props.text}</span>
      <button
        type="button"
        class="orphan-notice-dismiss"
        aria-label="Dismiss notice"
        title="Dismiss notice"
        data-ac-testid={props.testId ? `${props.testId}.dismiss` : undefined}
        onClick={(e) => {
          e.stopPropagation();
          props.onDismiss();
        }}
      >
        &#x2715;
      </button>
    </div>
  );
};

export default OrphanNotice;
