# Notifications and dialogs

For developers who just saw a message from AgentsCommander and want to know what it means before answering it. After this page you can match any toast, banner or modal on screen to what raised it and what each button does.

AC interrupts you in three ways: a toast in the corner, a banner inside the sidebar, and a modal that blocks until you answer. This page covers all of them, in the order you are likely to meet them.

## Toasts

A toast is a short message in the corner. Every toast has a kind, and the kind decides how long it stays:

| Kind | Lifetime |
|---|---|
| `error` | **Sticky.** It stays until you dismiss it. |
| `success` | Auto-dismisses after 4 seconds. |
| `info` | Auto-dismisses after 4 seconds. This is the default kind. |

Any toast can be dismissed by hand with the `×` button, labelled `Dismiss notification` for assistive technology. A caller can also override a toast's lifetime, including making a non-error toast sticky.

At most **four** toasts are visible at once. Past that, AC evicts the oldest one that is **not** an error, and falls back to the oldest overall only when every visible toast is an error. That rule exists so a transient info toast can never quietly bury an error you have not read.

Clicking a toast's body does not raise or focus the terminal behind it, so dismissing one never steals your place.

## The error modal

The error modal is titled `Application Error`. It appears when AC has an application-level failure to report that is too important for a toast.

It shows the error's detail in a region labelled `Error detail`, and when more than one error is queued it also shows a counter, reading for example `1 of 3`, so you know how many are waiting behind the one on screen.

`Dismiss` closes the current error and moves to the next queued one, if any.

## Quitting the app

Closing AC while detached terminal windows are open asks first. The dialog is titled `Quit AgentsCommander?` and tells you what is open:

> You have `<n>` detached session`(s)` open. Quit the app and close all detached sessions?

Two buttons: `Cancel` and `Quit`. `Cancel` has focus when the dialog opens, so a reflexive `Enter` cancels rather than quits. `Escape` cancels. `Enter` quits **only** when `Quit` already has focus.

## Opening an external link

A link that leads outside the app is confirmed before it opens. The dialog says:

> This link opens outside Agents Commander.

`Open anyway` proceeds and hands the URL to your browser. Declining leaves you where you were and opens nothing.

The confirmation is hosted by more than one window, including the Guide, so you get the same prompt wherever the link was.

## Onboarding

The first-run wizard is titled `Welcome`. It opens when AC has no configuration to work from, and walks you through adding a coding agent so the app is usable when it closes.

`onboardingDismissed` in `settings.json` records whether the first-run wizard was dismissed, and it defaults to `false`.

**One caveat before you build automation on that flag.** Exactly what sets it, and whether it means "onboarding completed" or only "the user cancelled onboarding", is an open product question tracked as issue #505 in the repository's own QA notes. This page does not state which reading is correct, because source did not settle it.

## Restart prompt

Some changes to a session's configuration only apply to a fresh process. When you make one, AC asks:

> Restart the session now to apply it?

Accepting restarts that session. Declining leaves it running with the previous configuration until you restart it yourself.

## Root Agent banner

The banner sits at the top of the sidebar and belongs to the **Root Agent** session: the single host-level agent described in the [glossary](../glossary.md#root-agent).

With no Root Agent session running, the banner's action creates one. With one running, the banner is that session's compact control strip: its status dot, its context badge when the agent has a context pattern configured, and controls to open its folder in the file explorer (`Open folder in explorer`), close it (`Close session`), restart it (`Restart Session`), pick its coding agent (`Coding Agent`), and cancel a voice recording in progress (`Cancel recording`).

## Context-template updates

When AC ships a newer default for a project context template you have edited, it asks rather than overwriting. The modal is titled `Context template update` and offers the choice in as many words: keep your version, or overwrite it with the new default.

Two buttons: `Keep my version` leaves your file untouched, and `Overwrite with default` replaces it with the version this build ships.

See [Agent Matrix conventions](../agent-matrix-conventions.md) for what project context templates are and where they live.

## Sounds

Two settings govern every sound AC makes.

`soundsEnabled` is the master switch for all app-emitted sounds, and it is `true` by default. Turn it off and AC makes no sound at all.

`teamIdleBeepEnabled`, also `true` by default, beeps when a team transitions from busy to all-idle. It is gated by `soundsEnabled`: with the master switch off, this one changes nothing.

## Settings

| Key | What it controls |
|---|---|
| `soundsEnabled` | Master switch for all app-emitted sounds. `true` by default. |
| `teamIdleBeepEnabled` | Beep when a team transitions from busy to all-idle. `true` by default, gated by `soundsEnabled`. |

See [Settings reference](../reference/settings.md#window--ui) for both keys in context.

## Troubleshooting

**"An error toast will not go away on its own."** That is deliberate: error toasts are sticky and wait for you. Dismiss it with the `×`.

**"A toast disappeared before I read it."** Four toasts are visible at once and a fifth evicts the oldest non-error. Errors are protected from that eviction, so what vanished was an info or success message.

**"The idle beep does not fire for the workgroup I am watching."** Expected. AC suppresses the beep for the workgroup whose session currently has your focus, and briefly after you move focus away from one. A workgroup you are not looking at still beeps.

**"No sound at all, and `teamIdleBeepEnabled` is `true`."** Check `soundsEnabled`. It is the master switch, and the beep is gated by it.

**"`Enter` quit the app when I meant to cancel."** It should not have: `Cancel` holds focus when the quit dialog opens, and `Enter` quits only when `Quit` has focus. If it did quit, the focus had moved to `Quit` first.

**"Onboarding came back after I completed it."** See the caveat in [Onboarding](#onboarding) above: what persists the dismissal is an open question tracked as issue #505, and this page does not assert either behavior.

## See also

- [App windows](app-windows.md) - the windows these dialogs appear in
- [Settings reference](../reference/settings.md#window--ui) - `soundsEnabled` and `teamIdleBeepEnabled`
- [Agent Matrix conventions](../agent-matrix-conventions.md) - the context templates the update modal offers to replace
