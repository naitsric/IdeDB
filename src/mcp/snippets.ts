/**
 * What to paste into an MCP client to reach IdeDB. A token is only known
 * right after it is created or regenerated; everywhere else the snippets
 * carry {@link TOKEN_PLACEHOLDER} for the user to replace.
 */

export const TOKEN_PLACEHOLDER = "<TOKEN>";

/** The name clients list the server under. */
export const SERVER_NAME = "idedb";

/** The MCP endpoint on `port`: an IP, not `localhost`, which may resolve to `::1` first. */
export function endpointUrl(port: number): string {
  return `http://127.0.0.1:${port}/mcp`;
}

export type SnippetId = "claudeCode" | "cursor" | "other";

export interface Snippet {
  id: SnippetId;
  /** The client, as a tab label. */
  label: string;
  /** Where the text goes. */
  hint: string;
  text: string;
}

/**
 * `text` as one argument for a POSIX shell: as is when nothing in it is
 * special to the shell, else in double quotes with `\`, `"`, `$` and
 * backticks escaped.
 */
export function shellArg(text: string): string {
  if (/^[\w\-.,:/@%+=]+$/.test(text)) return text;
  return `"${text.replace(/[\\"$`]/g, (c) => `\\${c}`)}"`;
}

export function claudeCodeSnippet(url: string, token: string): string {
  const header = shellArg(`Authorization: Bearer ${token}`);
  return `claude mcp add --transport http ${SERVER_NAME} ${shellArg(url)} --header ${header}`;
}

export function cursorSnippet(url: string, token: string): string {
  const config = { mcpServers: { [SERVER_NAME]: { url, headers: { Authorization: `Bearer ${token}` } } } };
  return JSON.stringify(config, null, 2);
}

export function genericSnippet(url: string, token: string): string {
  return `URL:    ${url}\nHeader: Authorization: Bearer ${token}`;
}

/** Every snippet for the server on `port`, with the token or the placeholder. */
export function snippets(port: number, token: string = TOKEN_PLACEHOLDER): Snippet[] {
  const url = endpointUrl(port);
  return [
    { id: "claudeCode", label: "Claude Code", hint: "Run in a terminal.", text: claudeCodeSnippet(url, token) },
    {
      id: "cursor",
      label: "Cursor",
      hint: "Add to ~/.cursor/mcp.json, or .cursor/mcp.json in a project.",
      text: cursorSnippet(url, token),
    },
    {
      id: "other",
      label: "Other Clients",
      hint: "Any client that speaks MCP over Streamable HTTP.",
      text: genericSnippet(url, token),
    },
  ];
}
