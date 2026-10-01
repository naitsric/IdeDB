import type { ButtonHTMLAttributes, ComponentProps, ReactNode } from "react";
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
}: ComponentProps<"button"> & { variant?: ButtonVariant }) {
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

/** An on/off switch, for settings that apply at once. */
export function Switch({
  checked,
  onCheckedChange,
  label,
  disabled,
}: {
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onCheckedChange(!checked)}
      className={cx(
        "relative inline-flex h-[18px] w-[30px] shrink-0 items-center rounded-full transition-colors",
        "disabled:pointer-events-none disabled:opacity-40",
        checked ? "bg-accent" : "bg-border-strong",
      )}
    >
      <span
        className={cx(
          "inline-block size-[14px] rounded-full bg-white shadow-sm transition-transform",
          checked ? "translate-x-[14px]" : "translate-x-[2px]",
        )}
      />
    </button>
  );
}

export interface SegmentOption<T extends string> {
  value: T;
  label: ReactNode;
  /** Why it can't be chosen; shown as its tooltip. */
  blocked?: string | null;
  title?: string;
  /** Text color while selected, e.g. `text-warning`. */
  selectedClassName?: string;
}

/**
 * One choice out of a few, as a row of segments. A blocked option keeps
 * its tooltip (a disabled button would lose it) and ignores clicks.
 */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
  disabled,
}: {
  value: T;
  options: readonly SegmentOption<T>[];
  onChange: (value: T) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <div
      role="radiogroup"
      aria-label={label}
      aria-disabled={disabled || undefined}
      className="flex shrink-0 items-center rounded-md bg-inset p-0.5 text-[11.5px]"
    >
      {options.map((option) => {
        const selected = option.value === value;
        const blocked = disabled || !!option.blocked;
        return (
          <button
            key={option.value}
            type="button"
            role="radio"
            aria-checked={selected}
            aria-disabled={blocked || undefined}
            title={option.blocked ?? option.title}
            onClick={() => !blocked && !selected && onChange(option.value)}
            className={cx(
              "h-5 rounded px-2 font-medium transition-colors",
              selected ? cx("bg-elevated text-fg shadow-sm", option.selectedClassName) : "text-muted",
              !selected && !blocked && "hover:text-fg",
              blocked && !selected && "opacity-40",
            )}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}

export { cx };
