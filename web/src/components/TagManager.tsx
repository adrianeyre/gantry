import { useState } from "react";
import type { FormEvent } from "react";
import { tags as tagApi } from "../library";
import type { Tag } from "../library";

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Admins create and remove Tags (PRD §7.6). */
export function TagManager({ tags, onChange }: { tags: Tag[]; onChange: (tags: Tag[]) => void }) {
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function add(e: FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      const tag = await tagApi.create(name.trim());
      onChange([...tags, tag].sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" })));
      setName("");
    } catch (err) {
      setError(message(err));
    }
  }

  async function remove(tag: Tag) {
    setError(null);
    try {
      await tagApi.remove(tag.id);
      onChange(tags.filter((t) => t.id !== tag.id));
    } catch (err) {
      setError(message(err));
    }
  }

  return (
    <section className="panel" aria-labelledby="tags-heading">
      <h2 id="tags-heading">Tags</h2>
      <p className="muted">Labels for filtering the Fleet, such as a room or a purpose. A Printer can have many.</p>
      {tags.length > 0 && (
        <ul className="tags">
          {tags.map((t) => (
            <li key={t.id} className="chip">
              {t.name}
              <button type="button" className="link" aria-label={`Remove the Tag ${t.name}`} onClick={() => void remove(t)}>
                ×
              </button>
            </li>
          ))}
        </ul>
      )}
      <form className="inline" onSubmit={(e) => void add(e)}>
        <label>
          New Tag
          <input value={name} maxLength={32} onChange={(e) => setName(e.target.value)} />
        </label>
        <button type="submit" disabled={!name.trim()}>
          Add Tag
        </button>
      </form>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
    </section>
  );
}

/** One Printer's Tags: checkboxes for an Admin, names for everyone else. */
export function PrinterTags({
  printer,
  name,
  tags,
  admin,
  onChange,
}: {
  printer: string;
  name: string;
  tags: Tag[];
  admin: boolean;
  onChange: (tags: Tag[]) => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const on = tags.filter((t) => t.printers.includes(printer));
  if (!admin) return <>{on.map((t) => t.name).join(", ")}</>;

  async function toggle(tag: Tag, checked: boolean) {
    setError(null);
    const ids = checked ? [...on.map((t) => t.id), tag.id] : on.filter((t) => t.id !== tag.id).map((t) => t.id);
    // Shown at once, and put back if the server refuses.
    const before = tags;
    onChange(
      tags.map((t) =>
        t.id !== tag.id
          ? t
          : { ...t, printers: checked ? [...t.printers, printer] : t.printers.filter((p) => p !== printer) },
      ),
    );
    try {
      await tagApi.setForPrinter(printer, ids);
    } catch (err) {
      onChange(before);
      setError(message(err));
    }
  }

  return (
    <span className="tag-boxes">
      {tags.map((t) => (
        <label key={t.id} className="check">
          <input
            type="checkbox"
            aria-label={`${t.name} on ${name}`}
            checked={t.printers.includes(printer)}
            onChange={(e) => void toggle(t, e.target.checked)}
          />
          {t.name}
        </label>
      ))}
      {error && (
        <span role="alert" className="error">
          {error}
        </span>
      )}
    </span>
  );
}
