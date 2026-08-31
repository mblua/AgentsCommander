# Phase 2: Frontend UI, notifications, IPC integration, and universal replica indicator rendering

Objective: Implement frontend types, IPC invocation, toast action button rendering, sidebar communication slot indicator rendered across all replica rows (coordinators and workers), and automatic / manual resolution integration.
Class: patterned
Owner: ac-dev-webpage-ui-v4

## 1. Exact Files and Symbols Modified

1. `src/shared/types.ts`:
   - Update `SessionCommunicationKind` union: `export type SessionCommunicationKind = "raiseHand" | "blockedMenu";`
   - Update `SessionCommunication` interface: add `message?: string | null;`

2. `src/shared/ipc.ts`:
   - Add and export function:
     ```ts
     export function resolveBlockingMenu(sessionId: string): Promise<void> {
       return transport.invoke("resolve_blocking_menu", { id: sessionId });
     }
     ```

3. `src/shared/stores/toasts.ts`:
   - Export interface `ToastAction`:
     ```ts
     export interface ToastAction {
       label: string;
       onClick: () => void;
     }
     ```
   - Update `PushToastOptions` interface: add `action?: ToastAction; tag?: string;`
   - Update `Toast` interface: add `action?: ToastAction; tag?: string;`
   - In `toastStore.push(opts)`: if `opts.tag` is specified and an existing toast in `toasts` has `toast.tag === opts.tag`, update its `message` and `action` in place without creating a duplicate or resetting exit animation.
   - Add method `dismissByTag(tag: string): void` to `toastStore` that finds and starts exit animation on any toast matching the tag.

4. `src/shared/components/ToastHost.tsx`:
   - In the `.toast-item` markup, immediately before the dismiss button, add:
     ```tsx
     <Show when={toast.action}>
       {(action) => (
         <button
           class="toast-item__action"
           type="button"
           data-ac-testid="toast.item.action"
           onClick={() => {
             action().onClick();
             toastStore.dismiss(toast.id);
           }}
         >
           {action().label}
         </button>
       )}
     </Show>
     ```

5. `src/sidebar/App.tsx`:
   - In `onSessionCommunicationChanged(({ sessionId, communication }) => { ... })`:
     - Update sessions store: `sessionsStore.setCommunication(sessionId, communication);`
     - If `communication?.kind === "blockedMenu" && communication.visible === true && communication.message`:
       ```ts
       toastStore.push({
         message: communication.message,
         kind: "info",
         durationMs: null,
         tag: `blockedMenu:${sessionId}`,
         action: {
           label: "Resolved by user",
           onClick: () => {
             void resolveBlockingMenu(sessionId);
           },
         },
       });
       ```
     - Else:
       ```ts
       toastStore.dismissByTag(`blockedMenu:${sessionId}`);
       ```

6. `src/sidebar/components/workgroup-session.ts`:
   - Add helper function:
     ```ts
     export function replicaHasBlockedMenu(wg: AcWorkgroup, replica: AcAgentReplica): boolean {
       const communication = findReplicaSession(wg, replica)?.communication;
       return communication?.kind === "blockedMenu" && communication?.visible === true;
     }
     ```
   - Add helper function:
     ```ts
     export function workgroupHasBlockedMenu(wg: AcWorkgroup): boolean {
       return wg.agents.some((replica) => replicaHasBlockedMenu(wg, replica));
     }
     ```

7. `src/sidebar/components/ProjectPanel.tsx`:
   - Define `showBlockedMenu`:
     ```ts
     const showBlockedMenu = createMemo(() =>
       communication()?.kind === "blockedMenu" && communication()?.visible === true
     );
     ```
   - In the replica row JSX within `replica-item-info`, render the blocked-menu indicator adjacent to `replica-item-name` (NOT inside `<Show when={taskTitle}>`), ensuring it displays for ALL agent replica rows (coordinators and worker replicas alike):
     ```tsx
     <div class="replica-item-name-row">
       <span class="replica-item-name">{replica.originProject ? `${replica.name}@${replica.originProject}` : replica.name}</span>
       <Show when={showBlockedMenu()}>
         <span
           class="coord-communication-slot coord-communication-slot--blocked-menu"
           data-kind="blockedMenu"
           data-ac-testid={communicationSlotTestId()}
           title={communication()?.message ?? "Interactive menu requires user input"}
           aria-label="Interactive menu requires user input"
         >
           <RaiseHandIcon class="coord-communication-icon" />
         </span>
       </Show>
     </div>
     ```

## 2. Inlined Decisions and Behavior

- **Universal Replica Row Indicator**: Blocked-menu indicator renders for ALL agent replicas in the sidebar when `communication?.kind === "blockedMenu" && communication?.visible === true`. It is explicitly placed outside `<Show when={taskTitle}>` so worker replicas (e.g. `dev-rust`, `dev-python`, `architect`) and coordinators without task titles display the visual indicator.
- **Sticky Notification with Action**: A blocked menu displays an informative sticky toast carrying the agent's specific notification text and a `"Resolved by user"` button.
- **Tag-Based Deduplication and Auto-Dismissal**: Toasts for blocked menus use `tag: "blockedMenu:${sessionId}"`. Pushing multiple events for the same session does not spawn duplicate toasts, and when the block clears in the backend, `dismissByTag` removes the toast automatically.
- **Manual Resolution IPC**: Clicking `"Resolved by user"` immediately invokes `resolveBlockingMenu(sessionId)` through IPC and dismisses the toast locally.

## 3. Required Tests and Verification

1. Unit tests in `src/shared/components/ToastHost.test.tsx`:
   - Test rendering toast with action button, clicking action button fires `onClick` and dismisses toast.
   - Test `dismissByTag` removes toast matching tag.
2. Workflow tests in `src/sidebar/App.menu-guard.workflow.test.tsx` (new test file):
   - Test emitting backend event `session_communication_changed` with `kind: "blockedMenu"` creates a toast with `"Resolved by user"` button.
   - Test clicking `"Resolved by user"` calls `resolveBlockingMenu` IPC command with correct `sessionId`.
   - Test emitting subsequent `session_communication_changed` with `communication: null` auto-dismisses the toast.
3. Component tests in `src/sidebar/components/ProjectPanel.menu-guard.test.tsx` (new test file):
   - Test coordinator replica item renders communication slot with `data-kind="blockedMenu"` when `communication` is `BlockedMenu`.
   - Test worker replica item (non-coordinator, without `taskTitle`) renders communication slot with `data-kind="blockedMenu"` when `communication` is `BlockedMenu`.
   - Test tooltip and aria-label reflect `communication.message`.

Verification command:
```bash
npm test
npm run build
```

## 4. Objective Acceptance Criteria

1. Running `npm test` and `npm run build` pass with 0 errors.
2. A backend `session_communication_changed` event with `kind: "blockedMenu"` displays a sticky notification in the UI with a `"Resolved by user"` button.
3. Clicking `"Resolved by user"` triggers the `resolve_blocking_menu` IPC invocation.
4. Clearing the communication in the backend automatically dismisses the notification.
5. The sidebar replica row shows the communication slot indicator with `data-kind="blockedMenu"` on BOTH coordinator and non-coordinator worker replica rows.

## 5. Preserve List

- Preserve existing `raiseHand` communication handling and tests in `App.raise-hand.workflow.test.tsx` and `ProjectPanel.raise-hand.test.tsx`.
- Preserve existing toast behavior (eviction policy, duration defaults, dismiss button).
- Preserve existing sidebar layout and styling.

