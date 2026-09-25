/** Shared look of the cmdk palettes (Search Everywhere, Query History). */

export const paletteOverlayClass = "fixed inset-0 z-50 bg-black/20";

export const paletteContentClass =
  "fixed left-1/2 top-[14vh] z-50 w-[min(640px,calc(100vw-32px))] -translate-x-1/2 overflow-hidden rounded-lg border border-border-strong bg-elevated shadow-popover";

export const paletteInputClass = "h-11 flex-1 bg-transparent text-[14px] text-fg outline-none placeholder:text-subtle";

export const paletteListClass = "max-h-[min(420px,60vh)] overflow-y-auto p-1.5";

export const paletteGroupClass =
  "[&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:pb-1 [&_[cmdk-group-heading]]:pt-2 [&_[cmdk-group-heading]]:text-[11px] [&_[cmdk-group-heading]]:font-medium [&_[cmdk-group-heading]]:text-subtle";

/** Selected rows invert to the accent; `.muted` text and `kbd`s follow. */
export const paletteItemClass =
  "flex h-8 items-center justify-between gap-4 rounded-md px-2 text-[13px] text-fg data-[disabled=true]:opacity-40 data-[selected=true]:bg-accent data-[selected=true]:text-accent-fg [&[data-selected=true]_kbd]:border-transparent [&[data-selected=true]_kbd]:bg-white/20 [&[data-selected=true]_kbd]:text-accent-fg [&[data-selected=true]_.muted]:text-accent-fg/75";
