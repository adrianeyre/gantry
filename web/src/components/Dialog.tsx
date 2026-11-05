import { useEffect, useId, useRef } from "react";
import type { ReactNode } from "react";
import { CloseIcon } from "./icons";

export interface DialogProps {
  open: boolean;
  onClose: () => void;
  title: string;
  /** What the close button says to a screen reader, such as "Close cookie policy". None, no close button. */
  closeLabel?: string;
  children: ReactNode;
  /** Buttons kept in place under the body, such as Cancel and Confirm. */
  footer?: ReactNode;
  /** A long body (a policy) scrolls on its own and can be scrolled from the keyboard. */
  long?: boolean;
  className?: string;
}

/**
 * A modal dialog on the platform's own `<dialog>`: focus moves in (to the
 * element marked `data-autofocus`, or the close button), stays there, Escape
 * and the backdrop close it, and focus goes back to whatever opened it.
 * Everything behind it is inert while it is open.
 */
export function Dialog({ open, onClose, title, closeLabel, children, footer, long, className }: DialogProps) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const returnTo = useRef<HTMLElement | null>(null);
  const closeRef = useRef(onClose);
  useEffect(() => {
    closeRef.current = onClose;
  }, [onClose]);

  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    if (open && !dialog.open) {
      returnTo.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      // jsdom has no showModal: the dialog still opens there, just not modally.
      if (typeof dialog.showModal === "function") dialog.showModal();
      else dialog.setAttribute("open", "");
      dialog.querySelector<HTMLElement>("[data-autofocus]")?.focus();
    } else if (!open && dialog.open) {
      if (typeof dialog.close === "function") dialog.close();
      else dialog.removeAttribute("open");
    }
    if (!open && returnTo.current) {
      // The opener can have gone (a row removed): then focus is left to the page.
      if (returnTo.current.isConnected) returnTo.current.focus();
      returnTo.current = null;
    }
  }, [open]);

  return (
    <dialog
      ref={ref}
      className={["dialog", long && "dialog-long", className].filter(Boolean).join(" ")}
      aria-labelledby={open ? titleId : undefined}
      // Escape: the platform's `cancel`, or the key itself where there is no platform dialog.
      onCancel={(event) => {
        event.preventDefault();
        closeRef.current();
      }}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          closeRef.current();
        }
      }}
      // A click on the backdrop lands on the dialog element itself.
      onClick={(event) => {
        if (event.target === event.currentTarget) closeRef.current();
      }}
    >
      {open && (
        <div className="dialog-frame">
          <div className="dialog-head">
            <h2 id={titleId}>{title}</h2>
            {closeLabel && (
              <button
                type="button"
                className="icon-button"
                aria-label={closeLabel}
                data-autofocus={footer ? undefined : true}
                onClick={onClose}
              >
                <CloseIcon />
              </button>
            )}
          </div>
          <div
            className="dialog-body"
            {...(long ? { tabIndex: 0, role: "region", "aria-labelledby": titleId } : {})}
          >
            {children}
          </div>
          {footer && <div className="dialog-foot">{footer}</div>}
        </div>
      )}
    </dialog>
  );
}
