import { useId, useState } from "react";
import type { ReactNode } from "react";

/** Puts text on the clipboard. False when the browser won't (an insecure page, say). */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (!navigator.clipboard) return false;
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

interface Props {
  /** What it is, such as "API token": the copy button and the box are named from it. */
  label: string;
  value: string;
  /** Why it is shown only now, and what to do with it. */
  warning: ReactNode;
  /** Offer it as a text file too, with this name. */
  download?: string;
  /** What the file holds, when not just `value`. */
  downloadText?: string;
  /** Lines of a list, such as recovery codes: shown one per line. */
  multiline?: boolean;
  children?: ReactNode;
}

/**
 * Something Gantry shows once and never again (a new API token, a webhook's
 * signing secret, recovery codes), with a warning, a copy button and,
 * if asked for, a download.
 */
export function OneTimeSecret({ label, value, warning, download, downloadText, multiline, children }: Props) {
  const [copied, setCopied] = useState<boolean | null>(null);
  const warningId = useId();

  return (
    <div className="one-time" role="region" aria-label={`New ${label}`} aria-describedby={warningId}>
      <p className="withdrawn" id={warningId}>
        {warning}
      </p>
      {multiline ? (
        <pre className="secret-value" data-testid="secret-value">
          {value}
        </pre>
      ) : (
        <code className="secret-value" data-testid="secret-value">
          {value}
        </code>
      )}
      <div className="controls">
        <button type="button" aria-label={`Copy the ${label}`} onClick={() => void copyText(value).then(setCopied)}>
          Copy
        </button>
        {download && (
          <a
            className="button"
            href={`data:text/plain;charset=utf-8,${encodeURIComponent(downloadText ?? value)}`}
            download={download}
            aria-label={`Download the ${label}`}
          >
            Download
          </a>
        )}
        <span role="status" className="muted">
          {copied === true ? "Copied." : copied === false ? "This browser won't copy here: select it and copy it yourself." : ""}
        </span>
      </div>
      {children}
    </div>
  );
}
