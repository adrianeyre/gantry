import type { ReactNode } from "react";

/** A link out of Gantry, which says (to everyone, not only screen readers) that it opens a new tab. */
export function ExternalLink({ href, children }: { href: string; children: ReactNode }) {
  return (
    <a href={href} target="_blank" rel="noopener noreferrer">
      {children}
      <span className="visually-hidden"> (opens in a new tab)</span>
    </a>
  );
}
