export function getConfirmableExternalUrl(
  href: string,
  currentHref: string = window.location.href
): string | null {
  let url: URL;
  let currentUrl: URL;
  try {
    currentUrl = new URL(currentHref);
    url = new URL(href, currentUrl);
  } catch {
    return null;
  }

  if (url.protocol !== "http:" && url.protocol !== "https:") {
    return null;
  }

  if (url.origin === currentUrl.origin) {
    return null;
  }

  return url.href;
}
