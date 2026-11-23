// Which AC placeholders a temp-room agent uses that temp rooms lack.
import { describe, expect, it } from "vitest";
import { FakeTransport } from "./testing/fake-transport";
import { settingsSnapshot } from "./testing/ui-harness";
import {
  createTempRoomAPI,
  tempRoomPlaceholderWarning,
  tempRoomUnavailablePlaceholders,
} from "./temp-rooms";
import type {
  AgentConfig,
  CodingAgentEnv,
  ProfileCellConfig,
  WorkgroupGroupsConfig,
} from "./types";

const W = "%AC_WORKSPACE_ROOT%";
const M = "%AC_MATRIX_ROOT%";
const CHOICE = { agentId: "a", requestedProfile: null };

const env = (value: string, enabled = true): CodingAgentEnv =>
  ({ key: "K", value, source: "user", enabled });

const agent = (o: Partial<AgentConfig> = {}): AgentConfig => ({
  id: "a",
  label: "a",
  command: "claude",
  color: "#000",
  envs: [],
  isolatedHome: false,
  ...o,
});

const cell = (o: Partial<ProfileCellConfig> = {}): ProfileCellConfig => ({
  enabled: true,
  command: "",
  env: {},
  notes: "",
  ...o,
});

function scan(a: AgentConfig, c?: ProfileCellConfig) {
  const s = settingsSnapshot({ agents: [a] });
  if (c) s.codingAgentProfiles.profilesByAgent = { a: { A: c } };
  return tempRoomUnavailablePlaceholders(s, CHOICE);
}

describe("#4786 tempRoomUnavailablePlaceholders", () => {
  it("no choice or a clean agent gives []", () => {
    expect(tempRoomUnavailablePlaceholders(settingsSnapshot({ agents: [agent()] }), undefined))
      .toEqual([]);
    expect(scan(agent())).toEqual([]);
  });

  it("finds the matrix token in the command", () => {
    expect(scan(agent({ command: `claude --dir ${M}` }))).toEqual([M]);
  });

  it("finds the workspace token in an enabled agent env; ignores a disabled one", () => {
    expect(scan(agent({ envs: [env(`${W}/x`)] }))).toEqual([W]);
    expect(scan(agent({ envs: [env(`${W}/x`, false)] }))).toEqual([]);
  });

  it("finds tokens in an enabled profile cell env and preflight; ignores a disabled cell", () => {
    expect(scan(agent(), cell({ env: { X: M } }))).toEqual([M]);
    expect(scan(agent(), cell({ preflight: `echo ${W}` }))).toEqual([W]);
    expect(scan(agent(), cell({ enabled: false, command: M, env: { X: W } }))).toEqual([]);
  });

  it("%AC_REPLICA_ROOT% alone gives []", () => {
    expect(scan(agent({ command: "x %AC_REPLICA_ROOT%" }))).toEqual([]);
  });

  it("both tokens come in fixed order, each once", () => {
    expect(scan(agent({ command: `${M} ${M}`, preflight: W }))).toEqual([W, M]);
  });

  it("an unknown agent id gives []", () => {
    const s = settingsSnapshot({ agents: [agent({ command: M })] });
    expect(tempRoomUnavailablePlaceholders(s, { agentId: "gone", requestedProfile: null }))
      .toEqual([]);
  });

  it("warning text is exact", () => {
    expect(tempRoomPlaceholderWarning(M)).toBe(
      "%AC_MATRIX_ROOT% does not apply to Temporary Rooms: it does not exist for them.",
    );
  });
});

describe("#4805 TempRoomAPI groups", () => {
  it("getGroups and updateGroups invoke the temp-room group commands", async () => {
    const fake = new FakeTransport();
    const cfg = { groups: [], nonStop: null } as unknown as WorkgroupGroupsConfig;
    fake.resolve("get_temp_room_groups", cfg);
    fake.resolve("update_temp_room_groups", cfg);
    const api = createTempRoomAPI(fake);
    await api.getGroups();
    await api.updateGroups(cfg);
    expect(fake.calls).toEqual([
      { cmd: "get_temp_room_groups", args: {} },
      { cmd: "update_temp_room_groups", args: { config: cfg } },
    ]);
  });
});
