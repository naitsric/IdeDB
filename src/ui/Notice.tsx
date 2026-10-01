import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { cx } from "./primitives";

/** A short note inside a dialog or panel: an icon, a line or two, maybe a button. */
export function Notice({
  tone,
  icon: Icon,
  children,
}: {
  tone: "muted" | "danger" | "warning";
  icon: LucideIcon;
  children: ReactNode;
}) {
  return (
    <div
      className={cx(
        "flex items-center gap-2 rounded-md px-3 py-2 text-[12px]",
        tone === "danger" && "bg-danger/10 text-danger",
        tone === "warning" && "bg-warning/10 text-warning",
        tone === "muted" && "bg-inset text-muted",
      )}
    >
      <Icon className="size-3.5 shrink-0" />
      {children}
    </div>
  );
}
