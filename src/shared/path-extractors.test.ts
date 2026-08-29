import { describe, it, expect } from 'vitest';
import {
  extractProjectName,
  extractWorkgroupName,
  extractAgentName,
  computeTrailingText,
} from './path-extractors';

describe('path-extractors', () => {
  it('empty_input_returns_all_null', () => {
    const w = '';
    expect(extractProjectName(w)).toBeNull();
    expect(extractWorkgroupName(w)).toBeNull();
    expect(extractAgentName(w)).toBeNull();
  });

  it('path_without_ac_root_returns_all_null', () => {
    const w = 'C:\\foo\\bar';
    expect(extractProjectName(w)).toBeNull();
    expect(extractWorkgroupName(w)).toBeNull();
    expect(extractAgentName(w)).toBeNull();
  });

  it('ac_only_returns_project_only', () => {
    const w = 'C:\\foo\\.ac';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractWorkgroupName(w)).toBeNull();
    expect(extractAgentName(w)).toBeNull();
  });

  it('agent_in_wg_returns_all_three', () => {
    const w = 'C:\\foo\\.ac\\wg-19-dev-team\\__agent_tech-lead';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractWorkgroupName(w)).toBe('WG-19-DEV-TEAM');
    expect(extractAgentName(w)).toBe('tech-lead');
  });

  it('repo_in_wg_returns_project_and_wg_no_agent', () => {
    const w = 'C:\\foo\\.ac\\wg-19-dev-team\\repo-X';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractWorkgroupName(w)).toBe('WG-19-DEV-TEAM');
    expect(extractAgentName(w)).toBeNull();
  });

  it('bare_underscore_agent_returns_no_agent', () => {
    const w = 'C:\\foo\\.ac\\wg-1\\__agent_';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractWorkgroupName(w)).toBe('WG-1');
    expect(extractAgentName(w)).toBeNull();
  });

  it('nested_ac_root_uses_innermost', () => {
    const w = 'C:\\proj\\.ac\\wg-1-outer\\repo-AC\\.ac\\wg-2-inner\\__agent_alice';
    expect(extractProjectName(w)).toBe('repo-AC');
    expect(extractWorkgroupName(w)).toBe('WG-2-INNER');
    expect(extractAgentName(w)).toBe('alice');
  });

  it('unc_prefix_handled', () => {
    const w = '\\\\?\\C:\\proj\\.ac\\wg-1\\__agent_x';
    expect(extractProjectName(w)).toBe('proj');
    expect(extractWorkgroupName(w)).toBe('WG-1');
    expect(extractAgentName(w)).toBe('x');
  });

  it('trailing_slash_handled', () => {
    const w = 'C:\\foo\\.ac\\wg-1\\__agent_x\\';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractWorkgroupName(w)).toBe('WG-1');
    expect(extractAgentName(w)).toBe('x');
  });

  it('lax_wg_segment_rejected_no_digits', () => {
    const w = 'C:\\foo\\.ac\\wg-foo\\__agent_x';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractWorkgroupName(w)).toBeNull();
    expect(extractAgentName(w)).toBe('x');
  });

  it('lax_wg_segment_rejected_bare_dash', () => {
    const w = 'C:\\foo\\.ac\\wg-\\__agent_x';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractWorkgroupName(w)).toBeNull();
    expect(extractAgentName(w)).toBe('x');
  });

  it('forward_slashes_handled', () => {
    const w = '/foo/.ac/wg-1/__agent_x';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractWorkgroupName(w)).toBe('WG-1');
    expect(extractAgentName(w)).toBe('x');
  });
});

describe('computeTrailingText', () => {
  it('project_and_agent_returns_agent_at_project', () => {
    const w = 'C:\\foo\\.ac\\wg-19-dev-team\\__agent_alice';
    expect(computeTrailingText(w, null)).toBe('alice@foo');
    expect(computeTrailingText(w, 'session-x')).toBe('alice@foo');
  });

  it('agent_only_returns_agent', () => {
    const w = '\\.ac\\wg-1\\__agent_alice';
    expect(extractProjectName(w)).toBeNull();
    expect(extractAgentName(w)).toBe('alice');
    expect(computeTrailingText(w, null)).toBe('alice');
    expect(computeTrailingText(w, 'session-x')).toBe('alice');
  });

  it('project_and_session_no_agent_returns_session_at_project', () => {
    const w = 'C:\\foo\\.ac\\wg-1\\repo-X';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractAgentName(w)).toBeNull();
    expect(computeTrailingText(w, 'my-session')).toBe('my-session@foo');
  });

  it('session_only_returns_session', () => {
    const w = 'C:\\unrelated\\path';
    expect(extractProjectName(w)).toBeNull();
    expect(extractAgentName(w)).toBeNull();
    expect(computeTrailingText(w, 'my-session')).toBe('my-session');
  });

  it('nothing_returns_null', () => {
    expect(computeTrailingText('', null)).toBeNull();
    expect(computeTrailingText('', undefined)).toBeNull();
    expect(computeTrailingText('', '')).toBeNull();
    expect(computeTrailingText('C:\\nothing', null)).toBeNull();
  });

  it('nested_ac_root_uses_innermost_for_trailing', () => {
    const w = 'C:\\proj\\.ac\\wg-1-outer\\repo-AC\\.ac\\wg-2-inner\\__agent_alice';
    expect(computeTrailingText(w, null)).toBe('alice@repo-AC');
  });
});

// #1614 section 9.1 frontend tests / section 15.4. F1 (`path-extractors.ts:25`)
// feeds the `titlebar-wg-badge` in BOTH sidebar and terminal titlebars, so an
// unrewired F1 renders no badge at all for a Room, silently. The legacy cases
// above are deliberately kept (Rule P2): the badge is an IDENTITY, so a Room
// and a legacy Workgroup must stay distinguishable in the mixed root.
describe('path-extractors, Room (#1614 F1)', () => {
  it('agent_in_room_returns_all_three', () => {
    const w = 'C:\\foo\\.ac\\room-19-dev-team\\__agent_tech-lead';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractWorkgroupName(w)).toBe('ROOM-19-DEV-TEAM');
    expect(extractAgentName(w)).toBe('tech-lead');
  });

  it('room_and_legacy_at_the_same_slot_render_different_badges', () => {
    expect(extractWorkgroupName('C:\\p\\.ac\\room-1-team\\__agent_x')).toBe('ROOM-1-TEAM');
    expect(extractWorkgroupName('C:\\p\\.ac\\wg-1-team\\__agent_x')).toBe('WG-1-TEAM');
  });

  it('room_forward_slashes_and_unc_handled', () => {
    expect(extractWorkgroupName('/foo/.ac/room-1/__agent_x')).toBe('ROOM-1');
    expect(extractWorkgroupName('\\\\?\\C:\\proj\\.ac\\room-1\\__agent_x')).toBe('ROOM-1');
  });

  it('lax_room_segment_rejected_no_digits', () => {
    const w = 'C:\\foo\\.ac\\room-foo\\__agent_x';
    expect(extractWorkgroupName(w)).toBeNull();
    expect(extractAgentName(w)).toBe('x');
  });

  it('room_repo_returns_project_and_room_no_agent', () => {
    const w = 'C:\\foo\\.ac\\room-19-dev-team\\repo-X';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractWorkgroupName(w)).toBe('ROOM-19-DEV-TEAM');
    expect(extractAgentName(w)).toBeNull();
  });
});
