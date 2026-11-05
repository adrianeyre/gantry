import { useId } from "react";
import type { PrinterRecord } from "../api";
import { OUTCOME_LABELS } from "../fleet";
import type { JobOutcome } from "../fleet";
import { ALERT_EVENTS, ALERT_EVENT_LABELS } from "../library";
import type { AlertEvent, Tag } from "../library";

export const OUTCOMES: JobOutcome[] = ["succeeded", "failed", "cancelled", "lost"];

/** Which events, Job Outcomes, and Printers or Tags something (an Alert Rule, a webhook) is for. */
export interface Targeting {
  events: AlertEvent[];
  outcomes: JobOutcome[];
  printers: string[];
  tags: number[];
}

export type Scope = "fleet" | "printers" | "tags";

/** The scope a saved Rule or webhook has. */
export function scopeOf(t: Pick<Targeting, "printers" | "tags">): Scope {
  if (t.printers.length > 0) return "printers";
  if (t.tags.length > 0) return "tags";
  return "fleet";
}

/** Only the Printers or Tags the chosen scope uses. */
export function forScope<T extends Targeting>(t: T, scope: Scope): T {
  return { ...t, printers: scope === "printers" ? t.printers : [], tags: scope === "tags" ? t.tags : [] };
}

/** "Job ended (Failed, Lost), Fault raised". */
export function describeEvents(t: Pick<Targeting, "events" | "outcomes">): string {
  return (
    t.events.map((e) => ALERT_EVENT_LABELS[e]).join(", ") +
    (t.outcomes.length > 0 ? ` (${t.outcomes.map((o) => OUTCOME_LABELS[o]).join(", ")})` : "")
  );
}

/** "The whole Fleet", the Printers' names, or "Tags: …". */
export function describeScope(t: Pick<Targeting, "printers" | "tags">, printers: PrinterRecord[], tags: Tag[]): string {
  if (t.printers.length > 0) return t.printers.map((id) => printers.find((p) => p.id === id)?.name ?? id).join(", ");
  if (t.tags.length > 0) return `Tags: ${t.tags.map((id) => tags.find((x) => x.id === id)?.name ?? id).join(", ")}`;
  return "The whole Fleet";
}

function toggle<T>(list: T[], item: T, on: boolean): T[] {
  return on ? [...list, item] : list.filter((x) => x !== item);
}

interface Props {
  value: Targeting;
  onChange: (value: Targeting) => void;
  scope: Scope;
  onScope: (scope: Scope) => void;
  printers: PrinterRecord[];
  tags: Tag[];
}

/** The events, Job Outcomes and Printers or Tags fieldsets, as Alert Rules and webhooks both use them. */
export function EventTargeting({ value, onChange, scope, onScope, printers, tags }: Props) {
  const group = useId();
  return (
    <>
      <fieldset>
        <legend>Events</legend>
        {ALERT_EVENTS.map((ev) => (
          <label key={ev} className="check">
            <input
              type="checkbox"
              checked={value.events.includes(ev)}
              onChange={(e) => onChange({ ...value, events: toggle(value.events, ev, e.target.checked) })}
            />
            {ALERT_EVENT_LABELS[ev]}
          </label>
        ))}
      </fieldset>
      {value.events.includes("job_ended") && (
        <fieldset>
          <legend>Only these Job Outcomes (none ticked: every Outcome)</legend>
          {OUTCOMES.map((o) => (
            <label key={o} className="check">
              <input
                type="checkbox"
                checked={value.outcomes.includes(o)}
                onChange={(e) => onChange({ ...value, outcomes: toggle(value.outcomes, o, e.target.checked) })}
              />
              {OUTCOME_LABELS[o]}
            </label>
          ))}
        </fieldset>
      )}
      <fieldset>
        <legend>Which Printers</legend>
        <label className="check">
          <input type="radio" name={group} checked={scope === "fleet"} onChange={() => onScope("fleet")} />
          The whole Fleet
        </label>
        <label className="check">
          <input type="radio" name={group} checked={scope === "printers"} onChange={() => onScope("printers")} />
          Chosen Printers
        </label>
        <label className="check">
          <input type="radio" name={group} checked={scope === "tags"} onChange={() => onScope("tags")} />
          Printers with chosen Tags
        </label>
        {scope === "printers" &&
          printers.map((p) => (
            <label key={p.id} className="check indent">
              <input
                type="checkbox"
                checked={value.printers.includes(p.id)}
                onChange={(e) => onChange({ ...value, printers: toggle(value.printers, p.id, e.target.checked) })}
              />
              {p.name}
            </label>
          ))}
        {scope === "tags" &&
          (tags.length === 0 ? (
            <p className="muted indent">No Tags yet: add them under Printers.</p>
          ) : (
            tags.map((t) => (
              <label key={t.id} className="check indent">
                <input
                  type="checkbox"
                  checked={value.tags.includes(t.id)}
                  onChange={(e) => onChange({ ...value, tags: toggle(value.tags, t.id, e.target.checked) })}
                />
                {t.name}
              </label>
            ))
          ))}
      </fieldset>
    </>
  );
}
