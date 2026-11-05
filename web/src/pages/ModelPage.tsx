import { Suspense, lazy, useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { library } from "../library";
import type { PrintFile } from "../library";
import { decodeModel, fetchModel } from "../model3d";
import type { ModelJson, ModelObject } from "../model3d";
import { usePageTitle } from "../usePageTitle";

// Three.js is big, so it loads only when a model is shown.
const ModelViewer = lazy(() => import("../components/ModelViewer"));

/** A Print File's 3D model (PRD §8.2, level 2). */
export function ModelPage() {
  const id = Number(useParams().id);
  const [file, setFile] = useState<PrintFile | null>(null);
  const [model, setModel] = useState<{ json: ModelJson; objects: ModelObject[] } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    library.get(id).then(
      (f) => live && setFile(f),
      (e: unknown) => live && setError(e instanceof Error ? e.message : String(e)),
    );
    fetchModel(id).then(
      (json) => live && setModel({ json, objects: decodeModel(json) }),
      (e: unknown) => live && setError(e instanceof Error ? e.message : String(e)),
    );
    return () => {
      live = false;
    };
  }, [id]);

  const name = file?.name ?? "Print File";
  usePageTitle(`${name}: 3D model`);
  return (
    <section aria-labelledby="model-heading">
      <div className="toolbar">
        <h1 id="model-heading">{name}: 3D model</h1>
        <Link to="/library" className="button">
          Back to the Library
        </Link>
      </div>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      {!model && !error && <p className="loading">Loading the model…</p>}
      {model && (
        <>
          <p className="muted">
            Drag to turn it, scroll or pinch to zoom, right-drag to move it.
            {model.json.simplified &&
              ` Simplified for the browser: ${model.json.triangles.toLocaleString()} of ${model.json.original_triangles.toLocaleString()} triangles.`}
          </p>
          <Suspense fallback={<p className="loading">Loading the viewer…</p>}>
            <ModelViewer objects={model.objects} label={`3D model of ${name}`} />
          </Suspense>
          <ul className="legend" aria-label="Objects">
            {model.objects.map((o, i) => (
              <li key={i}>
                <span className="swatch" style={{ background: o.colour }} aria-hidden />
                {o.name}
                {o.filament != null && <span className="muted"> · filament {o.filament}</span>}
              </li>
            ))}
          </ul>
        </>
      )}
    </section>
  );
}
