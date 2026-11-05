import { useEffect } from "react";

export const SITE_NAME = "Gantry";

/** The tab's title for one page, such as "Fleet · Gantry", so every page is told apart (WCAG 2.4.2). */
export function pageTitle(page?: string): string {
  return page ? `${page} · ${SITE_NAME}` : `${SITE_NAME}: 3D printer fleet dashboard`;
}

export function usePageTitle(page: string | undefined) {
  useEffect(() => {
    document.title = pageTitle(page);
  }, [page]);
}
