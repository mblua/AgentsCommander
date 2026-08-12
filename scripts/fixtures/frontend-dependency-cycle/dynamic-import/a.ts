export async function loadA(): Promise<unknown> {
  return import("./b");
}
