import type { ReactNode } from "react";
import { Dialog } from "./Dialog";

interface Props {
  open: boolean;
  title: string;
  confirmLabel: string;
  cancelLabel?: string;
  onConfirm: () => void;
  onCancel: () => void;
  /** The action can't be undone (Cancel a Job, remove something): its button is drawn as a warning. */
  danger?: boolean;
  children?: ReactNode;
}

/**
 * A modal that asks before doing something. Declining, Escape and the backdrop
 * all do nothing; focus starts on the safe choice, so Enter never confirms by
 * accident.
 */
export function ConfirmDialog({
  open,
  title,
  confirmLabel,
  cancelLabel = "Cancel",
  onConfirm,
  onCancel,
  danger,
  children,
}: Props) {
  return (
    <Dialog
      open={open}
      onClose={onCancel}
      title={title}
      footer={
        <>
          <button type="button" data-autofocus onClick={onCancel}>
            {cancelLabel}
          </button>
          <button type="button" className={danger ? "danger-solid" : "primary"} onClick={onConfirm}>
            {confirmLabel}
          </button>
        </>
      }
    >
      {children}
    </Dialog>
  );
}
