# Co-managed rooms

> **Off by default: this feature is in development.** Co-managed is switched off for the whole app until you add `"coManagedEnabled": true` to `settings.json` and restart AC. While the switch is off, no room is co-managed, the **Co-managed** checkbox is not drawn, and AC makes no Jev calls, whatever the room flags and the key say.

For developers who want a room's orchestrator to be read and routed automatically when it goes idle. After this page you know what Co-managed does, how to turn it on, how to write the category catalog, what the red dot means, and what the feature explicitly does not prove.

**Co-managed is off by default for the whole app and, once switched on, per room and off by default, and applies only to that room's orchestrator.** When the orchestrator reaches idle, AC takes the latest complete captured agent message, classifies it with [Jev](#what-jev-is-and-what-leaves-your-machine), and routes it to one of exactly four destinations. It reuses the transcript capture AC already runs for the [Telegram bridge](telegram-bridge.md), it **never requires a Telegram bot**, and enabling or disabling Telegram does not change it.

## What it does not prove

Two claims this feature never makes, and never lets a document make for it:

- **Idle is the gate, not proof that the agent finished.** AC acts at the orchestrator's idle edge because that is the only trigger it has. For a Claude session the turn boundary is unverified, so the candidate is presented as the latest captured assistant text at the idle edge, never as the final message of the turn.
- **An automatic reply is never your approval.** The `default_reply` destination sends a fixed sentence you wrote. It is the room's own canned answer, and it carries no authority you did not already put in it. The catalog loader **rejects** a fixed reply that reads like approval, naming the offending category, so a catalog can never manufacture your consent.

## Turning it on

Three steps, in this order:

1. **Turn on the global switch.** Add `"coManagedEnabled": true` to `settings.json` and restart AC. There is no UI control for it, and it takes effect only on restart. See [Settings: Co-managed (Jev)](../reference/settings.md#co-managed-jev).
2. **Set a Jev API key.** Settings > Integrations > **Co-managed (Jev)** > `Jev API Key`. An empty key leaves the feature inert everywhere. The five other `jev*` keys have working defaults; see [Settings: Co-managed (Jev)](../reference/settings.md#co-managed-jev).
3. **Turn on the room.** In the sidebar's project panel, the orchestrator row of the room carries a **Co-managed** checkbox, tooltipped `Let Jev read this room's orchestrator activity and route it when the room is idle`. One control per room, on the row that already carries the status dot. It writes `enabled` into `<room-root>/.co-managed/config.json`.

Then point the room at a **category catalog**, the file that says what to do with a classified message. Set `catalogPath` in that same `config.json`; there is no UI field for it yet. Without a catalog the room stays off with the reason `Set a category catalog file for this room.`

The room flag lives under the room root, so it **dies with the room**, and `room-*/` is gitignored, so nothing from `.co-managed/` enters your repository.

## The seven reasons a room is off

The orchestrator row shows the reason under the toggle when Co-managed is not effective. The reasons are checked in this order, and exactly one is reported:

| Reason | What you see | Fix |
|---|---|---|
| `GlobalSwitchOff` | no line: the checkbox is not drawn while the switch is off | Add `"coManagedEnabled": true` to `settings.json` and restart AC. |
| `NotAnOrchestrator` | Only this room's orchestrator can be co-managed. | Use the orchestrator row; worker replicas cannot be co-managed. |
| `UnsupportedProvider` | `<agent>` has no transcript reader, so nothing can be captured. | Run the orchestrator on a coding agent AC can read a transcript for (Claude or Codex). Antigravity and Pi take the PTY fallback, and Muse is rejected outright. |
| `RoomFlagOff` | no line: the toggle itself is the answer | Turn the room's Co-managed toggle on. |
| `NoApiKey` | Add a Jev API key in Settings. | Settings > Integrations > Co-managed (Jev). |
| `NoCatalogFile` | Set a category catalog file for this room. | Set `catalogPath` in `<room-root>/.co-managed/config.json`. |
| `CatalogUnreadable` | The category catalog could not be read. | Fix the catalog file; the reason names the problem. |

Every one of them is visible. A room is never silently inert.

## The four destinations

The system knows four destinations and nothing else:

| Destination | Where the message goes |
|---|---|
| `user` | Surfaced to you in the app. |
| `orchestrator` | Another room's orchestrator, named by a peer FQN in the catalog entry. |
| `root` | The [Root Agent](../glossary.md#root-agent). |
| `default_reply` | A fixed reply you wrote, sent back into the session. |

A category that names anything else, omits its question, or lacks the field its destination needs is an **abstention with a visible reason**, never a guess. Validation is per category, so one broken entry does not disable its valid siblings.

## The category catalog

The catalog is yours. AC invents no category vocabulary: you name the categories, you write the yes/no question that decides each one, and you pick its destination.

```json
{
  "categories": {
    "needs-my-decision": {
      "question": "Is the agent asking the user to make a decision?",
      "destination": "user"
    },
    "hand-off-to-review": {
      "question": "Is the agent reporting finished work that another room must review?",
      "destination": "orchestrator",
      "peer": "myproject:room-12-review-team/reviewer"
    },
    "keep-going": {
      "question": "Is the agent waiting only for a go-ahead it already has?",
      "destination": "default_reply",
      "reply": "Continue with the plan as written."
    }
  }
}
```

Three things you need to know and cannot guess from the file's shape:

1. **Each category maps to exactly one of the four destinations.** Anything else abstains, with a reason naming the category.
2. **A fixed reply that expresses approval is rejected when the catalog loads**, with the offending category named. The denied phrases are compared case-insensitively and include `approved`, `go ahead`, `the user agrees`, `authorised`, `authorized`, `lgtm` and `ship it`. A bad catalog fails visibly once, not silently on every turn.
3. **The number of categories affects the classification.** Jev's measured behaviour is order- and composition-sensitive: a faithful repeat of one measured run flipped 3.5% of bands, and changing only the question order collapsed the ranking (Spearman 0.349538 against 0.967303). AC therefore emits the questions in a fixed byte-sorted order by category id, and requires both an absolute score (`jevThreshold`, default `0.70`) and a margin over the runner-up (`jevMargin`, default `0.15`), abstaining otherwise. **Adding or removing a category changes the call, so it can change outcomes for categories you did not touch.** This is not "AI results may vary": it is a property of the call you are composing.

## What Jev is, and what leaves your machine

Jev is **Typesafe System One**, a third-party HTTP endpoint, by default `https://api.typesafe.ai/v1/systemone`. When a room's Co-managed flag is on and a Jev key is set, AC sends the **captured candidate text** and the **catalog's questions** there, at the orchestrator's idle edge. Nothing is sent from a room without the flag, and nothing is sent without a key.

A **pre-egress secret detector** runs first, before any file is written and before any network call. A flagged candidate produces no file, no request and no excerpt: the reason you see names the rule and the candidate's byte length, and nothing else.

See [`PRIVACY.md`](../../PRIVACY.md) and the [security model](../security.md#threat-model).

## The indicator

A co-managed session paints a **red status dot**: `#ff3b5c` in the dark theme, `#dc2626` in the light theme. It lights at the idle edge of an enabled orchestrator that holds a candidate, and goes out when the cycle ends, through action, abstention or a local result.

- **Merely enabling the room flag does not light it.** The dot follows the capture cycle, not the toggle.
- While it is lit, the session **does not show as waiting**: Co-managed wins over `waiting` and `pending` on purpose, because the idle edge is exactly where those would otherwise light up.
- `exited` still wins over Co-managed, so the two never paint the same dot at the same moment.
- **Declared limitation:** Co-managed carries the same red as `exited` and no glow, so colour alone does not distinguish them. The dot's `title` reads `Co-managed`, and the row carries `data-ac-comanaged`.

See [Sidebar guide: what a session row shows](sidebar-guide.md#what-a-session-row-shows).

## Declared limitations

These are deliberate, not defects:

1. **Claude rotation backfill is not routed.** The first sweep of a rotated Claude transcript is treated as backfill and is not routed, although the Telegram bridge does send it. A legitimate first turn can be suppressed.
2. **Attaching over a live reader discards the pending buffer**, so a Telegram chat no longer receives up to 2 s of pre-attach text.
3. **A deep room can make a pointer too long.** Composing the origin suffix lengthens the sender, so a pointer that fits today can stop fitting in a deeply nested room. It is **rejected with a visible reason**, never truncated.
4. **`messaging/` grows faster.** Co-managed writes up to one file per orchestrator turn into the room's `messaging/` directory, and nothing is auto-deleted, matching every other message AC writes.
5. **A co-managed replica row is searchable by the string `comanaged`**, because the dot class is the row's search text.
6. **A room whose orchestrator runs an agent with no transcript reader cannot be co-managed**, and says so with the `UnsupportedProvider` reason.

## See also

- [Settings reference: Co-managed (Jev)](../reference/settings.md#co-managed-jev)
- [Directory layout](../reference/directory-layout.md)
- [Architecture](../reference/architecture.md)
- [Concepts: Session](../concepts.md#session)
- [Security model](../security.md)
- [`PRIVACY.md`](../../PRIVACY.md)
