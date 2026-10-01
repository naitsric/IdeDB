import { describe, expect, it } from "vitest";
import {
  claudeCodeSnippet,
  cursorSnippet,
  endpointUrl,
  genericSnippet,
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

describe("snippets", () => {
  it("offers Claude Code, Cursor and a generic URL and header, all on the port", () => {
    const all = snippets(7999, TOKEN);
    expect(all.map((s) => s.id)).toEqual(["claudeCode", "cursor", "other"]);
    for (const snippet of all) {
      expect(snippet.text).toContain("http://127.0.0.1:7999/mcp");
      expect(snippet.text).toContain(`Bearer ${TOKEN}`);
    }
    expect(all[2].text).toBe(genericSnippet("http://127.0.0.1:7999/mcp", TOKEN));
  });

  it("uses the placeholder unless given the token", () => {
    for (const snippet of snippets(7412)) {
      expect(snippet.text).toContain("Bearer <TOKEN>");
      expect(snippet.text).not.toContain("idedb_");
    }
  });

  it("has no Claude Desktop snippet yet: it needs the stdio bridge", () => {
    expect(snippets(7412).some((s) => /desktop/i.test(s.label))).toBe(false);
  });
});
