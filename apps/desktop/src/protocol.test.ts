import { describe, expect, it } from "vitest";
import { connectionState, eligibleTargets, loadPreferences } from "./protocol";
import type { Status, Adapter, Target } from "./protocol";
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
  it("retains only distinct, currently eligible choices without adding adapters", () => {
    const adapter: Adapter = {
      id: "a956d6e1-8910-4d20-9a67-5e34915c2a10",
      name: "Ethernet",
      candidate: true,
      exclusion_reason: null,
      ipv4_enabled: true,
      ipv6_enabled: true,
      ipv6_usable: false,
      has_ipv4_dns: true,
      has_ipv6_dns: false,
    };
    const ipv4: Target = { adapter_id: adapter.id, family: "ipv4" };
    const ipv6: Target = { ...ipv4, family: "ipv6" };
    const targets: Target[] = [
      ipv4,
      ipv4,
      ipv6,
      { ...ipv4, adapter_id: "missing-adapter" },
    ];
    expect(eligibleTargets(targets, [adapter])).toEqual([ipv4]);
    expect(
      eligibleTargets(targets, [{ ...adapter, candidate: false }]),
    ).toEqual([]);
    expect(
      eligibleTargets([ipv4], [{ ...adapter, ipv4_enabled: false }]),
    ).toEqual([]);
    expect(
      eligibleTargets([ipv6], [{ ...adapter, ipv6_usable: true }]),
    ).toEqual([ipv6]);
    expect(eligibleTargets([], [adapter])).toEqual([]);
  });
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
