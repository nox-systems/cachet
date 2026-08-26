# ADR 0015: The counter route answers time and filtered questions

- **Status:** Accepted
- **Date:** 2026-08-26
- **Context doc:** [../../CLAUDE.md](../../CLAUDE.md) §1;
  [../security/threat-model.md](../security/threat-model.md)

## Context

`GET /api/self/events` shipped answering one question shape: totals for
one subject grouped by one blob column over one window, largest first.
The console's screens need two shapes it could not produce. A line of
reads per day is a group by time, and time is not a blob column. Laptop
reads split by outcome is a filter, and the route took none, so the only
way to ask about one caller class was to group by that class and lose
every other dimension.

Both are easy to add carelessly. The credential behind the route is a
Cloudflare API token, and the route's whole safety argument is that no
caller text reaches the statement: a value is either a literal in the
source or something an enum produced. A `WHERE blob4 = '<caller text>'`
would end that argument, and a `GROUP BY <caller text>` would end it
faster.

There was also a reason nobody had noticed the route did not work. The
worker read `CLOUDFLARE_ACCOUNT_ID` and the alchemy program never bound
it, so every deployment answered 503; and the workerd lane bound no
analytics token, so every lane row was refused at the configuration
check before any query ran. The success path had no coverage at all.

## Decision

1. Time buckets join the dimension enum as `hour` and `day`, so `by=day`
   is the same parameter as `by=actor`. A time dimension emits a
   projection rather than a column name, orders ascending rather than by
   count, and bounds itself by the bucket count its window implies.
2. The pair is validated. A bucket finer than its window can hold
   (hourly over a month is 720 rows against a cap of 100) is refused with
   400 `malformed_query`. Truncating instead would answer a chart that
   silently begins partway through its own window.
3. A bucketed answer is gap-filled by the worker before it is served.
   Analytics Engine returns no row for a bucket nothing happened in, so
   the raw answer has holes and a line drawn through them claims traffic
   was smooth when it was absent. The fill is pure, lives in cachet-core,
   and is held by a property-lane law.
4. Filters narrow on `kind`, `outcome`, and `actor` only. Those three
   columns hold values from vocabularies the writers define, so a filter
   value parses into an enum before anything is formatted. `repository`
   and `reference` hold text a pusher chose; they stay groupable and
   never filterable, because a `GROUP BY` names a column where a filter
   names a value.
5. The writers' vocabularies became enums (`StatKind`, `StatOutcome`) to
   make point 4 possible, which also ties the blob order to the query
   mapping with a test where a comment used to name the risk.
6. The SQL API's base address is a variable, `CACHET_STATS_API_URL`,
   defaulting to Cloudflare's. The lane points it at a stub that receives
   the composed statement as text and answers rows the scenario chose.
7. The deployment binds `CLOUDFLARE_ACCOUNT_ID`, and a deploy that sets
   a stats token without one is refused by name rather than deployed into
   a 503.

## Consequences

The console's charts have queries behind them, and the questions they ask
are the questions the route offers rather than questions it composes. The
statement is now asserted byte-for-byte in the lane instead of by four
substring checks, which is the right strength for a statement that runs
with an account token.

The route's success path is covered for the first time: the answer, the
row deserialization, the shaping, the fill, and the exact SQL for each
combination. `storage_unavailable` also gained a deterministic wire
assertion, because the stub can refuse on demand.

The cost is a second address the worker can be pointed at. It is
transport configuration rather than authority, the way `CACHET_JWKS_URL`
already is: the token is what authorises the query, and an operator who
can set worker variables can already set the token.

## Alternatives considered

**A separate `bucket=` parameter beside `by=`.** It would allow a
two-dimensional answer, reads per day split by outcome. Rejected for now:
nothing asks for it, and it doubles the shapes the response type has to
describe. Adding it later is compatible with this.

**Formatted dates as the bucket's name.** Rejected: the answer would then
depend on the SQL engine's calendar formatting, and every reader would
have to agree with it. Epoch seconds mean one thing.

**Filtering on `repository` with an escaped value.** Rejected: escaping
is a weaker claim than absence, and the threat model's sentence about
caller text never reaching the statement is worth more than the
convenience. A repository drill-down can group by the column instead.

**Fixing the account-id binding alone and leaving the lane blind.**
Rejected: the binding bug survived a release precisely because no test
could reach the code it broke, so the seam and the stub are the fix.
