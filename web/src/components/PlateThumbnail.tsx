import { useState } from "react";
import { thumbnailUrl } from "../library";

interface Props {
  printFileId: number | null | undefined;
  plate: number | null | undefined;
  /** What the picture shows, for screen readers. */
  label: string;
  /** Skip the request when the library says there is none. */
  known?: boolean;
  size?: number;
}

/**
 * A Plate's thumbnail (PRD §8.2, level 1), or a placeholder when the Print
 * File has none or Gantry doesn't know the Print File.
 */
export function PlateThumbnail({ printFileId, plate, label, known = true, size = 96 }: Props) {
  const [failed, setFailed] = useState(false);
  const style = { width: size, height: size };
  if (printFileId == null || plate == null || !known || failed) {
    return (
      <div className="thumbnail placeholder" role="img" aria-label={`${label}: no thumbnail`} style={style}>
        <svg viewBox="0 0 24 24" width={size / 2} height={size / 2} aria-hidden>
          <path
            d="M4 18 L12 6 L20 18 Z M3 20 H21"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinejoin="round"
          />
        </svg>
      </div>
    );
  }
  return (
    <img
      className="thumbnail"
      src={thumbnailUrl(printFileId, plate)}
      alt={label}
      width={size}
      height={size}
      style={style}
      onError={() => setFailed(true)}
    />
  );
}
