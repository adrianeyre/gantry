import { CameraView } from "../components/CameraView";
import type { PrinterView } from "../fleet";
import { has } from "../fleet";
import { useFleet } from "../useFleet";
import { usePageTitle } from "../usePageTitle";

/** Every Printer with a camera capability. */
export function CameraGrid({ printers }: { printers: PrinterView[] }) {
  const cameras = printers.filter((p) => has(p.status, "camera"));
  if (cameras.length === 0) return <p className="muted">No Printer has a camera right now.</p>;
  return (
    <div className="camera-grid">
      {cameras.map((p) => (
        <figure key={p.id} aria-label={p.name} className="camera">
          <CameraView printerId={p.id} name={p.name} />
          <figcaption>{p.name}</figcaption>
        </figure>
      ))}
    </div>
  );
}

export function CamerasPage() {
  const { printers } = useFleet();
  usePageTitle("Cameras");
  return (
    <section aria-labelledby="cameras-heading">
      <div className="toolbar">
        <h1 id="cameras-heading">Cameras</h1>
      </div>
      <CameraGrid printers={printers} />
    </section>
  );
}
