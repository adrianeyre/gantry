import { useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";
import { useLocation, useNavigate, useParams } from "react-router-dom";
import { api, fieldErrors } from "../api";
import { LiveCapabilityOverrides } from "../components/CapabilityOverrides";
import type { DriverInfo, FieldSpec, PrinterRecord } from "../api";
import type { DiscoveredState } from "../components/DiscoveredPrinters";
import type { FoundPrinter } from "../library";
import { usePageTitle } from "../usePageTitle";

type Values = Record<string, string | boolean>;

function initialValue(field: FieldSpec, record: PrinterRecord | null, found?: FoundPrinter): string | boolean {
  if (found) {
    const announced = field.kind === "secret" ? found.secrets[field.key] : found.settings[field.key];
    if (announced !== undefined && announced !== null) {
      return field.kind === "boolean" ? Boolean(announced) : String(announced);
    }
  }
  const stored = record?.settings[field.key];
  const value = stored ?? field.default;
  if (field.kind === "boolean") return value === undefined ? false : Boolean(value);
  if (field.kind === "secret") return "";
  return value === undefined || value === null ? "" : String(value);
}

/** Checks what the browser can before sending; the server checks everything again. */
function clientErrors(name: string, driver: DriverInfo | undefined, values: Values, record: PrinterRecord | null) {
  const errors: Record<string, string> = {};
  if (!name.trim()) errors.name = "Required";
  if (!driver) errors.driver = "Choose a Driver";
  for (const field of driver?.fields ?? []) {
    const value = values[field.key];
    if (field.kind === "number" && value !== "" && Number.isNaN(Number(value))) {
      errors[field.key] = "Must be a number";
    }
    if (!field.required) continue;
    if (field.kind === "secret") {
      if (value === "" && !record?.secrets_set.includes(field.key)) errors[field.key] = "Required";
    } else if (value === "") {
      errors[field.key] = "Required";
    }
  }
  return errors;
}

/** Adds a Printer, or edits one when the route has an id. */
export function PrinterFormPage() {
  const { id } = useParams();
  const navigate = useNavigate();
  // From "Found on your network": what the Printer announced about itself.
  const location = useLocation();
  const found = id ? undefined : (location.state as Partial<DiscoveredState> | null)?.discovered;
  const [drivers, setDrivers] = useState<DriverInfo[] | null>(null);
  const [record, setRecord] = useState<PrinterRecord | null>(null);
  const [name, setName] = useState("");
  const [driverId, setDriverId] = useState("");
  const [values, setValues] = useState<Values>({});
  const [clear, setClear] = useState<Record<string, boolean>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const driver = useMemo(() => drivers?.find((d) => d.id === driverId), [drivers, driverId]);
  usePageTitle(id ? (record ? `Edit ${record.name}` : "Edit Printer") : "Add a Printer");

  useEffect(() => {
    (async () => {
      try {
        const list = await api.drivers();
        setDrivers(list);
        const existing = id ? await api.printer(id) : null;
        setRecord(existing);
        const announced = found && list.some((d) => d.id === found.driver) ? found : undefined;
        const chosen = existing?.driver ?? announced?.driver ?? list[0]?.id ?? "";
        setDriverId(chosen);
        setName(existing?.name ?? announced?.name ?? "");
        const spec = list.find((d) => d.id === chosen);
        setValues(Object.fromEntries((spec?.fields ?? []).map((f) => [f.key, initialValue(f, existing, announced)])));
      } catch (e) {
        setFailure(e instanceof Error ? e.message : String(e));
      }
    })();
    // `found` comes with the navigation, so it is read once, with the id.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  function chooseDriver(next: string) {
    setDriverId(next);
    const spec = drivers?.find((d) => d.id === next);
    setValues(Object.fromEntries((spec?.fields ?? []).map((f) => [f.key, initialValue(f, null)])));
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    setFailure(null);
    const local = clientErrors(name, driver, values, record);
    setErrors(local);
    if (Object.keys(local).length > 0 || !driver) return;

    const settings: Record<string, unknown> = {};
    const secrets: Record<string, string | null> = {};
    for (const field of driver.fields) {
      const value = values[field.key];
      if (field.kind === "secret") {
        if (clear[field.key]) secrets[field.key] = null;
        else if (value !== "") secrets[field.key] = String(value);
      } else if (field.kind === "number") {
        if (value !== "") settings[field.key] = Number(value);
      } else if (field.kind === "boolean") {
        settings[field.key] = Boolean(value);
      } else if (value !== "") {
        settings[field.key] = value;
      }
    }
    setBusy(true);
    try {
      const form = { name, driver: driver.id, settings, secrets };
      if (record) await api.updatePrinter(record.id, form);
      else await api.createPrinter(form);
      navigate("/printers");
    } catch (e) {
      const fields = fieldErrors(e);
      if (fields.length > 0) setErrors(Object.fromEntries(fields.map((f) => [f.field, f.message])));
      else setFailure(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  if (drivers === null) {
    return (
      <section aria-labelledby="printer-form-heading">
        <div className="toolbar">
          <h1 id="printer-form-heading">{id ? "Edit Printer" : "Add a Printer"}</h1>
        </div>
        {failure ? <p role="alert">{failure}</p> : <p className="loading">Loading…</p>}
      </section>
    );
  }

  const errorFor = (key: string) =>
    errors[key] ? (
      <span className="error" id={`${key}-error`} role="alert">
        {errors[key]}
      </span>
    ) : null;

  return (
    <section aria-labelledby="printer-form-heading">
      <div className="toolbar">
        <h1 id="printer-form-heading">{record ? `Edit ${record.name}` : "Add a Printer"}</h1>
      </div>
      {found && !record && (
        <p className="withdrawn" role="note" data-testid="discovered-note">
          Filled in from what {found.name} announced on your network ({found.host}). Type in any credentials it
          needs yourself, such as its access code or API key: Printers never announce them.
        </p>
      )}
      <form className="form" onSubmit={submit} noValidate>
        <div className="field">
          <label>
            Name
            <input
              name="name"
              value={name}
              required
              onChange={(e) => setName(e.target.value)}
              aria-invalid={Boolean(errors.name)}
              aria-describedby={errors.name ? "name-required name-error" : "name-required"}
            />
          </label>
          <span className="hint" id="name-required">
            Required
          </span>
          {errorFor("name")}
        </div>
        <div className="field">
          <label>
            Driver
            <select
              name="driver"
              value={driverId}
              disabled={Boolean(record)}
              onChange={(e) => chooseDriver(e.target.value)}
            >
              {drivers.map((d) => (
                <option key={d.id} value={d.id}>
                  {d.name}
                </option>
              ))}
            </select>
          </label>
          {errorFor("driver")}
        </div>
        {driver?.fields.map((field) => {
          const value = values[field.key];
          const set = (v: string | boolean) => setValues((old) => ({ ...old, [field.key]: v }));
          const invalid = Boolean(errors[field.key]);
          const stored = field.kind === "secret" && record?.secrets_set.includes(field.key);
          const mustFill = field.required && !stored;
          const described =
            [
              mustFill && `${field.key}-required`,
              field.help && `${field.key}-help`,
              invalid && `${field.key}-error`,
            ]
              .filter(Boolean)
              .join(" ") || undefined;
          if (field.kind === "boolean") {
            return (
              <div key={field.key} className="field">
                <label className="check">
                  <input
                    type="checkbox"
                    name={field.key}
                    aria-describedby={field.help ? `${field.key}-help` : undefined}
                    checked={Boolean(value)}
                    onChange={(e) => set(e.target.checked)}
                  />
                  {field.label}
                </label>
                {field.help && (
                  <span className="hint" id={`${field.key}-help`}>
                    {field.help}
                  </span>
                )}
              </div>
            );
          }
          return (
            <div key={field.key} className="field">
              <label>
                {field.label}
                <input
                name={field.key}
                type={field.kind === "secret" ? "password" : field.kind === "number" ? "number" : "text"}
                autoComplete={field.kind === "secret" ? "new-password" : "off"}
                required={mustFill || undefined}
                value={String(value ?? "")}
                placeholder={stored ? "Saved. Leave blank to keep it." : undefined}
                onChange={(e) => set(e.target.value)}
                aria-invalid={invalid}
                aria-describedby={described}
              />
              </label>
              {mustFill && (
                <span className="hint" id={`${field.key}-required`}>
                  Required
                </span>
              )}
              {field.help && (
                <span className="hint" id={`${field.key}-help`}>
                  {field.help}
                </span>
              )}
              {stored && (
                <label className="check">
                  <input
                    type="checkbox"
                    checked={Boolean(clear[field.key])}
                    onChange={(e) => setClear((old) => ({ ...old, [field.key]: e.target.checked }))}
                  />
                  <span>
                    Clear the saved value<span className="visually-hidden"> of {field.label}</span>
                  </span>
                </label>
              )}
              {errorFor(field.key)}
            </div>
          );
        })}
        {failure && (
          <p role="alert" className="error">
            {failure}
          </p>
        )}
        <div className="controls">
          <button type="submit" className="primary" disabled={busy}>
            {record ? "Save" : "Add Printer"}
          </button>
          <button type="button" onClick={() => navigate("/printers")}>
            Back
          </button>
        </div>
      </form>
      {record && <LiveCapabilityOverrides printerId={record.id} />}
    </section>
  );
}
