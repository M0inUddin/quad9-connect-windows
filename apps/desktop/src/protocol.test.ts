import { describe, expect, it } from "vitest";
import { connectionState, loadPreferences } from "./protocol";
import type { Status } from "./protocol";
const base: Status = {
  type: "status",
  revision: 1,
  recovery_blocked: false,
  observed_matches: false,
  journal: null,
};
describe("service state presentation", () => {
  it("keeps unavailable distinct from off", () => {
    expect(connectionState(null)).toBe("unavailable");
    expect(connectionState(base)).toBe("off");
  });
  it("does not present an interrupted restore as off", () => {
    expect(
      connectionState({
        ...base,
        journal: { phase: "restoring", targets: [] },
      }),
    ).toBe("recovery");
  });
  it("gives recovery precedence over matching configuration", () => {
    expect(
      connectionState({
        ...base,
        recovery_blocked: true,
        observed_matches: true,
        journal: { phase: "committed", targets: [] },
      }),
    ).toBe("recovery");
  });
  it("separates changed settings from matching configuration", () => {
    const status: Status = {
      ...base,
      journal: { phase: "committed", targets: [] },
    };
    expect(connectionState(status)).toBe("conflict");
    expect(connectionState({ ...status, observed_matches: true })).toBe(
      "configured",
    );
  });
  it("uses restored terminal state for off", () => {
    expect(
      connectionState({ ...base, journal: { phase: "restored", targets: [] } }),
    ).toBe("off");
  });
});
describe("local preferences", () => {
  it("ignores corrupt storage and unsupported themes", () => {
    expect(loadPreferences({ getItem: () => "broken" })).toEqual({
      theme: "system",
      targets: [],
    });
    expect(
      loadPreferences({ getItem: () => '{"theme":"neon","targets":null}' })
        .theme,
    ).toBe("system");
  });
  it("never turns malformed saved adapter choices into scope", () => {
    expect(
      loadPreferences({
        getItem: () =>
          '{"targets":[null,{"adapter_id":"anything","family":"ipv4"}]}',
      }).targets,
    ).toEqual([]);
  });
});
