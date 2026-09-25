import { describe, expect, it } from "vitest";
import { nextVersion, setCargoLockVersions, setCargoWorkspaceVersion } from "./release-version.mjs";

describe("nextVersion", () => {
  it("bumps each part and resets the lower ones", () => {
    expect(nextVersion("1.4.7", "patch")).toBe("1.4.8");
    expect(nextVersion("1.4.7", "minor")).toBe("1.5.0");
    expect(nextVersion("1.4.7", "major")).toBe("2.0.0");
  });

  it("accepts an explicit version", () => {
    expect(nextVersion("0.1.0", "0.3.0")).toBe("0.3.0");
  });

  it("rejects anything else", () => {
    expect(() => nextVersion("0.1.0", "huge")).toThrow(/patch, minor, major/);
    expect(() => nextVersion("0.1.0", "1.0")).toThrow();
    expect(() => nextVersion("0.1.0-beta", "patch")).toThrow(/not x.y.z/);
  });
});

describe("setCargoWorkspaceVersion", () => {
  const toml = `[workspace]
members = ["src-tauri"]

[workspace.package]
version = "0.1.0"
edition = "2024"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
`;

  it("changes only the workspace package version", () => {
    const out = setCargoWorkspaceVersion(toml, "0.2.0");
    expect(out).toContain('[workspace.package]\nversion = "0.2.0"');
    expect(out).toContain('serde = { version = "1"');
  });

  it("fails without a workspace version", () => {
    expect(() => setCargoWorkspaceVersion("[package]\nversion = \"1.0.0\"\n", "2.0.0")).toThrow();
  });
});

describe("setCargoLockVersions", () => {
  const lock = `version = 4

[[package]]
name = "idedb"
version = "0.1.0"
dependencies = [
 "serde",
]

[[package]]
name = "idedb-core"
version = "0.1.0"

[[package]]
name = "some-registry-crate"
version = "0.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "abc"
`;

  it("bumps path packages and leaves registry crates alone", () => {
    const out = setCargoLockVersions(lock, "0.1.0", "0.2.0");
    expect(out).toContain('name = "idedb"\nversion = "0.2.0"');
    expect(out).toContain('name = "idedb-core"\nversion = "0.2.0"');
    expect(out).toContain('name = "some-registry-crate"\nversion = "0.1.0"');
    expect(out.startsWith("version = 4\n")).toBe(true);
  });
});
