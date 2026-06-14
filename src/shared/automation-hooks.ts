export type AutomationRole =
  | "agent-preset"
  | "button"
  | "checkbox"
  | "dialog"
  | "list"
  | "menu"
  | "menuitem"
  | "overlay"
  | "row"
  | "separator"
  | "status"
  | "surface"
  | "tab"
  | "textbox";

export function automationAttrs(
  testId: string,
  role: AutomationRole,
  state?: string,
  extra?: Record<string, string | boolean | number | null | undefined>,
): Record<string, string> {
  const attrs: Record<string, string> = {
    "data-ac-testid": testId,
    "data-ac-role": role,
  };

  if (state) {
    attrs["data-ac-state"] = state;
  }

  for (const [key, value] of Object.entries(extra ?? {})) {
    if (value !== null && value !== undefined) {
      attrs[`data-ac-${key}`] = String(value);
    }
  }

  return attrs;
}
