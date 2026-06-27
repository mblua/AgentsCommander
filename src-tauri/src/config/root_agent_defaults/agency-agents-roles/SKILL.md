---
name: agency-agents-roles
description: How the Root Agent offers Agency Agents role templates before creating any specialist agent: the mandatory offer, identifying Agency Agents from real local data (its source repo and cached templates, never invented), the bounded skip exceptions, the agency-templates CLI flow, and handling a missing local template cache.
when_to_use: Load before creating any new specialist agent, i.e. before any role-defined create-agent-matrix. Also whenever the user asks to add, create, or set up a new specialist role or agent.
---

# agency-agents-roles

## Mandatory offer before creating a specialist agent

Before you create any new specialist agent (any role-defined `create-agent-matrix`), you MUST first offer Agency Agents role templates. This is mandatory, not discretionary.

Skip the offer ONLY if, in this session, the user already declined Agency templates or explicitly asked for a custom or from-scratch role.

## Say what Agency Agents is, from real data only

When you offer, briefly say what Agency Agents is, but state ONLY what real local data supports. Never invent a description or recall one from memory.

- Agency Agents is a collection of tested, shareable role templates published in a source repository. The real source is the `repo` value reported by `agency-templates status` (and stored in the cache manifest), not a URL you guess.
- There is no local one-line project description to quote. Describe Agency Agents concretely by its source repo plus the actual templates available (their real names and 1-line descriptions from `agency-templates list`), not with invented prose.
- If the template cache is absent (status reports it unavailable), say so and offer to fetch it. Ask before downloading or updating, because it writes to the local template cache.

## On acceptance, use the CLI

Use the AgentsCommander CLI from `AGENTSCOMMANDER_BINARY_PATH`:

    "<AGENTSCOMMANDER_BINARY_PATH>" agency-templates update --ref main
    "<AGENTSCOMMANDER_BINARY_PATH>" agency-templates status --pretty
    "<AGENTSCOMMANDER_BINARY_PATH>" agency-templates list --pretty

`update` refreshes the local cache from the source repo (`--ref` selects the git ref, default `main`). `status` reports whether a cache is present and its repo, ref, and commit. `list` prints each cached template's real `id` and 1-line `description`.

Then present the candidate template(s), each with its real 1-line description from `agency-templates list`, and create with `create-agent-matrix --role-template <id>`. Use only the IDs and descriptions that command returns; never invent template IDs or descriptions.
