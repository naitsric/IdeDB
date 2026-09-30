import type { ButtonHTMLAttributes, ReactNode } from "react";
import { twMerge } from "tailwind-merge";
import { formatKeybinding } from "../commands/keymap";

/**
 * Joins class names; on conflicting Tailwind utilities the later one wins
 * (`cx(inputClass, "w-20")` really is 5rem wide). A plain join would leave
 * both classes and let stylesheet order decide, which differs between dev
 * and production builds.
 */
const cx = (...classes: (string | false | null | undefined)[]) => twMerge(classes.filter(Boolean).join(" "));

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";

const variants: Record<ButtonVariant, string> = {
  primary: "bg-accent text-accent-fg hover:brightness-110 active:brightness-95",
  secondary: "bg-elevated text-fg border border-border-strong hover:bg-hover active:bg-active",
  ghost: "text-muted hover:bg-hover hover:text-fg active:bg-active",
  danger: "text-danger hover:bg-hover active:bg-active",
};

export function Button({
  variant = "secondary",
  className,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: ButtonVariant }) {
  return (
    <button
      type="button"
      className={cx(
        "inline-flex h-7 items-center gap-1.5 rounded-md px-2.5 text-[12px] font-medium whitespace-nowrap transition-colors",
        "disabled:pointer-events-none disabled:opacity-40",
        variants[variant],
        className,
      )}
      {...props}
    />
  );
}

export function IconButton({
  label,
  shortcut,
  className,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { label: string; shortcut?: string }) {
  const title = shortcut ? `${label}  ${formatKeybinding(shortcut)}` : label;
  return (
    <button
      type="button"
      aria-label={label}
      title={title}
      className={cx(
        "inline-flex size-7 items-center justify-center rounded-md text-muted transition-colors",
        "hover:bg-hover hover:text-fg active:bg-active disabled:pointer-events-none disabled:opacity-40",
        className,
      )}
      {...props}
    />
  );
}

export function Kbd({ binding, children }: { binding?: string; children?: ReactNode }) {
  return (
    <kbd className="rounded border border-border bg-inset px-1.5 py-px font-sans text-[11px] text-subtle tracking-wider">
      {binding ? formatKeybinding(binding) : children}
    </kbd>
  );
}

export function StatusDot({ tone }: { tone: "success" | "warning" | "danger" | "idle" }) {
  const color = {
    success: "bg-success",
    warning: "bg-warning animate-pulse",
    danger: "bg-danger",
    idle: "bg-subtle",
  }[tone];
  return <span className={cx("inline-block size-2 shrink-0 rounded-full", color)} />;
}

export { cx };
