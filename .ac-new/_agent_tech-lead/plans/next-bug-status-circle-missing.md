# Bug: Agent session shows no status circle when idle/waiting

## Problem

In the sidebar, a tech-lead agent session that is active but idle (waiting for a response from another agent, e.g. waiting for shipper to build) shows NO status circle at all. It looks like the session is dead/inactive.

Both tech-lead sessions were active, but only one showed a colored status circle. The other (which was waiting for shipper's build) had no circle.

## Expected

An active session should always show some status indicator — even when idle/waiting. The absence of any circle suggests the session is dead, which is misleading.

## To investigate

- What determines whether a status circle is shown?
- What state does a session have when the coding agent is idle/waiting?
- Is the idle detector marking it as "no status" instead of "idle"?
