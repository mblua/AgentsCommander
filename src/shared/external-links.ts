export const EXTERNAL_LINK_REQUEST_EVENT = "agentscommander:external-link-request";

export interface ExternalLinkRequestDetail {
  url: string;
}

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

export function requestExternalLinkConfirmation(href: string): boolean {
  const url = getConfirmableExternalUrl(href);
  if (!url) {
    return false;
  }

  document.dispatchEvent(
    new CustomEvent<ExternalLinkRequestDetail>(EXTERNAL_LINK_REQUEST_EVENT, {
      detail: { url },
    })
  );
  return true;
}
