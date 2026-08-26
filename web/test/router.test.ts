import { describe, expect, it } from "vitest";

import { eventsSearch } from "../src/api/client.ts";
import { bucketFor } from "../src/screens/traffic.tsx";
import { readSubject, readWindow } from "../src/router.tsx";

describe("search parameters", () => {
  it("reads a choice the deployment offers", () => {
    expect(readSubject("writes")).toBe("writes");
    expect(readWindow("month")).toBe("month");
  });

  it("falls back rather than asking a question that will be refused", () => {
    // A hand-edited URL should land on a real view. The worker answers
    // 400 for anything outside its enums, so a console that passed the
    // text straight through would show a refusal for a typo.
    expect(readSubject("everything")).toBe("reads");
    expect(readSubject(undefined)).toBe("reads");
    expect(readWindow("decade")).toBe("week");
    expect(readWindow(7)).toBe("week");
  });
});

describe("bucketFor", () => {
  it("only asks for hours inside a day", () => {
    // The worker refuses an hourly bucket over a week or a month: 168
    // and 720 rows against a cap of 100. The console never asks.
    expect(bucketFor("day")).toBe("hour");
    expect(bucketFor("week")).toBe("day");
    expect(bucketFor("month")).toBe("day");
  });
});

describe("eventsSearch", () => {
  it("serializes a question in one fixed order", () => {
    // Two identical questions have to produce one string, or they miss
    // each other in the query cache and the screen fetches twice.
    expect(eventsSearch({ subject: "reads", by: "day", window: "week" })).toBe(
      "subject=reads&by=day&window=week",
    );
    expect(
      eventsSearch({
        subject: "reads",
        by: "outcome",
        window: "week",
        actor: "laptop",
      }),
    ).toBe("subject=reads&by=outcome&window=week&actor=laptop");
  });

  it("leaves an unstated filter out entirely", () => {
    const search = eventsSearch({
      subject: "writes",
      by: "kind",
      window: "day",
    });
    expect(search.includes("actor")).toBe(false);
    expect(search.includes("kind=")).toBe(false);
  });
});
