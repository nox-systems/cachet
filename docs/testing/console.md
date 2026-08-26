# The console lane

The console lane runs vitest over `web/`, the browser console the
deployment serves at `/console` (ADR 0014). It covers two kinds of code
and deliberately not a third.

The pure derivations are everything between a decoded answer and a
pixel: how a byte count is written, how a duration picks its unit, how a
counter value is named for a reader rather than for a query, and the
arithmetic that turns a gap-filled series into an SVG path, an axis top,
and a set of ticks. These are the parts a screenshot would not catch. A
chart whose axis top is the maximum of its own data reads as a full chart
at every scale, and a byte formatter handed a NaN prints one.

The presentational components are the ones that take props and no
queries: the figure row, the ranked bars, the chart, and the four states
the mockups do not draw. They render under Testing Library against
happy-dom with values passed in, so a component test never waits on a
network and never needs a fake for one.

The schemas have their own file, and it is a drift check rather than a
unit test. The console decodes every answer before a screen sees it, so
each case holds a body the worker actually serializes: a config with no
build stamp, a session with its expiry, a health answer from a deployment
that has never collected, a filtered series, and a collection report
whose `gate` is null rather than absent. A field the worker renames fails
there, at the boundary, rather than three components later as an
`undefined` on screen.

What the lane does not do is drive the console against a worker. That is
the workerd lane's job for the routing law and the integration lane's for
the real bundle, and a third copy of "does the deployment answer" here
would be a slower way to learn the same thing.

Run it: `just console`. The build verb is separate: `just web` produces
`web/dist`, which the deploy uploads.
