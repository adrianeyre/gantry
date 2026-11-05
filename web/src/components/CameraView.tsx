import { useEffect, useRef, useState, useSyncExternalStore } from "react";

/** A camera quality a viewer can pick. "auto" lets the server choose. */
export type CameraQuality = "auto" | "full" | "medium" | "low";

const QUALITIES: { value: CameraQuality; label: string }[] = [
  { value: "auto", label: "Auto" },
  { value: "full", label: "Full" },
  { value: "medium", label: "Medium" },
  { value: "low", label: "Low" },
];

/**
 * The viewer's chosen quality, for every camera at once: what suits this
 * browser's connection doesn't depend on the Printer. Kept in this browser
 * only. Storage can be missing or refuse (a private window), so every use is
 * guarded and the choice then lasts until the page is reloaded.
 */
const STORAGE_KEY = "gantry.cameraQuality";
const listeners = new Set<() => void>();
let fallback: CameraQuality = "auto";

function isQuality(value: unknown): value is CameraQuality {
  return QUALITIES.some((q) => q.value === value);
}

function readQuality(): CameraQuality {
  try {
    const stored = window.localStorage.getItem(STORAGE_KEY);
    if (isQuality(stored)) return stored;
  } catch {
    // Storage is unavailable: use the in-memory choice.
  }
  return fallback;
}

export function setCameraQuality(quality: CameraQuality) {
  fallback = quality;
  try {
    if (quality === "auto") window.localStorage.removeItem(STORAGE_KEY);
    else window.localStorage.setItem(STORAGE_KEY, quality);
  } catch {
    // Storage is unavailable: the choice lasts for this page only.
  }
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useCameraQuality(): CameraQuality {
  return useSyncExternalStore(subscribe, readQuality, () => "auto");
}

/** The camera URL at `quality`. "auto" leaves the choice to the server. */
export function cameraUrl(printerId: string, quality: CameraQuality) {
  const base = `/api/printers/${printerId}/camera`;
  return quality === "auto" ? base : `${base}?quality=${quality}`;
}

/**
 * An MJPEG stream as an image. A browser can keep a removed image's stream
 * open, and it allows only a few connections to one server, so the stream is
 * cut off as soon as this image goes.
 */
function StreamImage({ src, alt, onError }: { src: string; alt: string; onError: () => void }) {
  const ref = useRef<HTMLImageElement>(null);
  useEffect(() => {
    const image = ref.current;
    // Removing `src` (unlike emptying it) aborts the load without an error.
    return () => image?.removeAttribute("src");
  }, []);
  return <img ref={ref} src={src} alt={alt} width={320} height={240} onError={onError} />;
}

/**
 * A Printer's camera, relayed by Gantry as MJPEG. The stream flows only while
 * this element is on the page: unmounting it closes the connection, and the
 * server stops pulling once nobody is watching.
 *
 * With "Auto", remote viewers get a smaller, slower stream and local ones the
 * full camera. Picking a quality applies to every camera in this browser.
 */
export function CameraView({ printerId, name }: { printerId: string; name: string }) {
  const quality = useCameraQuality();
  const src = cameraUrl(printerId, quality);
  // A failure belongs to one URL: picking another quality tries again.
  const [failedSrc, setFailedSrc] = useState<string | null>(null);
  return (
    <div className="camera-view">
      {failedSrc === src ? (
        <p className="muted">The camera isn't available right now.</p>
      ) : (
        <StreamImage key={src} src={src} alt={`${name} camera`} onError={() => setFailedSrc(src)} />
      )}
      <label className="camera-quality">
        Quality{" "}
        <select
          aria-label={`${name} camera quality`}
          value={quality}
          onChange={(e) => {
            if (isQuality(e.target.value)) setCameraQuality(e.target.value);
          }}
        >
          {QUALITIES.map((q) => (
            <option key={q.value} value={q.value}>
              {q.label}
            </option>
          ))}
        </select>
      </label>
    </div>
  );
}

/** The camera on a card, off until asked for, so no stream runs unwatched. */
export function CardCamera({ printerId, name }: { printerId: string; name: string }) {
  const [open, setOpen] = useState(false);
  return (
    <details className="camera" onToggle={(e) => setOpen((e.target as HTMLDetailsElement).open)}>
      <summary>Camera</summary>
      {open && <CameraView printerId={printerId} name={name} />}
    </details>
  );
}
