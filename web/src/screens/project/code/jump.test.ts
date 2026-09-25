import { describe, expect, it } from "vitest";
import type { CodeLocation, CodeUsages } from "../../../api/types";
import { clickLanding, jumpLanding } from "./jump";

const loc = (path: string, line: number, col = 4, name = "Pet"): CodeLocation => ({
  name,
  path,
  line,
  col,
  len: name.length,
  preview: "",
});
const usages = (definitions: CodeLocation[], references: CodeLocation[]): CodeUsages => ({
  symbol: "Pet",
  provider: "native",
  definitions,
  references,
  truncated: false,
});

describe("clickLanding", () => {
  const def = loc("Pet.java", 3, 17);
  it("goes from a use to the only definition", () => {
    expect(clickLanding(usages([def], [loc("Cat.java", 3, 28)]), "Cat.java", 3, 28)).toEqual({ go: def });
  });
  it("lists usages when the click is on the definition", () => {
    const refs = [loc("Cat.java", 3, 28), loc("Dog.java", 3, 28)];
    expect(clickLanding(usages([def], refs), "Pet.java", 3, 18)).toEqual({
      pick: { title: "Usages of Pet (2)", locs: refs },
    });
    expect(clickLanding(usages([def], []), "Pet.java", 3, 17)).toEqual({ note: "Nothing in the project uses Pet." });
  });
  it("lets the user pick among several definitions, and leads nowhere without one", () => {
    const two = [def, loc("other/Pet.java", 1)];
    expect(clickLanding(usages(two, []), "Cat.java", 3, 28)).toMatchObject({ pick: { title: "Definitions of Pet" } });
    expect(clickLanding(usages([], [loc("Cat.java", 3)]), "Cat.java", 3, 4)).toBeNull();
  });
});

describe("jumpLanding", () => {
  it("jumps, lists, notes, or does nothing", () => {
    const one = { provider: "native", symbol: "Dog", truncated: false, locations: [loc("Dog.java", 3)] };
    expect(jumpLanding(one, "supertypes")).toEqual({ go: one.locations[0] });
    expect(jumpLanding({ ...one, locations: [] }, "implementations")).toEqual({
      note: "Nothing in the project extends or implements Dog.",
    });
    expect(jumpLanding({ ...one, locations: [loc("a", 1), loc("b", 2)] }, "implementations")).toMatchObject({
      pick: { title: "What implements Dog" },
    });
    expect(jumpLanding(null, "supertypes")).toBeNull();
  });
});
