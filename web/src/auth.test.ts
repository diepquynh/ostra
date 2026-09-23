import { describe, expect, it } from "vitest";
import { tokenFromHash, tokenFromInput } from "./auth";

describe("sign-in tokens", () => {
  it("reads the token from a hash", () => {
    expect(tokenFromHash("#token=abc123")).toBe("abc123");
    expect(tokenFromHash("#other=1")).toBeNull();
  });

  it("reads a pasted link or a bare token", () => {
    const t = "9a60fc10c6da075549a6427cb28fbaf3";
    expect(tokenFromInput(`http://192.168.0.2:7878/#token=${t}`)).toBe(t);
    expect(tokenFromInput(`  ${t}  `)).toBe(t);
    expect(tokenFromInput("http://192.168.0.2:7878/")).toBeNull();
    expect(tokenFromInput("hello")).toBeNull();
  });
});
