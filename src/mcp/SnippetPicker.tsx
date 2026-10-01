import { Check, Copy } from "lucide-react";
import { useEffect, useState } from "react";
import { cx, IconButton, Segmented } from "../ui/primitives";
import { mcpApi } from "./api";
import { INSTALLED_BRIDGE_COMMAND, snippets, type SnippetId } from "./snippets";

/** How long a copy button shows it copied. */
const COPIED_MS = 1500;

/** The executable the app runs from, once known: it doesn't change while the app runs. */
let knownBridgeCommand: string | undefined;
let loadingBridgeCommand: Promise<string> | undefined;

function loadBridgeCommand(): Promise<string> {
  loadingBridgeCommand ??= (async () => {
    try {
      return (await mcpApi.endpoint()).bridgeCommand ?? INSTALLED_BRIDGE_COMMAND;
    } catch {
      // The snippet still helps: most installs are in /Applications.
      return INSTALLED_BRIDGE_COMMAND;
    }
  })().then((command) => (knownBridgeCommand = command));
  return loadingBridgeCommand;
}

/**
 * What stdio-only clients run to reach IdeDB: the installed app's executable, until the app says which
 * one it runs from.
 */
function useBridgeCommand(): string {
  const [command, setCommand] = useState(knownBridgeCommand ?? INSTALLED_BRIDGE_COMMAND);
  useEffect(() => {
    if (knownBridgeCommand !== undefined) return;
    let mounted = true;
    void loadBridgeCommand().then((loaded) => {
      if (mounted) setCommand(loaded);
    });
    return () => {
      mounted = false;
    };
  }, []);
  return command;
}

/** Copies `text` to the clipboard, and says so for a moment. */
export function CopyButton({ text, label = "Copy", className }: { text: string; label?: string; className?: string }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), COPIED_MS);
    return () => window.clearTimeout(timer);
  }, [copied]);

  return (
    <IconButton
      label={copied ? "Copied" : label}
      className={className}
      onClick={async () => {
        await navigator.clipboard.writeText(text);
        setCopied(true);
      }}
    >
      {copied ? <Check className="size-3.5 text-success" /> : <Copy className="size-3.5" />}
    </IconButton>
  );
}

/** Text to paste somewhere, selectable, with a copy button. */
export function CodeBlock({ text, label, className }: { text: string; label: string; className?: string }) {
  return (
    <div className={cx("relative rounded-md border border-border bg-inset", className)}>
      <pre
        aria-label={label}
        className="selectable max-h-48 overflow-auto py-2 pr-10 pl-3 font-mono text-[11.5px] leading-relaxed whitespace-pre-wrap text-fg [overflow-wrap:anywhere]"
      >
        {text}
      </pre>
      <CopyButton text={text} label={`Copy ${label}`} className="absolute top-1 right-1 bg-inset" />
    </div>
  );
}

/** What to paste into each kind of client, for the server on `port`. */
export function SnippetPicker({ port, token }: { port: number; token?: string }) {
  const all = snippets(port, token, useBridgeCommand());
  const [id, setId] = useState<SnippetId>("claudeCode");
  const snippet = all.find((s) => s.id === id) ?? all[0];

  return (
    <div className="flex flex-col gap-2">
      <div className="flex min-w-0 items-center gap-3">
        <Segmented
          label="MCP client"
          value={snippet.id}
          options={all.map((s) => ({ value: s.id, label: s.label }))}
          onChange={setId}
        />
        <span className="min-w-0 truncate text-[11.5px] text-subtle" title={snippet.hint}>
          {snippet.hint}
        </span>
      </div>
      {/* Every snippet in one cell, the others hidden: as tall as the tallest, so switching never moves the layout. */}
      <div className="grid">
        {all.map((s) => (
          <CodeBlock
            key={s.id}
            text={s.text}
            label={`${s.label} configuration`}
            className={cx("col-start-1 row-start-1", s.id !== snippet.id && "invisible")}
          />
        ))}
      </div>
    </div>
  );
}
