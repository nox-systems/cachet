# The property lane

The property lane runs one `tests/property.rs` per crate through hegel,
isolated from the unit lane so a property failure is its own signal. The
same cases also run inside the unit suite; the lane exists for the
standalone failure name.

Every crate with a property target states laws, not examples. The law
classes in force now:

- Totality. `multipart::part_plan` answers Ok or a typed refusal for any
  u64, never a panic; the key, narinfo, and lease parsers answer over
  arbitrary bytes, never a panic.
- Plan laws. The plan sums to the declared total, non-final parts are
  full, no part is empty, part numbers are one-based and ascending, and
  the count never exceeds the multipart cap.
- Round-trips and fixed points. The narinfo canonical form re-parses and
  re-serializes to itself with the field order locked and unknown lines
  preserved, and the fingerprint string matches its recipe field by
  field.
- GC decision laws. Over a six-key inventory every combination of age
  class and mark is exhausted, computing the expected plan independently
  and comparing it whole: reserved keys are never swept, marked paths
  are never swept, an upload exactly at the grace boundary is kept, a
  tripped fraction gate always means an empty plan, and a NAR URL named
  by any marked narinfo survives.

New classes join this list in the same commit as the code that obeys them.

Run it: `just property`.
