import * as Dialog from "@radix-ui/react-dialog";
import type { ReactNode } from "react";
import { cx } from "./primitives";

/** Centered modal sheet with the app's chrome. Focus is trapped; Esc closes. */
export function Modal({
  open,
  onClose,
  title,
  description,
  width = 520,
  children,
  footer,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  description?: ReactNode;
  width?: number;
  children: ReactNode;
  footer?: ReactNode;
}) {
  return (
    <Dialog.Root open={open} onOpenChange={(o) => !o && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/25" />
        <Dialog.Content
          style={{ width: `min(${width}px, calc(100vw - 32px))` }}
          className="fixed top-1/2 left-1/2 z-50 flex max-h-[calc(100vh-64px)] -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-lg border border-border-strong bg-elevated shadow-popover outline-none"
          // Radix warns about a missing description unless opted out explicitly.
          {...(description ? {} : { "aria-describedby": undefined })}
        >
          <div className="border-b border-border px-5 pt-4 pb-3">
            <Dialog.Title className="text-[14px] font-semibold text-fg">{title}</Dialog.Title>
            {description && (
              <Dialog.Description className="mt-0.5 text-[12px] text-muted">{description}</Dialog.Description>
            )}
          </div>
          <div className="min-h-0 overflow-y-auto px-5 py-4">{children}</div>
          {footer && <div className="flex items-center gap-2 border-t border-border px-5 py-3">{footer}</div>}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

export function Field({ label, children, className }: { label: string; children: ReactNode; className?: string }) {
  return (
    <label className={cx("grid grid-cols-[92px_1fr] items-center gap-3", className)}>
      <span className="text-right text-[12px] text-muted">{label}</span>
      {children}
    </label>
  );
}

export const inputClass =
  "h-7 w-full min-w-0 rounded-md border border-border bg-inset px-2 text-[12.5px] text-fg outline-none placeholder:text-subtle focus:border-accent";
