import { Suspense, lazy, useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { library } from "../library";
import type { PrintFile } from "../library";
import { useFleet } from "../useFleet";
import { usePageTitle } from "../usePageTitle";

// Three.js is big, so it loads only when a toolpath is shown.
const ToolpathViewer = lazy(() => import("../components/ToolpathViewer"));

/**
 * A Plate's toolpath (PRD §8.2, level 3). While a Printer is printing this
 * Plate, its current layer is highlighted and followed.
 */
export function ToolpathPage() {
  const params = useParams();
  const id = Number(params.id);
  const plate = Number(params.plate);
  const { printers } = useFleet();
  const [file, setFile] = useState<PrintFile | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    library.get(id).then(
      (f) => live && setFile(f),
      (e: unknown) => live && setError(e instanceof Error ? e.message : String(e)),
    );
    return () => {
      live = false;
    };
  }, [id]);

  const name = file ? `${file.name} · Plate ${plate}` : "Toolpath";
  usePageTitle(`${name}: toolpath`);
  const printing = printers.find((p) => p.job?.print_file_id === id && p.job?.plate === plate && p.status.job);
  const colours = file?.plates.find((p) => p.number === plate)?.filaments.map((f) => f.colour);

  return (
    <section aria-labelledby="toolpath-heading">
      <div className="toolbar">
        <h1 id="toolpath-heading">{name}: toolpath</h1>
        <Link to="/library" className="button">
          Back to the Library
        </Link>
      </div>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      <Suspense fallback={<p className="loading">Loading the viewer…</p>}>
        <ToolpathViewer
          printFileId={id}
          plate={plate}
          filamentColours={colours}
          liveLayer={printing?.status.job?.layer ?? null}
          liveName={printing?.name}
          label={`The toolpath of ${name}`}
        />
      </Suspense>
    </section>
  );
}
