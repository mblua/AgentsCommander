/** UTF-16 code-unit order: exactly what a comparator-less `sort()` does. */
export function compareCodeUnits(a: string, b: string): number {
  if (a < b) return -1;
  if (a > b) return 1;
  return 0;
}
