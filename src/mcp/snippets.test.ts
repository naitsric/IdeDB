import { describe, expect, it } from "vitest";
import {
  claudeCodeSnippet,
  claudeDesktopSnippet,
  cursorSnippet,
  endpointUrl,
  genericSnippet,
  INSTALLED_BRIDGE_COMMAND,
  shellArg,
  snippets,
  TOKEN_PLACEHOLDER,
} from "./snippets";

const TOKEN = "idedb_Zm9vYmFyLWJhel9xdXV4LTEyMzQ1Njc4OTAtYWJjZGVm";

describe("endpointUrl", () => {
  it("uses the loopback IP and the port", () => {
    expect(endpointUrl(7412)).toBe("http://127.0.0.1:7412/mcp");
    expect(endpointUrl(65535)).toBe("http://127.0.0.1:65535/mcp");
  });
});

describe("Claude Code", () => {
  it("is the documented claude mcp add command", () => {
    expect(claudeCodeSnippet(endpointUrl(7412), TOKEN)).toBe(
      `claude mcp add --transport http idedb http://127.0.0.1:7412/mcp --header "Authorization: Bearer ${TOKEN}"`,
    );
  });

  it("keeps the placeholder inside quotes, where the shell leaves < and > alone", () => {
    expect(claudeCodeSnippet(endpointUrl(7500), TOKEN_PLACEHOLDER)).toBe(
      'claude mcp add --transport http idedb http://127.0.0.1:7500/mcp --header "Authorization: Bearer <TOKEN>"',
    );
  });
});

describe("shellArg", () => {
  it("leaves plain words alone", () => {
    expect(shellArg("http://127.0.0.1:7412/mcp")).toBe("http://127.0.0.1:7412/mcp");
    expect(shellArg(TOKEN)).toBe(TOKEN);
  });

  it("quotes and escapes what the shell would expand or split", () => {
    expect(shellArg("a b")).toBe('"a b"');
    expect(shellArg('say "hi" $HOME `id` \\n')).toBe('"say \\"hi\\" \\$HOME \\`id\\` \\\\n"');
    expect(shellArg("<TOKEN>")).toBe('"<TOKEN>"');
    expect(shellArg("")).toBe('""');
  });
});

describe("Cursor", () => {
  it("is an mcp.json with the URL and the header", () => {
    expect(JSON.parse(cursorSnippet(endpointUrl(7412), TOKEN))).toEqual({
      mcpServers: { idedb: { url: "http://127.0.0.1:7412/mcp", headers: { Authorization: `Bearer ${TOKEN}` } } },
    });
  });

  it("escapes as JSON", () => {
    const parsed = JSON.parse(cursorSnippet("http://127.0.0.1:1/mcp", 'a"b\\c'));
    expect(parsed.mcpServers.idedb.headers.Authorization).toBe('Bearer a"b\\c');
  });
});

describe("Claude Desktop", () => {
  it("is a claude_desktop_config.json that runs the stdio bridge, with the token in its environment", () => {
    expect(JSON.parse(claudeDesktopSnippet(INSTALLED_BRIDGE_COMMAND, 7412, TOKEN))).toEqual({
      mcpServers: {
        idedb: {
          command: "/Applications/IdeDB.app/Contents/MacOS/idedb",
          args: ["mcp-bridge", "--port", "7412"],
          env: { IDEDB_MCP_TOKEN: TOKEN },
        },
      },
    });
  });

  it("keeps the token out of the arguments, which other processes can read", () => {
    const server = JSON.parse(claudeDesktopSnippet(INSTALLED_BRIDGE_COMMAND, 7412, TOKEN)).mcpServers.idedb;
    expect(server.args.join(" ")).not.toContain(TOKEN);
  });

  it("escapes the executable's path as JSON: spaces stay, quotes and backslashes are escaped", () => {
    const command = '/Users/me/My Apps/IdeDB "dev".app/Contents/MacOS/idedb\\x';
    const text = claudeDesktopSnippet(command, 7500, TOKEN);
    expect(text).toContain('"/Users/me/My Apps/IdeDB \\"dev\\".app/Contents/MacOS/idedb\\\\x"');
    expect(JSON.parse(text).mcpServers.idedb.command).toBe(command);
    expect(JSON.parse(text).mcpServers.idedb.args).toEqual(["mcp-bridge", "--port", "7500"]);
  });

  it("carries the placeholder where the token isn't known", () => {
    const desktop = snippets(7412).find((s) => s.id === "claudeDesktop")!;
    expect(JSON.parse(desktop.text).mcpServers.idedb.env).toEqual({ IDEDB_MCP_TOKEN: "<TOKEN>" });
  });

  it("runs the installed app unless told which executable the app runs from", () => {
    const command = (bridge?: string) =>
      JSON.parse(snippets(7412, TOKEN, bridge).find((s) => s.id === "claudeDesktop")!.text).mcpServers.idedb.command;
    expect(command()).toBe(INSTALLED_BRIDGE_COMMAND);
    expect(command("/Users/me/idedb/target/debug/idedb")).toBe("/Users/me/idedb/target/debug/idedb");
  });
});

describe("snippets", () => {
  it("offers Claude Code, Claude Desktop, Cursor and a generic URL and header, all on the port", () => {
    const all = snippets(7999, TOKEN);
    expect(all.map((s) => s.id)).toEqual(["claudeCode", "claudeDesktop", "cursor", "other"]);
    for (const snippet of all.filter((s) => s.id !== "claudeDesktop")) {
      expect(snippet.text).toContain("http://127.0.0.1:7999/mcp");
      expect(snippet.text).toContain(`Bearer ${TOKEN}`);
    }
    expect(all[1].text).toBe(claudeDesktopSnippet(INSTALLED_BRIDGE_COMMAND, 7999, TOKEN));
    expect(all[3].text).toBe(genericSnippet("http://127.0.0.1:7999/mcp", TOKEN));
  });

  it("uses the placeholder unless given the token", () => {
    for (const snippet of snippets(7412)) {
      expect(snippet.text).toContain(TOKEN_PLACEHOLDER);
      expect(snippet.text).not.toContain("idedb_");
    }
    for (const snippet of snippets(7412).filter((s) => s.id !== "claudeDesktop")) {
      expect(snippet.text).toContain("Bearer <TOKEN>");
    }
  });
});
