import { describe, expect, it } from "vitest";
import { findLinks, localPort } from "./links";

const urls = (s: string) => findLinks(s).map((l) => l.url);

describe("findLinks", () => {
  it("finds bare local addresses agents print", () => {
    expect(urls("or open localhost:9004/plan for the full picture.")).toEqual(["http://localhost:9004/plan"]);
    expect(urls("Local:   127.0.0.1:5173/")).toEqual(["http://127.0.0.1:5173/"]);
    expect(urls("bound to 0.0.0.0:8000")).toEqual(["http://0.0.0.0:8000"]);
    expect(urls("(see localhost:3000)")).toEqual(["http://localhost:3000"]);
    expect(urls("**localhost:9004/plan**")).toEqual(["http://localhost:9004/plan"]);
  });

  it("finds normal URLs and trims sentence punctuation", () => {
    expect(urls("docs at https://example.com/a?b=1, then")).toEqual(["https://example.com/a?b=1"]);
    expect(urls("go to http://localhost:8888/lab?token=abc.")).toEqual(["http://localhost:8888/lab?token=abc"]);
    expect(urls("[link](https://x.dev/p_(1))")).toEqual(["https://x.dev/p_(1)"]);
  });

  it("reports exact offsets (for underlining)", () => {
    const s = "open localhost:9004/plan now";
    const [l] = findLinks(s);
    expect(s.slice(l.start, l.end)).toBe("localhost:9004/plan");
  });

  it("ignores things that aren't links", () => {
    expect(urls("ssh dev@localhost:22 and user@host")).toEqual([]);
    expect(urls("time 12:30:45, ratio 3:2, v1.2.3")).toEqual([]);
    expect(urls("mylocalhost:9000")).toEqual([]);
    expect(urls("localhost:99999")).toEqual([]);
    expect(urls("curl localhost:51…")).toEqual([]);
  });

  it("knows which port a local link needs", () => {
    expect(localPort("http://localhost:9004/plan")).toBe(9004);
    expect(localPort("https://127.0.0.1:8443/")).toBe(8443);
    expect(localPort("https://example.com:8443/")).toBeNull();
    expect(localPort("http://localhost/")).toBeNull();
  });
});
