import type { ReactNode, SVGProps } from "react";

/**
 * The few icons the app draws, as inline SVG in the Lucide style (24-unit
 * grid, 2px round strokes, `currentColor`). Every one is decorative: the
 * control or text beside it names what it means, so each is hidden from
 * assistive technology.
 */
function Icon({ children, size = 18, ...rest }: SVGProps<SVGSVGElement> & { size?: number; children: ReactNode }) {
  return (
    <svg
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      {...rest}
    >
      {children}
    </svg>
  );
}

type Props = { size?: number; className?: string };

export const CloseIcon = (p: Props) => (
  <Icon {...p}>
    <path d="M18 6 6 18M6 6l12 12" />
  </Icon>
);

export const SunIcon = (p: Props) => (
  <Icon {...p}>
    <circle cx="12" cy="12" r="4" />
    <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" />
  </Icon>
);

export const MoonIcon = (p: Props) => (
  <Icon {...p}>
    <path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" />
  </Icon>
);

export const MonitorIcon = (p: Props) => (
  <Icon {...p}>
    <rect width="20" height="14" x="2" y="3" rx="2" />
    <path d="M8 21h8M12 17v4" />
  </Icon>
);

export const CheckIcon = (p: Props) => (
  <Icon {...p}>
    <path d="M20 6 9 17l-5-5" />
  </Icon>
);

export const ShieldIcon = (p: Props) => (
  <Icon {...p}>
    <path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z" />
    <path d="m9 12 2 2 4-4" />
  </Icon>
);

export const CookieIcon = (p: Props) => (
  <Icon {...p}>
    <path d="M12 2a10 10 0 1 0 10 10 4 4 0 0 1-5-5 4 4 0 0 1-5-5" />
    <path d="M8.5 8.5v.01M16 15.5v.01M12 12v.01M11 17v.01M7 14v.01" />
  </Icon>
);

export const EyeIcon = (p: Props) => (
  <Icon {...p}>
    <path d="M2.06 12.35a1 1 0 0 1 0-.7 10.75 10.75 0 0 1 19.88 0 1 1 0 0 1 0 .7 10.75 10.75 0 0 1-19.88 0" />
    <circle cx="12" cy="12" r="3" />
  </Icon>
);

export const EyeOffIcon = (p: Props) => (
  <Icon {...p}>
    <path d="M10.73 5.08A10.43 10.43 0 0 1 12 5c4.97 0 9.16 3.18 10.5 7a10.5 10.5 0 0 1-1.65 2.94M6.61 6.61A13.53 13.53 0 0 0 1.5 12c1.34 3.82 5.53 7 10.5 7a9.74 9.74 0 0 0 5.39-1.61" />
    <path d="M14.12 14.12a3 3 0 1 1-4.24-4.24M2 2l20 20" />
  </Icon>
);

export const ExternalIcon = (p: Props) => (
  <Icon {...p}>
    <path d="M15 3h6v6M10 14 21 3M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
  </Icon>
);

/** GitHub's mark, as GitHub publishes it for linking to a repository. */
export function GitHubMark({ size = 18 }: Props) {
  return (
    <svg viewBox="0 0 24 24" width={size} height={size} fill="currentColor" aria-hidden="true" focusable="false">
      <path d="M12 .5a11.5 11.5 0 0 0-3.64 22.42c.58.1.79-.25.79-.56v-2c-3.2.7-3.88-1.36-3.88-1.36-.53-1.34-1.3-1.7-1.3-1.7-1.06-.72.08-.71.08-.71 1.17.08 1.79 1.2 1.79 1.2 1.04 1.79 2.73 1.27 3.4.97.1-.76.41-1.27.74-1.56-2.55-.29-5.24-1.28-5.24-5.7 0-1.26.45-2.29 1.2-3.1-.12-.29-.52-1.46.11-3.05 0 0 .98-.31 3.2 1.18a11.1 11.1 0 0 1 5.82 0c2.22-1.5 3.2-1.18 3.2-1.18.63 1.59.23 2.76.11 3.05.75.81 1.2 1.84 1.2 3.1 0 4.43-2.7 5.41-5.26 5.69.42.36.8 1.08.8 2.18v3.23c0 .31.21.67.8.56A11.5 11.5 0 0 0 12 .5z" />
    </svg>
  );
}

/**
 * Gantry's mark: a printer's gantry (two uprights and a crossbar) with the
 * print head on it, laying down the first line. The same drawing as
 * `public/favicon.svg`, in the theme's brand colours.
 */
export function GantryLogo({ size = 28 }: Props) {
  return (
    <svg viewBox="0 0 64 64" width={size} height={size} aria-hidden="true" focusable="false" className="logo">
      <rect width="64" height="64" rx="14" fill="var(--brand-from)" />
      <path d="M14 50V16h36v34" fill="none" stroke="var(--on-accent)" strokeWidth="5" strokeLinecap="round" strokeLinejoin="round" />
      <rect x="25" y="12" width="14" height="13" rx="3" fill="var(--on-accent)" />
      <path d="M32 25v7" stroke="var(--on-accent)" strokeWidth="4" strokeLinecap="round" />
      <path d="M21 44h22" stroke="var(--on-accent)" strokeWidth="5" strokeLinecap="round" />
    </svg>
  );
}
