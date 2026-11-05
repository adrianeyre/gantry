import { useEffect, useState } from "react";
import type { ChangeEvent, FormEvent } from "react";
import { Link } from "react-router-dom";
import { atLeast } from "../api";
import { useAuth } from "../auth";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { PlateThumbnail } from "../components/PlateThumbnail";
import { STATE_LABELS, formatDuration } from "../fleet";
import type { PrinterView } from "../fleet";
import { formatBytes, library, queue, uploadPrintFile } from "../library";
import type { PrintFile } from "../library";
import { useFleet } from "../useFleet";
import { usePageTitle } from "../usePageTitle";

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Sets the model a plain .gcode was sliced for. */
function ModelForm({ file, onSaved }: { file: PrintFile; onSaved: (f: PrintFile) => void }) {
  const [model, setModel] = useState(file.printer_model ?? "");
  const [error, setError] = useState<string | null>(null);

  async function save(e: FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      onSaved(await library.setModel(file.id, model.trim() || null));
    } catch (err) {
      setError(message(err));
    }
  }

  return (
    <form className="inline" onSubmit={(e) => void save(e)}>
      <label>
        Sliced for
        <input
          aria-label={`Model ${file.name} was sliced for`}
          value={model}
          placeholder="Printer model, such as X1C"
          onChange={(e) => setModel(e.target.value)}
        />
      </label>
      <button type="submit">Set model</button>
      {error && (
        <span role="alert" className="error">
          {error}
        </span>
      )}
    </form>
  );
}

/** The Printer choice that queues for any Compatible Printer (PRD §7.3). */
const ANY_PRINTER = "__any__";

/** Sends one Plate to a Printer now, or queues Plates for it or for any Compatible Printer. */
function SendForm({ file, printers }: { file: PrintFile; printers: PrinterView[] }) {
  const [printer, setPrinter] = useState("");
  const [plate, setPlate] = useState<string>(String(file.plates[0]?.number ?? 1));
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const any = printer === ANY_PRINTER;
  const chosen = printers.find((p) => p.id === printer);

  async function act(how: "start" | "queue") {
    if (!printer) return;
    setBusy(true);
    setResult(null);
    try {
      if (how === "start") {
        const job = await library.start(printer, file.id, Number(plate === "all" ? 1 : plate));
        setResult({ ok: true, text: `Started Job #${job.id} on ${chosen?.name ?? "the Printer"}.` });
      } else {
        const plates = plate === "all" ? undefined : [Number(plate)];
        const added = await queue.add(any ? null : printer, file.id, plates);
        const target = any ? "any Compatible Printer" : (chosen?.name ?? "the Printer");
        setResult({
          ok: true,
          text: `Queued ${added.length} Job${added.length === 1 ? "" : "s"} for ${target}.`,
        });
      }
    } catch (e) {
      setResult({ ok: false, text: message(e) });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="send" role="group" aria-label={`Send ${file.name}`}>
      <label>
        Printer
        <select aria-label={`Printer for ${file.name}`} value={printer} onChange={(e) => setPrinter(e.target.value)}>
          <option value="">Choose a Printer</option>
          <option value={ANY_PRINTER}>Any Compatible Printer (queue only)</option>
          {printers.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
              {p.model ? ` (${p.model})` : ""}
              {p.status.state ? ` · ${STATE_LABELS[p.status.state]}` : ""}
            </option>
          ))}
        </select>
      </label>
      <label>
        Plate
        <select aria-label={`Plate of ${file.name}`} value={plate} onChange={(e) => setPlate(e.target.value)}>
          {file.plates.map((p) => (
            <option key={p.number} value={p.number}>
              Plate {p.number}
              {p.name ? `: ${p.name}` : ""}
            </option>
          ))}
          {file.plates.length > 1 && <option value="all">Every Plate (queue only)</option>}
        </select>
      </label>
      <div className="controls">
        <button
          type="button"
          className="primary"
          disabled={busy || !printer || any || plate === "all"}
          onClick={() => void act("start")}
        >
          Start now
        </button>
        <button type="button" disabled={busy || !printer} onClick={() => void act("queue")}>
          Add to Queue
        </button>
      </div>
      {result && (
        <p role={result.ok ? "status" : "alert"} className={result.ok ? "" : "error"}>
          {result.text}
        </p>
      )}
    </div>
  );
}

function FileCard({
  file,
  printers,
  operate,
  onChanged,
  onRemove,
}: {
  file: PrintFile;
  printers: PrinterView[];
  operate: boolean;
  onChanged: (f: PrintFile) => void;
  onRemove: (f: PrintFile) => void;
}) {
  return (
    <article className="card print-file" aria-label={file.name}>
      <header>
        <h2>{file.name}</h2>
        <span className="badge">{file.kind === "gcode_3mf" ? "3MF" : "G-code"}</span>
      </header>
      <dl className="facts">
        <dt>Sliced for</dt>
        <dd data-testid="sliced-for">{file.printer_model ?? "Unknown"}</dd>
        {file.slicer && (
          <>
            <dt>Slicer</dt>
            <dd>{file.slicer}</dd>
          </>
        )}
        <dt>Size</dt>
        <dd>{formatBytes(file.size_bytes)}</dd>
        {file.filament_kinds.length > 0 && (
          <>
            <dt>Filament</dt>
            <dd>{file.filament_kinds.join(", ")}</dd>
          </>
        )}
      </dl>
      {operate && file.kind === "gcode" && <ModelForm file={file} onSaved={onChanged} />}
      <ul className="plates" aria-label={`Plates of ${file.name}`}>
        {file.plates.map((plate) => (
          <li key={plate.number} className="plate">
            <PlateThumbnail
              printFileId={file.id}
              plate={plate.number}
              known={plate.has_thumbnail}
              label={`Plate ${plate.number} of ${file.name}`}
            />
            <div>
              <strong>
                Plate {plate.number}
                {plate.name ? `: ${plate.name}` : ""}
              </strong>
              <div className="muted">
                {plate.estimated_secs != null ? formatDuration(plate.estimated_secs) : "No estimate"}
                {plate.layers != null && ` · ${plate.layers} layers`}
              </div>
              <Link
                to={`/library/${file.id}/plates/${plate.number}/toolpath`}
                aria-label={`Toolpath of ${file.name} Plate ${plate.number}`}
              >
                Toolpath
              </Link>
              <div className="slots">
                {plate.filaments.map((f, i) => (
                  <span key={i} className="chip">
                    <span className="swatch" style={{ background: f.colour ?? "transparent" }} aria-hidden />
                    {f.kind}
                  </span>
                ))}
              </div>
            </div>
          </li>
        ))}
      </ul>
      {operate && <SendForm file={file} printers={printers} />}
      <footer className="card-links">
        {file.kind === "gcode_3mf" && <Link to={`/library/${file.id}/model`}>3D model</Link>}
        {operate && (
          <button type="button" className="danger" aria-label={`Remove ${file.name}`} onClick={() => onRemove(file)}>
            Remove
          </button>
        )}
      </footer>
    </article>
  );
}

/** The Print File library (PRD §7.3, §8). */
export function LibraryPage() {
  const operate = atLeast(useAuth().user?.role, "operator");
  usePageTitle("Library");
  const { printers } = useFleet();
  const [files, setFiles] = useState<PrintFile[] | null>(null);
  const [status, setStatus] = useState<{ ok: boolean; text: string } | null>(null);
  const [uploading, setUploading] = useState(false);
  const [removing, setRemoving] = useState<PrintFile | null>(null);

  useEffect(() => {
    let live = true;
    library.list().then(
      (f) => live && setFiles(f),
      (e: unknown) => live && setStatus({ ok: false, text: message(e) }),
    );
    return () => {
      live = false;
    };
  }, []);

  async function upload(e: ChangeEvent<HTMLInputElement>) {
    const picked = [...(e.target.files ?? [])];
    e.target.value = "";
    if (picked.length === 0) return;
    setUploading(true);
    setStatus(null);
    for (const file of picked) {
      try {
        const added = await uploadPrintFile(file);
        setFiles((old) => [added, ...(old ?? [])]);
        setStatus({
          ok: true,
          text: `Added ${added.name}: ${added.plates.length} Plate${added.plates.length === 1 ? "" : "s"}.`,
        });
      } catch (err) {
        setStatus({ ok: false, text: `${file.name}: ${message(err)}` });
      }
    }
    setUploading(false);
  }

  async function remove(file: PrintFile) {
    setRemoving(null);
    try {
      await library.remove(file.id);
      setFiles((old) => (old ?? []).filter((f) => f.id !== file.id));
    } catch (e) {
      setStatus({ ok: false, text: message(e) });
    }
  }

  return (
    <section aria-labelledby="library-heading">
      <div className="toolbar">
        <h1 id="library-heading">Library</h1>
        {operate && (
          <label className="button primary upload">
            {uploading ? "Uploading…" : "Upload Print Files"}
            <input
              type="file"
              aria-label="Upload Print Files"
              multiple
              accept=".3mf,.gcode,.gco,.g,.stl,.obj,.step,.stp"
              disabled={uploading}
              onChange={(e) => void upload(e)}
              className="visually-hidden"
            />
          </label>
        )}
      </div>
      <p className="muted">
        Sliced .gcode.3mf and .gcode files. Models (.stl, .obj, .step) need slicing first.
      </p>
      {status && (
        <p role={status.ok ? "status" : "alert"} className={status.ok ? "" : "error"}>
          {status.text}
        </p>
      )}
      {files === null ? (
        <p className="loading">Loading…</p>
      ) : files.length === 0 ? (
        <p className="muted">No Print Files yet.</p>
      ) : (
        <div className="cards">
          {files.map((file) => (
            <FileCard
              key={file.id}
              file={file}
              printers={printers}
              operate={operate}
              onChanged={(f) => setFiles((old) => (old ?? []).map((o) => (o.id === f.id ? f : o)))}
              onRemove={setRemoving}
            />
          ))}
        </div>
      )}
      <ConfirmDialog
        open={removing !== null}
        title={`Remove ${removing?.name ?? ""}?`}
        confirmLabel="Remove"
        onConfirm={() => removing && void remove(removing)}
        onCancel={() => setRemoving(null)}
      >
        The file leaves the library. Jobs that printed it keep their History.
      </ConfirmDialog>
    </section>
  );
}
