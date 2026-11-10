export function portableAgentRef(path: string): string {
  const last = path.replaceAll("\\", "/").split("/").filter(Boolean).pop() ?? path;
  return last.startsWith("_agent_") ? last : `_agent_${last}`;
}
