import type { ReactNode } from "react";

/**
 * A wide table in its own sideways-scrolling box, so the page itself never
 * scrolls in two directions at 400% zoom or on a phone (WCAG 1.4.10). It can
 * take focus, so the box can be scrolled from the keyboard too.
 */
export function TableScroll({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="table-scroll" role="region" aria-label={label} tabIndex={0}>
      {children}
    </div>
  );
}
