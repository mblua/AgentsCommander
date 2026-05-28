import { describe, it, expect } from 'vitest';
import { stripFrontmatter, taskFirstLine } from './markdown';

describe('stripFrontmatter', () => {
  it('empty_input_returns_empty', () => {
    expect(stripFrontmatter('')).toBe('');
  });

  it('no_frontmatter_returns_unchanged', () => {
    const body = 'Hello world\nLine two';
    expect(stripFrontmatter(body)).toBe(body);
  });

  it('strips_basic_frontmatter', () => {
    const input = "---\ntitle: 'Foo'\n---\n\nBody text";
    expect(stripFrontmatter(input)).toBe('Body text');
  });

  it('strips_multiline_frontmatter', () => {
    const input = "---\ntitle: 'Foo'\nstatus: active\nowner: alice\n---\n\nBody text";
    expect(stripFrontmatter(input)).toBe('Body text');
  });

  it('handles_crlf_line_endings', () => {
    const input = "---\r\ntitle: 'Foo'\r\n---\r\n\r\nBody text";
    expect(stripFrontmatter(input)).toBe('Body text');
  });

  it('preserves_inline_dashes_in_body', () => {
    const input = "---\ntitle: x\n---\n\nA --- separator in the body\n---\nshould stay";
    expect(stripFrontmatter(input)).toBe('A --- separator in the body\n---\nshould stay');
  });

  it('unclosed_frontmatter_returns_unchanged', () => {
    const input = "---\ntitle: 'never closed'\nbody continues here";
    expect(stripFrontmatter(input)).toBe(input);
  });

  it('frontmatter_only_no_body_returns_empty', () => {
    expect(stripFrontmatter("---\ntitle: x\n---")).toBe('');
    expect(stripFrontmatter("---\ntitle: x\n---\n")).toBe('');
  });

  it('triple_dash_not_at_start_is_not_frontmatter', () => {
    const input = "Heading\n---\nfield: value\n---\nBody";
    expect(stripFrontmatter(input)).toBe(input);
  });

  it('trailing_whitespace_after_closing_delimiter_handled', () => {
    const input = "---\ntitle: x\n---   \n\nBody";
    expect(stripFrontmatter(input)).toBe('Body');
  });

  it('strips_frontmatter_when_text_starts_with_utf8_bom', () => {
    const bom = String.fromCharCode(0xfeff);
    const input = `${bom}---\ntitle: 'Foo'\n---\n\nBody text`;
    expect(stripFrontmatter(input)).toBe('Body text');
  });

  it('strips_lone_bom_with_no_frontmatter', () => {
    const bom = String.fromCharCode(0xfeff);
    const input = `${bom}Hello world`;
    expect(stripFrontmatter(input)).toBe('Hello world');
  });
});

describe('taskFirstLine', () => {
  it('null_input_returns_null', () => {
    expect(taskFirstLine(null)).toBeNull();
  });

  it('undefined_input_returns_null', () => {
    expect(taskFirstLine(undefined)).toBeNull();
  });

  it('empty_input_returns_null', () => {
    expect(taskFirstLine('')).toBeNull();
  });

  it('whitespace_only_returns_null', () => {
    expect(taskFirstLine('   \n\n\t\r\n')).toBeNull();
  });

  it('returns_first_non_empty_line', () => {
    expect(taskFirstLine('Hello\nworld')).toBe('Hello');
  });

  it('skips_leading_blank_lines', () => {
    expect(taskFirstLine('\n\n  \nFirst content\nSecond')).toBe('First content');
  });

  it('strips_single_heading_marker', () => {
    expect(taskFirstLine('# Title')).toBe('Title');
  });

  // §9.2 - greedy strip: matches Rust's `trim_start_matches("# ")`. A naive
  // single-slice port would leave one "# " behind here, diverging from
  // discover_project's value and producing two write paths with different
  // results.
  it('strips_repeated_heading_markers_greedily', () => {
    expect(taskFirstLine('# # Title')).toBe('Title');
    expect(taskFirstLine('# # # Deep Title')).toBe('Deep Title');
  });

  it('does_not_strip_heading_marker_without_trailing_space', () => {
    // `# ` with the trailing space is the prefix; `#Title` is not stripped.
    expect(taskFirstLine('#NoSpace')).toBe('#NoSpace');
  });

  it('strips_frontmatter_before_extracting_line', () => {
    const input = "---\ntitle: 'Foo'\n---\n\n# Real Title\nbody";
    expect(taskFirstLine(input)).toBe('Real Title');
  });

  it('handles_crlf_line_endings', () => {
    expect(taskFirstLine('# Windows\r\nbody')).toBe('Windows');
  });

  it('handles_utf8_bom_via_stripFrontmatter', () => {
    const bom = String.fromCharCode(0xfeff);
    expect(taskFirstLine(`${bom}# Title`)).toBe('Title');
  });

  it('frontmatter_only_returns_null', () => {
    expect(taskFirstLine("---\ntitle: x\n---\n")).toBeNull();
  });

  it('trims_surrounding_whitespace_on_picked_line', () => {
    expect(taskFirstLine('   # Title   \nrest')).toBe('Title');
  });
});
